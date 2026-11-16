//! The print and cancel forms. Both are plain form posts answered with a redirect, so they work
//! without JavaScript and a reload never sends a document twice. A refusal goes back to the
//! page with its code (`/?error=too_large`, see [`Refusal`]).

use axum::{
    Extension,
    body::Bytes,
    extract::{Multipart, Path, State, multipart::MultipartError},
    http::StatusCode,
    response::Redirect,
};
use recto_app::{
    refusal::Refusal,
    state::{AppState, User},
};
use recto_domain::jobs::{self, NewJob};
use recto_printing::{
    Capabilities, ColorMode, DocumentFormat, PageRanges, PrintError, PrintOptions, PrintRequest,
    Sides,
};

/// Room for the form fields and multipart headers next to the largest accepted file, so a file
/// just under the limit is not refused for its envelope.
pub const FORM_SLACK: usize = 64 * 1024;

/// IPP `name` values are at most 255 octets.
const MAX_JOB_NAME: usize = 255;

/// `POST /print`: a multipart form with `file`, `printer`, `copies`, `sides`, `color_mode` and
/// `page_ranges`. Answers `303 /jobs?submitted=<id>`, or `303 /?error=<code>`.
pub async fn submit(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    multipart: Result<Multipart, axum::extract::multipart::MultipartRejection>,
) -> Redirect {
    let submitted = match multipart {
        Ok(multipart) => submit_form(&state, &user, multipart).await,
        Err(rejection) => {
            tracing::warn!(%rejection, "unreadable print form");
            Err(Refusal::InvalidForm)
        }
    };
    match submitted {
        Ok(job) => {
            tracing::info!(user = user.username, job, "document sent to the printer");
            Redirect::to(&format!("/jobs?submitted={job}"))
        }
        Err(refusal) => Redirect::to(&format!("/?error={}", refusal.code())),
    }
}

async fn submit_form(state: &AppState, user: &User, multipart: Multipart) -> Result<i32, Refusal> {
    let form = Form::read(multipart, state.max_upload_size).await?;
    let (file_name, document) = form.file.ok_or(Refusal::NoFile)?;
    if document.is_empty() {
        return Err(Refusal::EmptyFile);
    }
    // From the content, never the name or declared type: a renamed document stops here.
    let format = DocumentFormat::detect(&document).ok_or(Refusal::UnsupportedFormat)?;
    let printer_name = form.printer.ok_or(Refusal::InvalidForm)?;
    let printer = state.print.printer(&printer_name).await.map_err(refusal)?;
    if !printer.accepting_jobs {
        return Err(Refusal::NotAccepting);
    }
    let options = options(&form.options, &printer.capabilities, format)?;
    let request = PrintRequest {
        printer: printer.name,
        document,
        format,
        job_name: job_name(&file_name),
        user: user.username.clone(),
        options,
    };
    let printer = request.printer.clone();
    let document_name = request.job_name.clone();
    let id = state.print.submit(request).await.map_err(refusal)?;
    // The job is printing already: failing to record it must not tell the user it failed.
    let recorded = jobs::record(
        &state.database,
        NewJob {
            user_id: user.id,
            printer,
            cups_job_id: id,
            document_name,
        },
    )
    .await;
    if let Err(error) = recorded {
        tracing::error!(%error, job = id, "recording the job failed");
    }
    Ok(id)
}

/// `POST /jobs/{id}/cancel`: answers `303 /jobs`, or `303 /jobs?error=<code>`. Anyone signed in
/// may cancel any job (the household trust model); CUPS is asked in the user's name when it
/// hides the job's owner.
pub async fn cancel(
    State(state): State<AppState>,
    Extension(user): Extension<User>,
    Path(id): Path<String>,
) -> Redirect {
    let canceled = match id.parse::<i32>() {
        Ok(id) => state
            .print
            .cancel(id, &user.username)
            .await
            .map_err(refusal),
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
