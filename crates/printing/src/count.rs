//! How many pages a document prints, for the ledger.

use crate::PageRanges;

/// The page count of a PDF, or `None` when it cannot be parsed (damaged, or encrypted so that
/// its page tree cannot be read). The count is the document's own, before any page range.
pub fn pdf_pages(document: &[u8]) -> Option<u32> {
    let document = lopdf::Document::load_mem(document).ok()?;
    u32::try_from(document.get_pages().len()).ok()
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
}
