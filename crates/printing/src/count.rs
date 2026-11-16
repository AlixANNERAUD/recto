//! How many pages a document prints, for the ledger.
//!
//! Documents come from users, so reading one is bounded: [`PageCounter`] limits how many are
//! read at once and for how long, and the reader drops what the page count does not need.

use std::{sync::Arc, time::Duration};

use bytes::Bytes;
use lopdf::{Document, LoadOptions, Object, ObjectId};
use tokio::sync::Semaphore;

use crate::PageRanges;

/// The most one object or cross-reference stream may inflate to while a PDF is read. Real ones
/// are a few hundred kilobytes; a compression bomb stops here instead of at lopdf's 256 MiB.
const MAX_INFLATED_STREAM: usize = 16 * 1024 * 1024;

/// Keeps what the page tree is made of. Page contents, fonts and images are the bulk of a PDF
/// and never needed: dropping them keeps the memory of a large document small. Object streams
/// are kept for loading, because ordinary objects may live inside them.
fn page_tree_only(id: ObjectId, object: &mut Object) -> Option<(ObjectId, Object)> {
    match object {
        Object::Stream(stream)
            if stream.dict.has_type(b"ObjStm") || stream.dict.has_type(b"XRef") =>
        {
            // lopdf reads the stream itself; this answer is only used for objects inside one.
            Some((id, Object::Null))
        }
        Object::Stream(_) => None,
        other => Some((id, other.clone())),
    }
}

/// The page count of a PDF, or `None` when it cannot be parsed (damaged, or encrypted so that
/// its page tree cannot be read). The count is the document's own, before any page range.
///
/// The trailer's `/Count` is not trusted nor used: the file may lie, and CUPS prints the pages
/// the tree really holds. lopdf walks the tree with its own depth and visit limits, so a cyclic
/// or very deep tree ends.
pub fn pdf_pages(document: &[u8]) -> Option<u32> {
    let options = LoadOptions {
        filter: Some(page_tree_only),
        max_decompressed_size: Some(MAX_INFLATED_STREAM),
        ..LoadOptions::default()
    };
    let document = Document::load_mem_with_options(document, options).ok()?;
    u32::try_from(document.get_pages().len()).ok()
}

/// Reads PDFs for their page count with bounded concurrency and time. Reading runs on blocking
/// threads, which cannot be cancelled: a read that times out keeps its permit until it ends, so
/// the number of threads and the memory they hold stay bounded whatever is uploaded.
#[derive(Clone, Debug)]
pub struct PageCounter {
    permits: Arc<Semaphore>,
    timeout: Duration,
}

impl PageCounter {
    /// At most `concurrency` PDFs read at once; a count not known after `timeout`, waiting for
    /// a place included, is unknown.
    pub fn new(concurrency: usize, timeout: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrency.max(1))),
            timeout,
        }
    }

    /// [`pdf_pages`], or `None` when it takes too long. The document is shared, not copied.
    pub async fn pdf_pages(&self, document: Bytes) -> Option<u32> {
        let counted = async {
            let permit = self.permits.clone().acquire_owned().await.ok()?;
            tokio::task::spawn_blocking(move || {
                let pages = pdf_pages(&document);
                drop(permit);
                pages
            })
            .await
            .ok()
            .flatten()
        };
        tokio::time::timeout(self.timeout, counted)
            .await
            .ok()
            .flatten()
    }
}

impl Default for PageCounter {
    /// Two at a time, ten seconds each.
    fn default() -> Self {
        Self::new(2, Duration::from_secs(10))
    }
}

