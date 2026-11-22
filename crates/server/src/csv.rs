//! A small CSV writer for the usage exports: RFC 4180 (comma separated, CRLF line ends, quotes
//! doubled), after a UTF-8 byte order mark so that Excel reads accents.
//!
//! Spreadsheets run a cell that starts with `=`, `+`, `-` or `@` as a formula, so text cells
//! starting with one of those, a tab or a carriage return get a leading `'`: a document named
//! `=HYPERLINK(...)` stays text. Numbers are written as they are, never through [`text`].

/// The byte order mark the files start with.
pub(crate) const BOM: &str = "\u{feff}";

/// A text cell: neutralized when a spreadsheet would read it as a formula, then quoted when it
/// holds a comma, a quote or a line break.
pub(crate) fn text(value: &str) -> String {
    let formula = value.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let value = if formula {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

/// A line of cells, comma separated and ending with CRLF.
pub(crate) fn line(cells: &[String]) -> String {
    let mut line = cells.join(",");
    line.push_str("\r\n");
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_are_quoted_and_neutralized() {
        assert_eq!(text("report.pdf"), "report.pdf");
        assert_eq!(text("a,b"), "\"a,b\"");
        assert_eq!(text("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(text("two\nlines"), "\"two\nlines\"");
        assert_eq!(text("two\r\nlines"), "\"two\r\nlines\"");
        assert_eq!(text("=SUM(A1)"), "'=SUM(A1)");
        for (value, expected) in [("+1", "'+1"), ("-2", "'-2"), ("@x", "'@x"), ("\tx", "'\tx")] {
            assert_eq!(text(value), expected);
        }
        assert_eq!(text("\rx"), "\"'\rx\"");
        assert_eq!(text("=a,b"), "\"'=a,b\"");
        assert_eq!(text("=HYPERLINK(\"x\")"), "\"'=HYPERLINK(\"\"x\"\")\"");
        assert_eq!(text(""), "");
        assert_eq!(text("élève"), "élève");
        assert_eq!(line(&["1".into(), "x".into()]), "1,x\r\n");
        assert_eq!(line(&[]), "\r\n");
    }
}
