//! The print and cancel forms. Both are plain form posts answered with a redirect, so they work
//! without JavaScript and a reload never sends a document twice. A refusal goes back to the
//! page with its code (`/?error=too_large`, see [`Refusal`]).
//!
//! With quota rules enabled, a job is checked and entered in the ledger in one write
//! transaction before CUPS is asked, so two jobs racing for the last pages cannot both pass, and
//! any database failure refuses the job: quotas fail closed. Without rules, the ledger is best
//! effort and never stops printing.

use axum::{
    Extension,
    body::Bytes,
    extract::{Multipart, Path, State, multipart::MultipartError},
    http::StatusCode,
    response::Redirect,
};
use recto_app::{
    functions::visible,
    refusal::Refusal,
    state::{AppState, JobVisibility, User},
};
use recto_domain::{
    access::{self, DeviceKind, Usable},
    database::begin_write,
    held, jobs,
    ledger::{self, NewEntry},
    prices::{self, Prices},
    quotas::{self, Metric, Rule, Verdict},
};
use recto_printing::{
    Capabilities, ColorMode, DocumentFormat, PageRanges, PrintError, PrintOptions, PrintRequest,
    Sides,
    count::{Uncounted, printed_pages},
};

/// Room for the form fields and multipart headers next to the largest accepted file, so a file
/// just under the limit is not refused for its envelope.
pub const FORM_SLACK: usize = 64 * 1024;

/// IPP `name` values are at most 255 octets.
const MAX_JOB_NAME: usize = 255;

