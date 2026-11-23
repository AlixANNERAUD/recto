//! How many pages a document prints, for the ledger.
//!
//! Documents come from users, so reading one is bounded: [`PageCounter`] limits how many are
//! read at once and for how long, and the reader drops what the page count does not need.

use std::time::Duration;

use bytes::Bytes;
use lopdf::{Document, LoadOptions, Object, ObjectId};

use crate::PageRanges;
use crate::bounded::{Bounded, Late};

/// The most one object or cross-reference stream may inflate to while a PDF is read. Real ones
/// are a few hundred kilobytes; a compression bomb stops here instead of at lopdf's 256 MiB.
const MAX_INFLATED_STREAM: usize = 16 * 1024 * 1024;

/// The deepest page tree counted, its root and pages included.
pub const MAX_DEPTH: usize = 64;

/// The most pages a document may have and still be counted.
pub const MAX_PAGES: u32 = 100_000;

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

/// The page count of a PDF, or `None` when it cannot be counted. The count is the document's own,
/// before any page range.
///
/// The tree is walked here rather than through lopdf's page list, which drops what it cannot
/// place and visits each page once: CUPS prints a page as often as the tree lists it, so a page
/// listed 1000 times counts 1000 times. The trailer's `/Count` is neither trusted nor used. A
/// document is uncountable when it cannot be parsed (damaged, or encrypted so that its page tree
/// cannot be read), or when its tree has a cycle, is deeper than [`MAX_DEPTH`], has a node that
/// is neither `/Pages` nor `/Page`, more than [`MAX_PAGES`] pages, or none.
pub fn pdf_pages(document: &[u8]) -> Option<u32> {
    let options = LoadOptions {
        filter: Some(page_tree_only),
        max_decompressed_size: Some(MAX_INFLATED_STREAM),
        ..LoadOptions::default()
    };
    let document = Document::load_mem_with_options(document, options).ok()?;
    let root = document
        .catalog()
        .ok()?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .ok()?;
    let mut walk = Walk {
        document: &document,
        path: Vec::new(),
        visits: 0,
        pages: 0,
    };
    walk.node(root)?;
    (walk.pages > 0).then_some(walk.pages)
}

/// The most nodes visited in a page tree. A tree that lists its subtrees many times could make
/// the walk exponential without pages to stop it: this bounds the time it takes.
const MAX_VISITS: usize = 1_000_000;

/// A walk of a page tree, counting every page each time the tree lists it.
struct Walk<'a> {
    document: &'a Document,
    /// The nodes from the root to the current one, to find cycles.
    path: Vec<ObjectId>,
    visits: usize,
    pages: u32,
}

impl Walk<'_> {
    /// Counts the pages under `id`; `None` as soon as the tree is uncountable.
    fn node(&mut self, id: ObjectId) -> Option<()> {
        self.visits += 1;
        if self.visits > MAX_VISITS || self.path.len() >= MAX_DEPTH || self.path.contains(&id) {
            return None;
        }
        let node = self.document.get_dictionary(id).ok()?;
        match node.get(b"Type").and_then(Object::as_name).ok()? {
            b"Page" => {
                self.pages += 1;
                (self.pages <= MAX_PAGES).then_some(())
            }
            b"Pages" => {
                let kids = node.get(b"Kids").ok()?;
                let (_, kids) = self.document.dereference(kids).ok()?;
                self.path.push(id);
                for kid in kids.as_array().ok()? {
                    self.node(kid.as_reference().ok()?)?;
                }
                self.path.pop();
                Some(())
            }
            _ => None,
        }
    }
}

/// Reads PDFs for their page count with bounded concurrency and time. Reading runs on blocking
/// threads, which cannot be cancelled: a read that times out keeps its permit until it ends, so
/// the number of threads and the memory they hold stay bounded whatever is uploaded.
#[derive(Clone, Debug)]
pub struct PageCounter {
    bounded: Bounded,
}

impl PageCounter {
    /// At most `concurrency` PDFs read at once; a count not known after `timeout`, waiting for
    /// a place included, is unknown.
    pub fn new(concurrency: usize, timeout: Duration) -> Self {
        Self {
            bounded: Bounded::new(concurrency, timeout),
        }
    }

    /// [`pdf_pages`], or `None` when it takes too long. The document is shared, not copied.
    pub async fn pdf_pages(&self, document: Bytes) -> Option<u32> {
        self.count(document).await.ok()
    }

    /// The page count of a PDF, or why it is unknown. Waiting for a place counts in the time
    /// limit: when other documents hold every place until then, the count is [`Uncounted::Busy`],
    /// which a retry may fix; a document that cannot be read in the time left is
    /// [`Uncounted::Unreadable`], like a damaged one.
    pub async fn count(&self, document: Bytes) -> Result<u32, Uncounted> {
        match self.bounded.run(move |_| pdf_pages(&document)).await {
            Ok(Some(pages)) => Ok(pages),
            Ok(None) | Err(Late::TimedOut) => Err(Uncounted::Unreadable),
            Err(Late::Busy) => Err(Uncounted::Busy),
        }
    }
}

