//! Quota rules and where people stand against them, as the interface words them. The server
//! reads them from `recto-domain` and hands the pages these plain copies, which the browser
//! build can hold too.

use serde::{Deserialize, Serialize};

use crate::{currency::Currency, t, tf, tp};

/// What a rule counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Metric {
    Pages,
    Sheets,
    ColorPages,
    MonoPages,
    Jobs,
    /// In minor currency units.
    Cost,
}

/// The window a rule counts in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Period {
    Day,
    Week,
    Month,
    Quarter,
    Year,
    /// The last so many days.
    Rolling(u16),
    Total,
}

impl Metric {
    pub const ALL: [Metric; 6] = [
        Metric::Pages,
        Metric::Sheets,
        Metric::ColorPages,
        Metric::MonoPages,
        Metric::Jobs,
        Metric::Cost,
    ];

    /// The keyword forms post.
    pub fn keyword(self) -> &'static str {
        match self {
            Metric::Pages => "pages",
            Metric::Sheets => "sheets",
            Metric::ColorPages => "color_pages",
            Metric::MonoPages => "mono_pages",
            Metric::Jobs => "jobs",
            Metric::Cost => "cost",
        }
    }

    /// Its name in a list of choices.
    pub fn label(self) -> &'static str {
        match self {
            Metric::Pages => t!("quotas.metric.pages"),
            Metric::Sheets => t!("quotas.metric.sheets"),
            Metric::ColorPages => t!("quotas.metric.color_pages"),
            Metric::MonoPages => t!("quotas.metric.mono_pages"),
            Metric::Jobs => t!("quotas.metric.jobs"),
            Metric::Cost => t!("quotas.metric.cost"),
        }
    }

    /// `count` of the metric, such as `50 color pages` or `2.50 EUR`.
    pub fn amount(self, count: i64, currency: &Currency) -> String {
        match self {
            Metric::Pages => tp!("quotas.amount.pages", count),
            Metric::Sheets => tp!("quotas.amount.sheets", count),
            Metric::ColorPages => tp!("quotas.amount.color_pages", count),
            Metric::MonoPages => tp!("quotas.amount.mono_pages", count),
            Metric::Jobs => tp!("quotas.amount.jobs", count),
            Metric::Cost => currency.format(count),
        }
    }

    /// A limit as a form field shows it: an amount in the currency for cost.
    pub fn field(self, limit: i64, currency: &Currency) -> String {
        match self {
            Metric::Cost => currency.amount_in(limit, crate::i18n::locale()),
            _ => limit.to_string(),
        }
    }
}

impl Period {
    /// The keywords forms post, rolling days apart.
    pub const KEYWORDS: [&'static str; 7] = [
        "day", "week", "month", "quarter", "year", "rolling", "total",
    ];

    pub fn keyword(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Quarter => "quarter",
            Period::Year => "year",
            Period::Rolling(_) => "rolling",
            Period::Total => "total",
        }
    }

    /// The name of a period keyword in a list of choices.
    pub fn label(keyword: &str) -> &'static str {
        match keyword {
            "day" => t!("quotas.period.day"),
            "week" => t!("quotas.period.week"),
            "month" => t!("quotas.period.month"),
            "quarter" => t!("quotas.period.quarter"),
            "year" => t!("quotas.period.year"),
            "rolling" => t!("quotas.period.rolling"),
            _ => t!("quotas.period.total"),
        }
    }

    /// The current window: `this month`, `in the last 7 days`, `in total`.
    pub fn current(self) -> String {
        match self {
            Period::Day => t!("quotas.current.day").to_owned(),
            Period::Week => t!("quotas.current.week").to_owned(),
            Period::Month => t!("quotas.current.month").to_owned(),
            Period::Quarter => t!("quotas.current.quarter").to_owned(),
            Period::Year => t!("quotas.current.year").to_owned(),
            Period::Rolling(days) => tp!("quotas.current.rolling", days),
            Period::Total => t!("quotas.current.total").to_owned(),
        }
    }

    /// How often a limit renews: `a month`, `in any 7 days`, `in total`.
    pub fn each(self) -> String {
        match self {
            Period::Day => t!("quotas.each.day").to_owned(),
            Period::Week => t!("quotas.each.week").to_owned(),
            Period::Month => t!("quotas.each.month").to_owned(),
            Period::Quarter => t!("quotas.each.quarter").to_owned(),
            Period::Year => t!("quotas.each.year").to_owned(),
            Period::Rolling(days) => tp!("quotas.each.rolling", days),
            Period::Total => t!("quotas.each.total").to_owned(),
        }
    }
}