/// `POST /print`: a multipart form with `file`, `printer`, `copies`, `sides`, `color_mode` and
/// `page_ranges`. Answers `303 /jobs?submitted=<id>`, with `&over=<rule id>,…` when the job went
/// over soft quota rules, `303 /?error=quota&rule=<id>` when a hard rule refused it, or
/// `303 /?error=<code>`.
pub async fn submit(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    multipart: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> Redirect {
    let submitted = match multipart {
        Ok(multipart) => submit_form(&state, &user, multipart).await,
        Err(rejection) => {
            tracing::warn!(%rejection, "unreadable print form");
            Err(Refusal::InvalidForm.into())
        }
    };
    match submitted {
        Ok(Submitted { job, over }) if over.is_empty() => {
            tracing::info!(user = user.username, job, "document sent to the printer");
            Redirect::to(&format!("/jobs?submitted={job}"))
        }
        Ok(Submitted { job, over }) => {
            let over: Vec<String> = over.iter().map(i64::to_string).collect();
            let over = over.join(",");
            tracing::info!(
                user = user.username,
                job,
                rules = over,
                "document sent to the printer over a soft quota"
            );
            Redirect::to(&format!("/jobs?submitted={job}&over={over}"))
        }
        Err(Rejected::Quota(rule)) => {
            tracing::info!(user = user.username, rule, "print refused by a quota");
            Redirect::to(&format!("/?error={}&rule={rule}", Refusal::Quota.code()))
        }
        Err(Rejected::Refusal(refusal)) => Redirect::to(&format!("/?error={}", refusal.code())),
    }
}

/// A job CUPS accepted, and the soft rules it went over.
pub(crate) struct Submitted {
    pub job: i32,
    pub over: Vec<i64>,
}

/// Why a job was not printed.
pub(crate) enum Rejected {
    Refusal(Refusal),
    /// The hard quota rule, by id, that has too little left.
    Quota(i64),
}

impl From<Refusal> for Rejected {
    fn from(refusal: Refusal) -> Self {
        Rejected::Refusal(refusal)
    }
}

/// What a database failure is, while quotas are enforced: a refusal. Printing without being able
/// to check the quotas would let anyone print past them.
fn closed(what: &'static str) -> impl Fn(recto_domain::Error) -> Refusal + Copy {
    move |error| {
        tracing::error!(%error, "{what} failed");
        Refusal::Unavailable
    }
}

/// A document to print, its format already detected from its content.
pub(crate) struct Document {
    /// The name the user knows it by, such as the uploaded file's name.
    pub name: String,
    pub bytes: Bytes,
    pub format: DocumentFormat,
}

/// The printer and the options as typed, before they are checked against the printer. A missing
/// option means the printer's default.
#[derive(Default)]
pub(crate) struct Choices {
    pub printer: String,
    pub copies: Option<String>,
    pub sides: Option<String>,
    pub color_mode: Option<String>,
    pub page_ranges: Option<String>,
}

/// A held job being released, claimed by the caller ([`held::claim`]) before the document is
/// printed. Its ledger entry is never best effort: the job is marked released in the
/// transaction that writes the entry, so it can never be printed twice, and a release whose
/// entry cannot be written is refused.
pub(crate) struct HeldClaim {
    pub id: i64,
    /// The claim the release holds, which marking it released or returning it must name.
    pub claim: i64,
}

async fn submit_form(
    state: &AppState,
    user: &User,
    multipart: Multipart,
) -> Result<Submitted, Rejected> {
    let form = Form::read(multipart, state.max_upload_size).await?;
    let (file_name, bytes) = form.file.ok_or(Refusal::NoFile)?;
    if bytes.is_empty() {
        return Err(Refusal::EmptyFile.into());
    }
    // From the content, never the name or declared type: a renamed document stops here.
    let format = DocumentFormat::detect(&bytes).ok_or(Refusal::UnsupportedFormat)?;
    let printer = form.printer.ok_or(Refusal::InvalidForm)?;
    let document = Document {
        name: file_name,
        bytes,
        format,
    };
    let choices = Choices {
        printer,
        ..form.options
    };
    print_document(state, user, document, &choices, None).await
}

/// Prints a document for a user: the access rules, the page count, the quota check and the
/// ledger entry, then CUPS. The print form and the release of a held job both come here, so
/// neither can print around the other's checks.
///
/// A held job refused before its ledger entry is committed goes back to `held` with the
/// refusal; one CUPS refuses goes back with its entry refunded, in one transaction. One CUPS
/// may have queued (`Unconfirmed`) stays released and counted, like a job of the form.
pub(crate) async fn print_document(
    state: &AppState,
    user: &User,
    document: Document,
    choices: &Choices,
    held: Option<HeldClaim>,
) -> Result<Submitted, Rejected> {
    let held = held.as_ref();
    let (request, entry_id, over) = match counted(state, user, document, choices, held).await {
        Ok(counted) => counted,
        Err(rejected) => {
            if let Some(held) = held {
                give_back(state, held, &rejected).await;
            }
            return Err(rejected);
        }
    };
    let id = match state.print.submit(request).await {
        Ok(id) => id,
        // CUPS may have queued the job and printed it: refunding it now would let it print for
        // free. The entry stays counted, and the sweep refunds it if CUPS never shows it.
        Err(PrintError::Unconfirmed(reason)) => {
            tracing::warn!(
                user = user.username,
                entry = entry_id,
                reason,
                "the printing service did not confirm the job"
            );
            return Err(Refusal::Unconfirmed.into());
        }
        Err(error) => {
            let refusal = refusal(error);
            // CUPS turned the job down or never got it: nothing prints, so it must not count.
            match (held, entry_id) {
                (Some(held), Some(entry_id)) => {
                    if let Err(error) = held::return_refunded(
                        &state.database,
                        held.id,
                        held.claim,
                        entry_id,
                        refusal.code(),
                        chrono::Utc::now(),
                    )
                    .await
                    {
                        tracing::error!(%error, "returning a refused held job failed");
                    }
                }
                (None, Some(entry_id)) => {
                    if let Err(error) = ledger::refund(&state.database, entry_id).await {
                        tracing::error!(%error, "refunding the ledger entry failed");
                    }
                }
                _ => {}
            }
            return Err(refusal.into());
        }
    };
    // The job is printing already: failing to link it must not tell the user it failed.
    if let Some(entry_id) = entry_id
        && let Err(error) = ledger::attach(&state.database, entry_id, id).await
    {
        tracing::error!(
            %error,
            job = id,
            "linking the CUPS job to its ledger entry failed"
        );
    }
    Ok(Submitted { job: id, over })
}

/// Puts a held job refused before its ledger entry back to `held`, with why.
async fn give_back(state: &AppState, held: &HeldClaim, rejected: &Rejected) {
    let code = match rejected {
        Rejected::Quota(_) => Refusal::Quota.code(),
        Rejected::Refusal(refusal) => refusal.code(),
    };
    if let Err(error) = held::unclaim(
        &state.database,
        held.id,
        held.claim,
        Some(code),
        chrono::Utc::now(),
    )
    .await
    {
        tracing::error!(%error, "returning a refused held job failed");
    }
}

/// Everything before CUPS: the checks, and the ledger entry written. Returns the request to
/// send, the entry's id when there is one, and the soft rules the job goes over.
async fn counted(
    state: &AppState,
    user: &User,
    document: Document,
    choices: &Choices,
    held: Option<&HeldClaim>,
) -> Result<(PrintRequest, Option<i64>, Vec<i64>), Rejected> {
    let Document {
        name: file_name,
        bytes: document,
        format,
    } = document;
    let printer_name = choices.printer.as_str();
    // Before CUPS is asked anything: a printer the user may not use is unknown to them. Rules
    // that cannot be read refuse everything, the open printers too.
    let usable = Usable::read(
        &state.database,
        DeviceKind::Printer,
        user.id,
        user.is_administrator,
    )
    .await
    .map_err(closed("reading the printer access rules"))?;
    if !usable.allows(printer_name) {
        return Err(Refusal::PrinterNotFound.into());
    }
    // Rules that cannot be read might be there: refused, like access rules. Whether they are
    // enforced is decided again in the write transaction; this only prepares the job.
    let enforced = quotas::rules(&state.database)
        .await
        .map_err(closed("reading the quota rules"))?
        .iter()
        .any(|rule| rule.enabled);
    let printer = state.print.printer(printer_name).await.map_err(refusal)?;
    // CUPS ignores the case of printer names, so "LAB" reaches the printer "lab": its rule is
    // checked again under the name CUPS answered with.
    if !usable.allows(&printer.name) {
        return Err(Refusal::PrinterNotFound.into());
    }
    if !printer.accepting_jobs {
        return Err(Refusal::NotAccepting.into());
    }
    let options = options(choices, &printer.capabilities, format)?;
    let document_pages = match format {
        // Bounded in concurrency and time, and not copied: see `PageCounter`.
        DocumentFormat::Pdf => state.page_counter.count(document.clone()).await,
        DocumentFormat::Jpeg | DocumentFormat::Png => Ok(1),
    };
    let uncounted = document_pages.err();
    let document_pages = document_pages.ok();
    let printed = document_pages.map(|pages| printed_pages(pages, options.page_ranges.as_ref()));
    // The cost is fixed now: a later price change leaves this job's cost alone.
    let (prices, priced) = match prices::get(&state.database, &printer.name).await {
        Ok(prices) => (prices, true),
        Err(error) if enforced => return Err(closed("reading the printer prices")(error).into()),
        // Without quota rules, prices that cannot be read must not stop printing.
        Err(error) => {
            tracing::error!(%error, "reading the printer prices failed");
            (Prices::default(), false)
        }
    };
    let counts = ledger::counts(
        printed,
        options.copies,
        options.sides != Sides::OneSided,
        options.color_mode == ColorMode::Color,
        prices,
    );
    let entry = NewEntry {
        user_id: user.id,
        username: user.username.clone(),
        printer: printer.name.clone(),
        document_name: job_name(&file_name),
        document_format: format.mime().to_owned(),
        document_pages,
        copies: options.copies,
        sides: options.sides.keyword().to_owned(),
        color_mode: options.color_mode.keyword().to_owned(),
        counts,
    };
    // Written before CUPS is asked, so a job cannot print without being counted.
    let prepared = Prepared {
        enforced,
        uncounted,
        priced,
    };
    let (entry_id, over) = enter(state, user, entry, prepared, held).await?;
    let request = PrintRequest {
        printer: printer.name,
        document,
        format,
        job_name: job_name(&file_name),
        user: user.username.clone(),
        options,
    };
    Ok((request, entry_id, over))
}

/// What a job was prepared under, before its write transaction.
struct Prepared {
    /// Whether a rule was enabled when the job was prepared.
    enforced: bool,
    /// Why the pages are unknown, if they are.
    uncounted: Option<Uncounted>,
    /// Whether the prices were read; without them the cost is 0.
    priced: bool,
}

/// Enters the job in the ledger, checked against the quota rules, in one write transaction: a
/// concurrent job of the same user or pool waits for this one to be counted. Returns the entry's
/// id, if it could be written, and the soft rules the job goes over.
///
/// Whether the rules are enforced is decided on the rules read inside the transaction, so a rule
/// enabled while the job was prepared applies to it. With a rule enabled, any failure refuses
/// the job. Without one, a ledger that cannot be written does not stop printing, except for a
/// held job: it is marked released in this transaction ([`held::mark_released`]), so any failure
/// refuses it. A failure before the rules are read again is judged by the rules the job was
/// prepared under.
async fn enter(
    state: &AppState,
    user: &User,
    entry: NewEntry,
    prepared: Prepared,
    held: Option<&HeldClaim>,
) -> Result<(Option<i64>, Vec<i64>), Rejected> {
    let unchecked = |error: recto_domain::Error| {
        if prepared.enforced || held.is_some() {
            Err(closed("checking the print quotas")(error).into())
        } else {
            tracing::error!(%error, "writing the ledger entry failed");
            Ok((None, Vec::new()))
        }
    };
    let transaction = match begin_write(&state.database).await {
        Ok(transaction) => transaction,
        Err(error) => return unchecked(error.into()),
    };
    // The user first, before the rules are read: on PostgreSQL, a job that waits here for a
    // racing one reads the rules and the usage only once that one is committed.
    if let Err(error) = quotas::lock_subjects(&transaction, user.id, &[]).await {
        return unchecked(error);
    }
    let rules = match quotas::rules(&transaction).await {
        Ok(rules) => rules,
        Err(error) => return unchecked(error),
    };
    if !rules.iter().any(|rule| rule.enabled) {
        let entered = async {
            let now = chrono::Utc::now();
            let id = ledger::insert(&transaction, entry, now).await?;
            if let Some(held) = held {
                held::mark_released(&transaction, held.id, held.claim, id, now).await?;
            }
            transaction.commit().await?;
            Ok::<_, recto_domain::Error>(id)
        };
        return match entered.await {
            Ok(id) => Ok((Some(id), Vec::new())),
            Err(error) if held.is_some() => Err(lost_or_closed(error)),
            Err(error) => {
                tracing::error!(%error, "writing the ledger entry failed");
                Ok((None, Vec::new()))
            }
        };
    }
    let groups = access::group_ids_of(&transaction, user.id)
        .await
        .map_err(closed("reading the groups of the user"))?;
    let applicable: Vec<Rule> = quotas::applicable(&rules, user.id, &groups, &entry.printer);
    // Only the jobs metric does without the pages; the others cannot be checked blind.
    if let Some(uncounted) = prepared.uncounted
        && applicable.iter().any(|rule| rule.metric != Metric::Jobs)
    {
        return Err(match uncounted {
            Uncounted::Unreadable => Refusal::Uncountable,
            Uncounted::Busy => Refusal::CountingBusy,
        }
        .into());
    }
    // Prices that could not be read made the job free: not to be checked as such.
    if !prepared.priced {
        return Err(Refusal::Unavailable.into());
    }
    let failed = closed("checking the print quotas");
    let checked = async {
        quotas::lock_subjects(&transaction, user.id, &quotas::pool_groups(&applicable)).await?;
        let now = chrono::Utc::now();
        let verdict = quotas::check(
            &transaction,
            user.id,
            &groups,
            &entry.printer,
            &entry.counts,
            now,
            state.timezone,
        )
        .await?;
        let warnings = match verdict {
            Verdict::Refused { standing } => return Ok(Err(standing.rule.id)),
            Verdict::Allowed { warnings } => warnings,
        };
        let id = ledger::insert(&transaction, entry, now).await?;
        if let Some(held) = held {
            held::mark_released(&transaction, held.id, held.claim, id, now).await?;
        }
        Ok::<_, recto_domain::Error>(Ok((id, warnings)))
    }
    .await;
    match checked {
        Ok(Ok((id, warnings))) => {
            transaction
                .commit()
                .await
                .map_err(|error| failed(error.into()))?;
            Ok((
                Some(id),
                warnings.iter().map(|standing| standing.rule.id).collect(),
            ))
        }
        // Dropping the transaction rolls it back.
        Ok(Err(rule)) => Err(Rejected::Quota(rule)),
        Err(error) if held.is_some() => Err(lost_or_closed(error)),
        Err(error) => Err(failed(error).into()),
    }
}

/// Why a held job's ledger transaction failed: the claim was lost (the sweep returned the job
/// and another release took it), or the database failed. Either way nothing was written.
fn lost_or_closed(error: recto_domain::Error) -> Rejected {
    match error {
        recto_domain::Error::NotFound => {
            tracing::warn!("a held job's release lost its claim");
            Refusal::HeldGone.into()
        }
        error => closed("writing the ledger entry of a held job")(error).into(),
    }
}

/// `POST /jobs/{id}/cancel`: answers `303 /jobs`, or `303 /jobs?error=<code>`. By default anyone
/// signed in may cancel any job (the household trust model); with private jobs, only the
/// jobs the user may see. CUPS is asked in the user's name when it hides the job's owner.
pub async fn cancel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Redirect {
    let canceled = match id.parse::<i32>() {
        Ok(id) => match may_cancel(&state, &user, id).await {
            Ok(()) => state
                .print
                .cancel(id, &user.username)
                .await
                .map_err(refusal),
            Err(refusal) => Err(refusal),
        },
        Err(_) => Err(Refusal::JobNotFound),
    };
    match canceled {
        Ok(()) => {
            tracing::info!(user = user.username, job = id, "job canceled");
            Redirect::to("/jobs")
        }
        Err(refusal) => Redirect::to(&format!("/jobs?error={}", refusal.code())),
    }
}

/// Applies the job privacy setting. A job the user may not see is reported as missing, as it is
/// in the list. Whoever sees every job needs no lookup, so those cases never touch the database.
/// Ownership that cannot be read refuses: nobody cancels on a guess.
async fn may_cancel(state: &AppState, user: &User, id: i32) -> Result<(), Refusal> {
    if user.is_administrator || state.job_visibility == JobVisibility::All {
        return Ok(());
    }
    let jobs = state.print.jobs().await.map_err(refusal)?;
    let job = jobs
        .iter()
        .find(|job| job.id == id)
        .ok_or(Refusal::JobNotFound)?;
    let records = jobs::for_cups_ids(&state.database, &[id])
        .await
        .map_err(|error| {
            tracing::error!(%error, job = id, "reading the job records failed");
            Refusal::Unavailable
        })?;
    let record = jobs::matching(&records, job.id, job.created);
    if visible(record, user, state.job_visibility) {
        Ok(())
    } else {
        Err(Refusal::JobNotFound)
    }
}

fn refusal(error: PrintError) -> Refusal {
    tracing::warn!(%error, "the printing service refused");
    match error {
        PrintError::Unavailable(_) => Refusal::Unavailable,
        PrintError::Unconfirmed(_) => Refusal::Unconfirmed,
        PrintError::PrinterNotFound(_) => Refusal::PrinterNotFound,
        PrintError::JobNotFound(_) => Refusal::JobNotFound,
        PrintError::Refused(_) => Refusal::Refused,
        PrintError::InvalidPageRanges(_) => Refusal::InvalidPageRanges,
    }
}

#[derive(Default)]
struct Form {
    /// The file name the browser sent, and the content.
    file: Option<(String, Bytes)>,
    printer: Option<String>,
    /// The options as typed; the printer is in `printer` until it is known to be there.
    options: Choices,
}

impl Form {
    async fn read(mut multipart: Multipart, max_upload_size: usize) -> Result<Self, Refusal> {
        let mut form = Form::default();
        while let Some(field) = multipart.next_field().await.map_err(unreadable)? {
            let name = field.name().unwrap_or_default().to_owned();
            if name == "file" {
                let file_name = field.file_name().unwrap_or_default().to_owned();
                let bytes = field.bytes().await.map_err(unreadable)?;
                if bytes.len() > max_upload_size {
                    return Err(Refusal::TooLarge);
                }
                // A file input left empty still sends a part, without a name or content.
                if !(file_name.is_empty() && bytes.is_empty()) {
                    form.file = Some((file_name, bytes));
                }
                continue;
            }
            let slot = match name.as_str() {
                "printer" => &mut form.printer,
                "copies" => &mut form.options.copies,
                "sides" => &mut form.options.sides,
                "color_mode" => &mut form.options.color_mode,
                "page_ranges" => &mut form.options.page_ranges,
                _ => continue,
            };
            let value = field.text().await.map_err(unreadable)?;
            // An empty field means the same as a missing one: the printer's default.
            *slot = Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        }
        Ok(form)
    }
}

fn unreadable(error: MultipartError) -> Refusal {
    if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
        Refusal::TooLarge
    } else {
        tracing::warn!(%error, "unreadable print form");
        Refusal::InvalidForm
    }
}