/// Why a PDF's page count is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uncounted {
    /// Damaged, encrypted, or too slow to read.
    Unreadable,
    /// Other documents were being counted the whole time.
    Busy,
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
    use lopdf::dictionary;

    #[test]
    fn pdf_pages_are_counted() {
        assert_eq!(pdf_pages(&sample_pdf(3)), Some(3));
        assert_eq!(pdf_pages(b"%PDF-1.7 garbage"), None);
    }

    #[tokio::test]
    async fn a_busy_counter_says_so() {
        let counter = PageCounter::new(1, Duration::from_millis(50));
        assert_eq!(counter.count(Bytes::from(sample_pdf(2))).await, Ok(2));
        assert_eq!(
            counter.count(Bytes::from_static(b"%PDF-1.7 garbage")).await,
            Err(Uncounted::Unreadable)
        );
        // Another document holds the only place until the time is up.
        let held = counter
            .bounded
            .permits
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        assert_eq!(
            counter.count(Bytes::from(sample_pdf(2))).await,
            Err(Uncounted::Busy)
        );
        drop(held);
        assert_eq!(counter.pdf_pages(Bytes::from(sample_pdf(2))).await, Some(2));
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
    fn deep_pdf(depth: usize) -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut document = Document::with_version("1.5");
        let ids: Vec<_> = (0..depth).map(|_| document.new_object_id()).collect();
        let page =
            document.add_object(dictionary! { "Type" => "Page", "Parent" => ids[depth - 1] });
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
    fn a_cyclic_page_tree_is_uncountable() {
        // The file claims 99999 pages; the tree reaches its root again.
        assert_eq!(pdf_pages(&cyclic_pdf()), None);
    }

    #[test]
    fn a_page_tree_deeper_than_the_limit_is_uncountable() {
        // The chain of nodes, then the page: 64 levels in all.
        assert_eq!(pdf_pages(&deep_pdf(MAX_DEPTH - 1)), Some(1));
        assert_eq!(pdf_pages(&deep_pdf(MAX_DEPTH)), None);
        assert_eq!(pdf_pages(&deep_pdf(5_000)), None);
    }

    /// A PDF whose root node is `Pages` with the kids `tree` adds to the document.
    fn tree_pdf(tree: impl FnOnce(&mut lopdf::Document) -> Vec<lopdf::Object>) -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut document = Document::with_version("1.5");
        let root = document.new_object_id();
        let kids = tree(&mut document);
        document.objects.insert(
            root,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Count" => 1, "Kids" => kids }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => root });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    fn page(document: &mut lopdf::Document) -> lopdf::ObjectId {
        document.add_object(dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        })
    }

    /// A `Pages` node listing `kid` `times` times.
    fn node(document: &mut lopdf::Document, kid: lopdf::ObjectId, times: usize) -> lopdf::ObjectId {
        document.add_object(dictionary! {
            "Type" => "Pages", "Count" => 1, "Kids" => vec![kid.into(); times],
        })
    }

    #[test]
    fn a_page_listed_many_times_counts_each_time() {
        // CUPS prints a page as often as the tree lists it.
        let repeated = tree_pdf(|document| vec![page(document).into(); 1000]);
        assert_eq!(pdf_pages(&repeated), Some(1000));
        // A shared subtree counts each time too.
        let shared = tree_pdf(|document| {
            let page = page(document);
            let pair = node(document, page, 2);
            vec![pair.into(), pair.into(), page.into()]
        });
        assert_eq!(pdf_pages(&shared), Some(5));
    }

    #[test]
    fn more_pages_than_the_limit_are_uncountable() {
        let pages = |each: usize| {
            tree_pdf(|document| {
                let page = page(document);
                let hundred = node(document, page, each);
                vec![hundred.into(); 1000]
            })
        };
        assert_eq!(pdf_pages(&pages(100)), Some(MAX_PAGES));
        assert_eq!(pdf_pages(&pages(101)), None);
    }

    #[test]
    fn a_document_without_pages_is_uncountable() {
        assert_eq!(pdf_pages(&tree_pdf(|_| Vec::new())), None);
    }

    #[test]
    fn a_node_neither_pages_nor_page_is_uncountable() {
        let mistyped = tree_pdf(|document| {
            let page = page(document);
            let odd = document.add_object(dictionary! {
                "Type" => "Folder", "Kids" => vec![page.into()],
            });
            vec![page.into(), odd.into()]
        });
        assert_eq!(pdf_pages(&mistyped), None);
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
        let place = busy.bounded.permits.clone().acquire_owned().await.unwrap();
        assert_eq!(busy.pdf_pages(document.clone()).await, None);
        drop(place);
        assert_eq!(busy.pdf_pages(document).await, Some(3));
    }
}