/// The pages of a document of `document_pages` pages that the ranges select, all of them without
/// ranges. A range beyond the end selects nothing there: CUPS prints only the pages that exist.
pub fn printed_pages(document_pages: u32, ranges: Option<&PageRanges>) -> u32 {
    let Some(ranges) = ranges else {
        return document_pages;
    };
    ranges
        .0
        .iter()
        .filter(|(first, _)| *first <= document_pages)
        .map(|(first, last)| last.min(&document_pages) - first + 1)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PageRanges, testing::sample_pdf};

    #[test]
    fn pdf_pages_are_counted() {
        assert_eq!(pdf_pages(&sample_pdf(3)), Some(3));
        assert_eq!(pdf_pages(b"%PDF-1.7 garbage"), None);
    }

    #[test]
    fn ranges_select_existing_pages() {
        let ranges = |text: &str| text.parse::<PageRanges>().unwrap();
        assert_eq!(printed_pages(10, Some(&ranges("1-3,5"))), 4);
        assert_eq!(printed_pages(10, Some(&ranges("8-20"))), 3);
        assert_eq!(printed_pages(10, Some(&ranges("15-20"))), 0);
        assert_eq!(printed_pages(10, None), 10);
    }

    /// A PDF whose page tree contains itself.
    fn cyclic_pdf() -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Count" => 99_999,
                "Kids" => vec![pages_id.into(), page.into(), pages_id.into()],
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    /// A PDF whose page tree is a chain of `depth` nodes, with one page at the end.
    fn deep_pdf(depth: u32) -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut document = Document::with_version("1.5");
        let ids: Vec<_> = (0..depth).map(|_| document.new_object_id()).collect();
        let page = document
            .add_object(dictionary! { "Type" => "Page", "Parent" => ids[depth as usize - 1] });
        for (index, id) in ids.iter().enumerate() {
            let kid: Object = match ids.get(index + 1) {
                Some(next) => (*next).into(),
                None => page.into(),
            };
            document.objects.insert(
                *id,
                Object::Dictionary(dictionary! {
                    "Type" => "Pages", "Count" => 1, "Kids" => vec![kid],
                }),
            );
        }
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => ids[0] });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_cyclic_page_tree_ends() {
        // The file claims 99999 pages; the tree holds one real page, however often it is
        // reached. Any answer is fine as long as it comes, and is not the claimed count.
        let pages = pdf_pages(&cyclic_pdf());
        assert!(pages.is_none_or(|pages| pages < 100), "{pages:?}");
    }

    #[test]
    fn a_very_deep_page_tree_ends() {
        let pages = pdf_pages(&deep_pdf(5_000));
        assert!(pages.is_none_or(|pages| pages <= 1), "{pages:?}");
    }

    #[test]
    fn page_contents_are_not_needed_to_count() {
        // A page with a content stream still counts, without its content being kept.
        use lopdf::{Document, Object, Stream, dictionary};
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let content = document.add_object(Stream::new(dictionary! {}, vec![b'q'; 1000]));
        let page = document.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => content,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        document.objects.insert(
            pages_id,
            Object::Dictionary(
                dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => vec![page.into()] },
            ),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        assert_eq!(pdf_pages(&bytes), Some(1));
    }

    #[test]
    fn pages_inside_object_streams_are_counted() {
        use lopdf::{Document, SaveOptions};
        let mut document = Document::load_mem(&sample_pdf(4)).unwrap();
        let mut bytes = Vec::new();
        document
            .save_with_options(
                &mut bytes,
                SaveOptions::builder()
                    .use_object_streams(true)
                    .use_xref_streams(true)
                    .build(),
            )
            .unwrap();
        assert_eq!(pdf_pages(&bytes), Some(4));
    }

    #[tokio::test]
    async fn the_counter_counts_and_times_out() {
        let document = Bytes::from(sample_pdf(3));
        assert_eq!(
            PageCounter::default().pdf_pages(document.clone()).await,
            Some(3)
        );
        // Every place taken for longer than the timeout: the count is unknown, like an
        // unreadable document, and the document is not read at all.
        let busy = PageCounter::new(1, Duration::from_millis(50));
        let place = busy.permits.clone().acquire_owned().await.unwrap();
        assert_eq!(busy.pdf_pages(document.clone()).await, None);
        drop(place);
        assert_eq!(busy.pdf_pages(document).await, Some(3));
    }
}