/// The options of the form, refused unless the printer supports them. Single-sided printing is
/// always possible; a printer that lists no color modes is not second-guessed.
fn options(
    fields: &Choices,
    capabilities: &Capabilities,
    format: DocumentFormat,
) -> Result<PrintOptions, Refusal> {
    let copies = match &fields.copies {
        None => 1,
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|copies| (1..=capabilities.max_copies.max(1)).contains(copies))
            .ok_or(Refusal::InvalidCopies)?,
    };
    let sides = match &fields.sides {
        None => Sides::OneSided,
        Some(keyword) => Sides::from_keyword(keyword)
            .filter(|sides| *sides == Sides::OneSided || capabilities.sides.contains(sides))
            .ok_or(Refusal::UnsupportedSides)?,
    };
    let color_mode = match &fields.color_mode {
        None if capabilities.color_modes.contains(&ColorMode::Color) => ColorMode::Color,
        None => ColorMode::Monochrome,
        Some(keyword) => ColorMode::from_keyword(keyword)
            .filter(|mode| {
                capabilities.color_modes.is_empty() || capabilities.color_modes.contains(mode)
            })
            .ok_or(Refusal::UnsupportedColor)?,
    };
    // Images are one page; a range for them is meaningless and not even checked.
    let page_ranges = match (&fields.page_ranges, format) {
        (Some(text), DocumentFormat::Pdf) => Some(
            text.parse::<PageRanges>()
                .map_err(|_| Refusal::InvalidPageRanges)?,
        ),
        _ => None,
    };
    Ok(PrintOptions {
        copies,
        sides,
        color_mode,
        page_ranges,
    })
}

