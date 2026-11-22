//! Amounts of money as the interface reads and writes them. The same code runs on the server
//! and in the browser, so the job list's costs read the same once its island refreshes them.

use serde::{Deserialize, Serialize};

/// The currency prices and costs are shown in. Amounts are stored in minor units (cents); this
/// only decides how they read, and never converts anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Currency {
    /// The ISO 4217 code, in capitals.
    pub code: String,
    /// Digits after the decimal point: the currency's minor unit exponent.
    pub decimals: u32,
}

/// Currencies with a thousandth as the minor unit (ISO 4217 exponent 3).
const THREE_DECIMALS: [&str; 7] = ["BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND"];

/// Currencies without a minor unit (ISO 4217 exponent 0); every other one has two decimals.
const ZERO_DECIMAL: [&str; 16] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "VND", "VUV",
    "XAF", "XOF", "XPF",
];

impl Default for Currency {
    /// Euros, the default of `RECTO_CURRENCY`.
    fn default() -> Self {
        Self::new("EUR")
    }
}

impl Currency {
    /// The largest amount [`Currency::parse_amount`] accepts, in major units: far above any
    /// price, and far below where minor units overflow.
    pub const MAX_AMOUNT: i64 = 1_000_000;

    /// The currency of an ISO code, in any case.
    pub fn new(code: &str) -> Self {
        let code = code.to_ascii_uppercase();
        let decimals = if ZERO_DECIMAL.contains(&code.as_str()) {
            0
        } else if THREE_DECIMALS.contains(&code.as_str()) {
            3
        } else {
            2
        };
        Self { code, decimals }
    }

    /// An amount in minor units as `12.34 EUR`.
    pub fn format(&self, minor: i64) -> String {
        format!("{} {}", self.amount(minor), self.code)
    }

    /// An amount in minor units as `12.34`, without the code: what a form field shows, and
    /// what [`Currency::parse_amount`] reads back.
    pub fn amount(&self, minor: i64) -> String {
        let sign = if minor < 0 { "-" } else { "" };
        let minor = minor.unsigned_abs();
        if self.decimals == 0 {
            return format!("{sign}{minor}");
        }
        let unit = 10u64.pow(self.decimals);
        format!(
            "{sign}{}.{:0width$}",
            minor / unit,
            minor % unit,
            width = self.decimals as usize
        )
    }

    /// Reads a price typed in the currency, such as `0.05` or `0,05`, into minor units. Refuses
    /// what is not a plain non-negative amount, more decimals than the currency has, and more
    /// than [`Currency::MAX_AMOUNT`] major units.
    pub fn parse_amount(&self, text: &str) -> Option<i64> {
        let text = text.trim();
        let (whole, fraction) = match text.split_once(['.', ',']) {
            Some((whole, fraction)) => (whole, Some(fraction)),
            None => (text, None),
        };
        let digits =
            |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
        if !digits(whole) {
            return None;
        }
        let unit = 10i64.pow(self.decimals);
        let mut minor = match fraction {
            None => 0,
            Some(fraction) => {
                if !digits(fraction) || fraction.len() > self.decimals as usize {
                    return None;
                }
                // `0.1` in a two-decimal currency is 10 minor units.
                let scale = 10i64.pow(self.decimals - fraction.len() as u32);
                fraction.parse::<i64>().ok()? * scale
            }
        };
        // Leading zeros aside, more digits than the limit has is over it, and would not fit.
        let whole = whole.trim_start_matches('0');
        if whole.len() > 7 {
            return None;
        }
        let whole: i64 = if whole.is_empty() {
            0
        } else {
            whole.parse().ok()?
        };
        minor += whole * unit;
        (minor <= Self::MAX_AMOUNT * unit).then_some(minor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_in_the_currency_minor_unit() {
        let euro = Currency::new("eur");
        assert_eq!(euro.format(1234), "12.34 EUR");
        assert_eq!(euro.format(5), "0.05 EUR");
        assert_eq!(euro.format(-250), "-2.50 EUR");
        assert_eq!(Currency::new("JPY").format(1234), "1234 JPY");
        let dinar = Currency::new("KWD");
        assert_eq!(dinar.decimals, 3);
        assert_eq!(dinar.format(12_345), "12.345 KWD");
        assert_eq!(dinar.format(7), "0.007 KWD");
        assert_eq!(euro.amount(5), "0.05");
        assert_eq!(Currency::new("JPY").amount(12), "12");
    }

    #[test]
    fn typed_amounts_read_into_minor_units() {
        let euro = Currency::new("EUR");
        let yen = Currency::new("JPY");
        let dinar = Currency::new("KWD");
        for (currency, text, expected) in [
            (&euro, "0.05", Some(5)),
            (&euro, "0,05", Some(5)),
            (&euro, " 1.5 ", Some(150)),
            (&euro, "2", Some(200)),
            (&euro, "0", Some(0)),
            (&euro, "007.10", Some(710)),
            (&euro, "1000000", Some(100_000_000)),
            (&euro, "1000000.00", Some(100_000_000)),
            (&yen, "12", Some(12)),
            (&dinar, "0.125", Some(125)),
            (&dinar, "0.1", Some(100)),
            (&euro, "1000000.01", None),
            (&euro, "1000001", None),
            (&euro, "99999999999999999999", None),
            (&euro, "-0.05", None),
            (&euro, "+1", None),
            (&euro, "0.005", None),
            (&euro, "", None),
            (&euro, " ", None),
            (&euro, ".5", None),
            (&euro, "5.", None),
            (&euro, "1,000.00", None),
            (&euro, "1.2.3", None),
            (&euro, "1e3", None),
            (&euro, "٣", None),
            (&euro, "5 EUR", None),
            (&yen, "1.5", None),
            (&yen, "1.0", None),
            (&dinar, "0.0001", None),
        ] {
            assert_eq!(
                currency.parse_amount(text),
                expected,
                "{text:?} in {}",
                currency.code
            );
        }
    }
}
