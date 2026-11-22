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
    jobs,
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
struct Submitted {
    job: i32,
    over: Vec<i64>,
}

/// Why a job was not printed.
enum Rejected {
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

async fn submit_form(
    state: &AppState,
    user: &User,
    multipart: Multipart,
) -> Result<Submitted, Rejected> {
    let form = Form::read(multipart, state.max_upload_size).await?;
    let (file_name, document) = form.file.ok_or(Refusal::NoFile)?;
    if document.is_empty() {
        return Err(Refusal::EmptyFile.into());
    }
    // From the content, never the name or declared type: a renamed document stops here.
    let format = DocumentFormat::detect(&document).ok_or(Refusal::UnsupportedFormat)?;
    let printer_name = form.printer.ok_or(Refusal::InvalidForm)?;
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
    if !usable.allows(&printer_name) {
        return Err(Refusal::PrinterNotFound.into());
    }
    // Rules that cannot be read might be there: refused, like access rules.
    let rules = quotas::rules(&state.database)
        .await
        .map_err(closed("reading the quota rules"))?;
    let enforced = rules.iter().any(|rule| rule.enabled);
    let printer = state.print.printer(&printer_name).await.map_err(refusal)?;
    // CUPS ignores the case of printer names, so "LAB" reaches the printer "lab": its rule is
    // checked again under the name CUPS answered with.
    if !usable.allows(&printer.name) {
        return Err(Refusal::PrinterNotFound.into());
    }
    if !printer.accepting_jobs {
        return Err(Refusal::NotAccepting.into());
    }
    let options = options(&form.options, &printer.capabilities, format)?;
    let document_pages = match format {
        // Bounded in concurrency and time, and not copied: see `PageCounter`.
        DocumentFormat::Pdf => state.page_counter.count(document.clone()).await,
        DocumentFormat::Jpeg | DocumentFormat::Png => Ok(1),
    };
    let groups = if enforced {
        access::group_ids_of(&state.database, user.id)
            .await
            .map_err(closed("reading the groups of the user"))?
    } else {
        Vec::new()
    };
    let applicable = quotas::applicable(&rules, user.id, &groups, &printer.name);
    // Only the jobs metric does without the pages; the others cannot be checked blind.
    if let Err(uncounted) = document_pages
        && applicable.iter().any(|rule| rule.metric != Metric::Jobs)
    {
        return Err(match uncounted {
            Uncounted::Unreadable => Refusal::Uncountable,
            Uncounted::Busy => Refusal::CountingBusy,
        }
        .into());
    }
    let document_pages = document_pages.ok();
    let printed = document_pages.map(|pages| printed_pages(pages, options.page_ranges.as_ref()));
    // The cost is fixed now: a later price change leaves this job's cost alone.
    let prices = match prices::get(&state.database, &printer.name).await {
        Ok(prices) => prices,
        Err(error) if enforced => return Err(closed("reading the printer prices")(error).into()),
        // Without quota rules, prices that cannot be read must not stop printing.
        Err(error) => {
            tracing::error!(%error, "reading the printer prices failed");
            Prices::default()
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
    let (entry_id, over) = if enforced {
        let (entry_id, over) = enter_checked(state, user, &groups, entry).await?;
        (Some(entry_id), over)
    } else {
        // Without rules, a ledger that cannot be written must not stop printing.
        match ledger::insert(&state.database, entry, chrono::Utc::now()).await {
            Ok(id) => (Some(id), Vec::new()),
            Err(error) => {
                tracing::error!(%error, "writing the ledger entry failed");
                (None, Vec::new())
            }
        }
    };
    let request = PrintRequest {
        printer: printer.name,
        document,
        format,
        job_name: job_name(&file_name),
        user: user.username.clone(),
        options,
    };
    let id = match state.print.submit(request).await {
        Ok(id) => id,
        Err(error) => {
            // Nothing printed, so the job must not count.
            if let Some(entry_id) = entry_id
                && let Err(error) = ledger::refund(&state.database, entry_id).await
            {
                tracing::error!(%error, "refunding the ledger entry failed");
            }
            return Err(refusal(error).into());
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

/// Checks the job against the quota rules and enters it in the ledger, in one write
/// transaction: a concurrent job of the same user or pool waits for this one to be counted.
/// Returns the entry's id and the soft rules the job goes over.
async fn enter_checked(
    state: &AppState,
    user: &User,
    groups: &[i64],
    entry: NewEntry,
) -> Result<(i64, Vec<i64>), Rejected> {
    let failed = closed("checking the print quotas");
    let transaction = begin_write(&state.database)
        .await
        .map_err(|error| failed(error.into()))?;
    let checked = async {
        // Read again inside the transaction, so the pools locked are those checked.
        let rules = quotas::rules(&transaction).await?;
        let applicable: Vec<Rule> = quotas::applicable(&rules, user.id, groups, &entry.printer);
        quotas::lock_subjects(&transaction, user.id, &quotas::pool_groups(&applicable)).await?;
        let now = chrono::Utc::now();
        let verdict = quotas::check(
            &transaction,
            user.id,
            groups,
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
                id,
                warnings.iter().map(|standing| standing.rule.id).collect(),
            ))
        }
        // Dropping the transaction rolls it back.
        Ok(Err(rule)) => Err(Rejected::Quota(rule)),
        Err(error) => Err(failed(error).into()),
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
        PrintError::PrinterNotFound(_) => Refusal::PrinterNotFound,
        PrintError::JobNotFound(_) => Refusal::JobNotFound,
        PrintError::Refused(_) => Refusal::Refused,
        PrintError::InvalidPageRanges(_) => Refusal::InvalidPageRanges,
    }
}

/// The options as typed in the form, before they are checked against the printer.
#[derive(Default)]
struct OptionFields {
    copies: Option<String>,
    sides: Option<String>,
    color_mode: Option<String>,
    page_ranges: Option<String>,
}

#[derive(Default)]
struct Form {
    /// The file name the browser sent, and the content.
    file: Option<(String, Bytes)>,
    printer: Option<String>,
    options: OptionFields,
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
    fields: &OptionFields,
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
fn job_name(file_name: &str) -> String {
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
        let defaults = options(
            &OptionFields::default(),
            &capabilities(),
            DocumentFormat::Pdf,
        )
        .unwrap();
        assert_eq!(defaults.color_mode, ColorMode::Monochrome);
        assert_eq!(defaults.sides, Sides::OneSided);
        assert_eq!(defaults.copies, 1);
        let unknown = Capabilities {
            max_copies: 0,
            sides: vec![],
            color_modes: vec![],
        };
        let fields = OptionFields {
            copies: Some("1".into()),
            color_mode: Some("color".into()),
            sides: Some("one-sided".into()),
            ..OptionFields::default()
        };
        assert!(options(&fields, &unknown, DocumentFormat::Pdf).is_ok());
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
