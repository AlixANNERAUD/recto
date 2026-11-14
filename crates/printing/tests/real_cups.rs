//! The CUPS client against a real CUPS server, which wiremock cannot stand in for (Host header
//! checks, job URIs, owner lookup). Ignored, and never run in CI. Start the compose stack, create
//! a queue and pause it with `cupsdisable`, then:
//!
//! `RECTO_CUPS_URL_TEST=http://localhost:631 cargo nextest run -p recto-printing \
//!  --test real_cups --run-ignored only`

use bytes::Bytes;
use recto_printing::{Cups, DocumentFormat, PrintOptions, PrintRequest, PrintService};
use url::Url;

/// The smallest PDF CUPS will accept: one empty page.
const TINY_PDF: &[u8] = b"%PDF-1.4\n\
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\n\
trailer<</Root 1 0 R/Size 4>>\n%%EOF\n";

#[tokio::test]
#[ignore = "needs a real CUPS server with a queue, see the module docs"]
async fn prints_lists_and_cancels_against_real_cups() {
    let base = std::env::var("RECTO_CUPS_URL_TEST").expect("set RECTO_CUPS_URL_TEST");
    let cups = Cups::new(Url::parse(&base).unwrap());

    let printers = cups.printers().await.expect("lists printers");
    println!("printers: {printers:?}");
    let printer = printers.first().expect("create a queue first").name.clone();
    cups.printer(&printer).await.expect("reads one printer");

    let id = cups
        .submit(PrintRequest {
            printer: printer.clone(),
            document: Bytes::from_static(TINY_PDF),
            format: DocumentFormat::Pdf,
            job_name: "recto real cups test".into(),
            user: "recto-test".into(),
            options: PrintOptions::default(),
        })
        .await
        .expect("submits the job");
    println!("job id: {id}");

    let jobs = cups.jobs().await.expect("lists jobs");
    println!("jobs: {jobs:?}");
    let job = jobs.iter().find(|j| j.id == id).expect("the job is listed");
    assert_eq!(job.printer, printer);

    // Create the queue disabled (`cupsdisable`) so the job is still pending and cancellable.
    cups.cancel(id, "recto-test")
        .await
        .expect("cancels the job");
}