/// Where someone stands against one rule, in its current window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowance {
    pub rule: i64,
    pub name: String,
    pub metric: Metric,
    pub period: Period,
    /// The printers the rule is limited to; empty for every printer.
    pub printers: Vec<String>,
    pub limit: i64,
    /// Extra from grants that have not expired.
    pub granted: i64,
    pub used: i64,
}

impl Allowance {
    /// The limit with the grants.
    fn total(&self) -> i64 {
        self.limit.saturating_add(self.granted)
    }

    /// What is left: `70 of 100 pages left this month`, then the printers when the rule is
    /// limited to some. Never below 0, even over a soft rule.
    pub fn left(&self, currency: &Currency) -> String {
        let total = self.total();
        let left = total.saturating_sub(self.used).max(0);
        let period = self.period.current();
        let text = match self.metric {
            Metric::Cost => tf!(
                "quotas.left.cost",
                left = currency.format(left),
                limit = currency.format(total),
                period = period
            ),
            Metric::Pages => tf!(
                "quotas.left.pages",
                count = total,
                left = left,
                period = period
            ),
            Metric::Sheets => {
                tf!(
                    "quotas.left.sheets",
                    count = total,
                    left = left,
                    period = period
                )
            }
            Metric::ColorPages => tf!(
                "quotas.left.color_pages",
                count = total,
                left = left,
                period = period
            ),
            Metric::MonoPages => tf!(
                "quotas.left.mono_pages",
                count = total,
                left = left,
                period = period
            ),
            Metric::Jobs => tf!(
                "quotas.left.jobs",
                count = total,
                left = left,
                period = period
            ),
        };
        on_printers(text, &self.printers)
    }

    /// What was used: `30 of 100 pages used this month`, with the extra granted if any.
    pub fn used(&self, currency: &Currency) -> String {
        let period = self.period.current();
        let used = match self.metric {
            Metric::Cost => currency.format(self.used),
            _ => self.used.to_string(),
        };
        let text = tf!(
            "quotas.used",
            used = used,
            limit = self.metric.amount(self.limit, currency),
            period = period
        );
        if self.granted > 0 {
            tf!(
                "quotas.with_grant",
                text = text,
                granted = self.metric.amount(self.granted, currency)
            )
        } else {
            text
        }
    }
}

/// `text`, then the printers a rule is limited to.
pub fn on_printers(text: String, printers: &[String]) -> String {
    if printers.is_empty() {
        text
    } else {
        tf!(
            "quotas.on_printers",
            text = text,
            printers = printers.join(t!("quotas.separator"))
        )
    }
}

#[cfg(feature = "ssr")]
mod domain {
    use recto_domain::quotas::{self, Standing};

    use super::{Allowance, Metric, Period};

    impl From<quotas::Metric> for Metric {
        fn from(metric: quotas::Metric) -> Self {
            match metric {
                quotas::Metric::Pages => Metric::Pages,
                quotas::Metric::Sheets => Metric::Sheets,
                quotas::Metric::ColorPages => Metric::ColorPages,
                quotas::Metric::MonoPages => Metric::MonoPages,
                quotas::Metric::Jobs => Metric::Jobs,
                quotas::Metric::Cost => Metric::Cost,
            }
        }
    }

    impl From<quotas::Period> for Period {
        fn from(period: quotas::Period) -> Self {
            match period {
                quotas::Period::Day => Period::Day,
                quotas::Period::Week => Period::Week,
                quotas::Period::Month => Period::Month,
                quotas::Period::Quarter => Period::Quarter,
                quotas::Period::Year => Period::Year,
                quotas::Period::Rolling(days) => Period::Rolling(days),
                quotas::Period::Total => Period::Total,
            }
        }
    }

    impl From<&Standing> for Allowance {
        fn from(standing: &Standing) -> Self {
            let rule = &standing.rule;
            Allowance {
                rule: rule.id,
                name: rule.name.clone(),
                metric: rule.metric.into(),
                period: rule.period.into(),
                printers: rule.printers.clone(),
                limit: rule.limit,
                granted: standing.granted,
                used: standing.used,
            }
        }
    }
}
