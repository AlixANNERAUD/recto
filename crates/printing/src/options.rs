//! Print options and their mapping to IPP job template attributes.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::PrintError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sides {
    OneSided,
    TwoSidedLongEdge,
    TwoSidedShortEdge,
}

impl Sides {
    pub fn keyword(self) -> &'static str {
        match self {
            Sides::OneSided => "one-sided",
            Sides::TwoSidedLongEdge => "two-sided-long-edge",
            Sides::TwoSidedShortEdge => "two-sided-short-edge",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<Sides> {
        match keyword {
            "one-sided" => Some(Sides::OneSided),
            "two-sided-long-edge" => Some(Sides::TwoSidedLongEdge),
            "two-sided-short-edge" => Some(Sides::TwoSidedShortEdge),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    Color,
    Monochrome,
}

impl ColorMode {
    pub fn keyword(self) -> &'static str {
        match self {
            ColorMode::Color => "color",
            ColorMode::Monochrome => "monochrome",
        }
    }

    pub fn from_keyword(keyword: &str) -> Option<ColorMode> {
        match keyword {
            "color" => Some(ColorMode::Color),
            "monochrome" => Some(ColorMode::Monochrome),
            _ => None,
        }
    }
}

/// Sorted, merged, inclusive 1-based page ranges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRanges(pub Vec<(u32, u32)>);

impl FromStr for PageRanges {
    type Err = PrintError;

    /// Parses `"1-3, 5"`. Open ranges (`"4-"`) are refused: the page count of the document is not
    /// known here, and guessing would print the wrong pages.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || PrintError::InvalidPageRanges(input.to_owned());
        let page = |text: &str| match text.trim().parse::<u32>() {
            Ok(n) if n >= 1 => Ok(n),
            _ => Err(invalid()),
        };

        let mut ranges = Vec::new();
        for part in input.split(',') {
            let range = match part.split_once('-') {
                Some((first, last)) => (page(first)?, page(last)?),
                None => {
                    let n = page(part)?;
                    (n, n)
                }
            };
            if range.0 > range.1 {
                return Err(invalid());
            }
            ranges.push(range);
        }

        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (first, last) in ranges {
            match merged.last_mut() {
                // Adjacent ranges merge too: CUPS rejects overlapping or unordered ranges.
                Some(previous) if first <= previous.1.saturating_add(1) => {
                    previous.1 = previous.1.max(last);
                }
                _ => merged.push((first, last)),
            }
        }
        Ok(PageRanges(merged))
    }
}

#[cfg(feature = "service")]
impl PageRanges {
    pub fn to_ipp(&self) -> Vec<ipp::value::IppValue> {
        self.0
            .iter()
            .map(|&(first, last)| ipp::value::IppValue::RangeOfInteger {
                min: i32::try_from(first).unwrap_or(i32::MAX),
                max: i32::try_from(last).unwrap_or(i32::MAX),
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrintOptions {
    pub copies: u32,
    pub sides: Sides,
    pub color_mode: ColorMode,
    pub page_ranges: Option<PageRanges>,
}

impl Default for PrintOptions {
    fn default() -> Self {
        PrintOptions {
            copies: 1,
            sides: Sides::OneSided,
            color_mode: ColorMode::Color,
            page_ranges: None,
        }
    }
}

#[cfg(feature = "service")]
impl PrintOptions {
    /// The job template attributes for Print-Job.
    pub fn to_attributes(
        &self,
        format: crate::DocumentFormat,
    ) -> Vec<ipp::attribute::IppAttribute> {
        use ipp::attribute::IppAttribute;
        use ipp::value::IppValue;

        // Names and keywords below are short constants, so building them cannot fail.
        let keyword = |name: &str, value: &str| {
            IppAttribute::with_name(name, IppValue::new_keyword(value).expect("valid keyword"))
                .expect("valid attribute name")
        };

        let mut attributes = Vec::new();
        if self.copies != 1 {
            attributes.push(
                IppAttribute::with_name(
                    IppAttribute::COPIES,
                    IppValue::Integer(i32::try_from(self.copies).unwrap_or(i32::MAX)),
                )
                .expect("valid attribute name"),
            );
        }
        attributes.push(keyword(IppAttribute::SIDES, self.sides.keyword()));
        attributes.push(keyword("print-color-mode", self.color_mode.keyword()));
        // Images are one page, and CUPS ignores the attribute for them at best.
        if let (Some(ranges), crate::DocumentFormat::Pdf) = (&self.page_ranges, format) {
            let mut values = ranges.to_ipp();
            let value = if values.len() == 1 {
                values.remove(0)
            } else {
                IppValue::Array(values)
            };
            attributes.push(
                IppAttribute::with_name(IppAttribute::PAGE_RANGES, value)
                    .expect("valid attribute name"),
            );
        }
        attributes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ranges(input: &str) -> Result<Vec<(u32, u32)>, PrintError> {
        input.parse::<PageRanges>().map(|r| r.0)
    }

    #[test]
    fn parses_page_ranges() {
        assert_eq!(ranges("1-3,5"), Ok(vec![(1, 3), (5, 5)]));
        assert_eq!(ranges(" 5 , 1-2 "), Ok(vec![(1, 2), (5, 5)]));
        assert_eq!(ranges("1-3,2-6"), Ok(vec![(1, 6)]));
        for bad in ["0", "3-1", "a", "", "4-"] {
            assert!(
                matches!(ranges(bad), Err(PrintError::InvalidPageRanges(_))),
                "{bad:?} should be refused"
            );
        }
    }

    #[test]
    fn keywords_round_trip() {
        for sides in [
            Sides::OneSided,
            Sides::TwoSidedLongEdge,
            Sides::TwoSidedShortEdge,
        ] {
            assert_eq!(Sides::from_keyword(sides.keyword()), Some(sides));
        }
        for mode in [ColorMode::Color, ColorMode::Monochrome] {
            assert_eq!(ColorMode::from_keyword(mode.keyword()), Some(mode));
        }
        assert_eq!(Sides::from_keyword("x"), None);
    }

    #[cfg(feature = "service")]
    #[test]
    fn maps_options_to_attributes() {
        use crate::DocumentFormat;
        use ipp::value::IppValue;

        let options = PrintOptions {
            copies: 3,
            sides: Sides::TwoSidedLongEdge,
            color_mode: ColorMode::Monochrome,
            page_ranges: Some("1-3,5".parse().unwrap()),
        };
        let find = |attrs: &[ipp::attribute::IppAttribute], name: &str| {
            attrs
                .iter()
                .find(|a| a.name().as_str() == name)
                .map(|a| a.value().clone())
        };

        let pdf = options.to_attributes(DocumentFormat::Pdf);
        assert_eq!(find(&pdf, "copies"), Some(IppValue::Integer(3)));
        assert_eq!(
            find(&pdf, "sides"),
            Some(IppValue::new_keyword("two-sided-long-edge").unwrap())
        );
        assert_eq!(
            find(&pdf, "print-color-mode"),
            Some(IppValue::new_keyword("monochrome").unwrap())
        );
        assert_eq!(
            find(&pdf, "page-ranges"),
            Some(IppValue::Array(vec![
                IppValue::RangeOfInteger { min: 1, max: 3 },
                IppValue::RangeOfInteger { min: 5, max: 5 },
            ]))
        );

        let png = options.to_attributes(DocumentFormat::Png);
        assert_eq!(find(&png, "page-ranges"), None);

        let single = PrintOptions::default().to_attributes(DocumentFormat::Pdf);
        assert_eq!(find(&single, "copies"), None);
    }
}