/// The job name CUPS shows: the uploaded file's own name, without any folder some browsers
/// include, cut to what IPP accepts.
pub(crate) fn job_name(file_name: &str) -> String {
    let name = file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    let mut end = name.len().min(MAX_JOB_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    match &name[..end] {
        "" => "document".to_owned(),
        name => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use recto_printing::{Printer, PrinterState, testing::MockPrintService};
    use sea_orm::{EntityTrait, PaginatorTrait};

    use super::*;

    fn capabilities() -> Capabilities {
        Capabilities {
            max_copies: 5,
            sides: vec![Sides::OneSided],
            color_modes: vec![ColorMode::Monochrome],
        }
    }

    #[test]
    fn defaults_follow_the_printer() {
        let defaults = options(&Choices::default(), &capabilities(), DocumentFormat::Pdf).unwrap();
        assert_eq!(defaults.color_mode, ColorMode::Monochrome);
        assert_eq!(defaults.sides, Sides::OneSided);
        assert_eq!(defaults.copies, 1);
        let unknown = Capabilities {
            max_copies: 0,
            sides: vec![],
            color_modes: vec![],
        };
        let fields = Choices {
            copies: Some("1".into()),
            color_mode: Some("color".into()),
            sides: Some("one-sided".into()),
            ..Choices::default()
        };
        assert!(options(&fields, &unknown, DocumentFormat::Pdf).is_ok());
    }

    /// The state of the print path over a fresh database, mock CUPS with an `office` printer,
    /// and held documents under `dir`.
    async fn app(dir: &std::path::Path) -> (AppState, std::sync::Arc<MockPrintService>) {
        use std::sync::Arc;
        let print = Arc::new(MockPrintService::new().with_printers(vec![Printer {
            name: "office".into(),
            description: None,
            location: None,
            make_and_model: None,
            is_default: true,
            state: PrinterState::Idle,
            reasons: vec![],
            accepting_jobs: true,
            supplies: vec![],
            capabilities: capabilities(),
        }]));
        let state = AppState {
            print: print.clone(),
            printer_admin: None,
            scan: Arc::new(recto_scanning::MockScannerService::default()),
            results: Arc::new(recto_scanning::ResultStore::new(
                dir.join("results"),
                std::time::Duration::from_secs(60),
            )),
            output_dir: None,
            max_upload_size: 1024 * 1024,
            leptos_options: leptos::config::LeptosOptions::builder()
                .output_name("recto")
                .build(),
            database: recto_domain::testing::database().await,
            branding: Arc::new(crate::branding::default_branding()),
            job_visibility: JobVisibility::All,
            currency: recto_app::state::Currency::new("EUR"),
            timezone: chrono_tz::UTC,
            page_counter: Default::default(),
            raster: Default::default(),
            os_integration: true,
            held: Arc::new(held::files::HeldFiles::new(&dir.join("data")).unwrap()),
            held_lifetime: chrono::TimeDelta::hours(24),
        };
        (state, print)
    }

    #[tokio::test]
    async fn a_lost_claim_rolls_the_ledger_entry_back() {
        let dir = recto_domain::testing::temporary_directory();
        let (state, print) = app(dir.path()).await;
        let alice = recto_domain::users::create_local(
            &state.database,
            recto_domain::users::NewLocalUser {
                username: "alice".into(),
                display_name: String::new(),
                email: None,
                password: "correct horse battery staple".into(),
                is_administrator: false,
            },
            &recto_domain::passwords::PasswordPolicy { minimum_length: 12 },
        )
        .await
        .unwrap();
        let user = User {
            id: alice.id,
            username: alice.username,
            name: alice.display_name,
            is_administrator: false,
            has_password: true,
        };
        let document = recto_printing::testing::sample_pdf(1);
        let file = state.held.write(&document).unwrap();
        let id = held::receive(
            &state.database,
            held::NewHeld {
                user_id: user.id,
                device_password_id: None,
                name: "minutes.pdf".into(),
                requested: "{}".into(),
                stored: held::Stored {
                    file,
                    stored_format: "application/pdf".into(),
                    received_format: None,
                    document_pages: Some(1),
                    size: document.len() as i64,
                },
                now: chrono::Utc::now(),
                lifetime: chrono::TimeDelta::hours(24),
            },
            i64::MAX,
        )
        .await
        .unwrap();
        // A release claims the job, stalls, and the sweep gives it back; another release
        // claims it again.
        let stale = held::claim(&state.database, user.id, id, chrono::Utc::now())
            .await
            .unwrap()
            .unwrap();
        held::unclaim(&state.database, id, stale.claim, None, chrono::Utc::now())
            .await
            .unwrap();
        let current = held::claim(&state.database, user.id, id, chrono::Utc::now())
            .await
            .unwrap()
            .unwrap();

        // The stale release reaches the ledger: its entry must not stay, with quota rules or
        // without.
        for rules in [false, true] {
            if rules {
                quotas::create(
                    &state.database,
                    quotas::NewRule {
                        name: "Monthly pages".into(),
                        enabled: true,
                        subject: quotas::Subject::Everyone,
                        printers: vec![],
                        metric: Metric::Pages,
                        period: quotas::Period::Month,
                        limit: 100,
                        enforcement: quotas::Enforcement::Hard,
                    },
                )
                .await
                .unwrap();
            }
            let printed = print_document(
                &state,
                &user,
                Document {
                    name: "minutes.pdf".into(),
                    bytes: Bytes::from(document.clone()),
                    format: DocumentFormat::Pdf,
                },
                &Choices {
                    printer: "office".into(),
                    ..Choices::default()
                },
                Some(HeldClaim {
                    id,
                    claim: stale.claim,
                }),
            )
            .await;
            assert!(
                matches!(printed, Err(Rejected::Refusal(Refusal::HeldGone))),
                "rules: {rules}"
            );
            assert!(print.submitted().is_empty());
            let entries = recto_entity::print_jobs::Entity::find()
                .count(&state.database)
                .await
                .unwrap();
            assert_eq!(entries, 0, "rules: {rules}");
            let job = held::get(&state.database, user.id, id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(job.state, held::HeldState::Releasing);
            assert_eq!(job.claim, current.claim);
        }
    }

    #[test]
    fn job_names_lose_folders_and_excess() {
        assert_eq!(job_name("C:\\Users\\a\\report.pdf"), "report.pdf");
        assert_eq!(job_name("a/b/photo.png"), "photo.png");
        assert_eq!(job_name("  "), "document");
        let long = "é".repeat(200);
        let cut = job_name(&long);
        assert!(cut.len() <= MAX_JOB_NAME && cut.chars().all(|c| c == 'é'));
    }
}
