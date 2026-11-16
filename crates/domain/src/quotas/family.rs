//! Which rules apply to a job, and what a job counts for under each.

use std::collections::BTreeMap;

use super::{Metric, Period, Rule, Subject};
use crate::ledger::Counts;

/// How specific a personal rule's subject is: the most specific of a family wins.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Everyone,
    Group,
    User,
}

/// The rules a job by `user_id`, member of `groups`, on `printer` must pass, by id.
///
/// A rule matches when it is enabled, its subject is everyone, one of the user's groups, the
/// user or a pool of one of the user's groups, and it covers the printer (no printers means
/// every printer; names compare without case, as CUPS does). Personal rules (everyone, group,
/// user) with the same metric, period and printers form a family, and only one rule of a family
/// applies: the user's over a group's over everyone's, and among rules of the same rank, the
/// largest limit. Pool rules are shared budgets, never replaced: every matching pool applies.
pub fn applicable(rules: &[Rule], user_id: i64, groups: &[i64], printer: &str) -> Vec<Rule> {
    let mut pools = Vec::new();
    let mut families: BTreeMap<(Metric, Period, Vec<String>), &Rule> = BTreeMap::new();
    for rule in rules.iter().filter(|rule| rule.enabled) {
        if !covers(rule, printer) {
            continue;
        }
        let rank = match rule.subject {
            Subject::Everyone => Rank::Everyone,
            Subject::Group(group) if groups.contains(&group) => Rank::Group,
            Subject::User(user) if user == user_id => Rank::User,
            Subject::Pool(group) if groups.contains(&group) => {
                pools.push(rule.clone());
                continue;
            }
            _ => continue,
        };
        let key = (rule.metric, rule.period, scope(&rule.printers));
        let better = |current: &Rule| {
            let current_rank = rank_of(current);
            rank > current_rank
                || (rank == current_rank
                    && (rule.limit > current.limit
                        || (rule.limit == current.limit && rule.id < current.id)))
        };
        match families.get(&key) {
            Some(current) if !better(current) => {}
            _ => {
                families.insert(key, rule);
            }
        }
    }
    let mut applicable: Vec<Rule> = families.into_values().cloned().collect();
    applicable.extend(pools);
    applicable.sort_by_key(|rule| rule.id);
    applicable
}

fn rank_of(rule: &Rule) -> Rank {
    match rule.subject {
        Subject::User(_) => Rank::User,
        Subject::Group(_) => Rank::Group,
        _ => Rank::Everyone,
    }
}

/// Whether a rule counts jobs on `printer`.
pub(super) fn covers(rule: &Rule, printer: &str) -> bool {
    rule.printers.is_empty()
        || rule
            .printers
            .iter()
            .any(|name| name.eq_ignore_ascii_case(printer))
}

/// The printers of a rule as a family compares them: in lower case, sorted, once each.
fn scope(printers: &[String]) -> Vec<String> {
    let mut scope: Vec<String> = printers
        .iter()
        .map(|printer| printer.to_ascii_lowercase())
        .collect();
    scope.sort();
    scope.dedup();
    scope
}

/// What a job counts for under a metric; `None` when its counts are unknown (a PDF recto could
/// not read), which only the jobs metric does not need. Cost is unknown too then, since it comes
/// from the pages.
pub fn amount(metric: Metric, counts: &Counts) -> Option<i64> {
    match metric {
        Metric::Pages => counts.pages,
        Metric::Sheets => counts.sheets,
        Metric::ColorPages => counts.color_pages,
        Metric::MonoPages => counts.mono_pages,
        Metric::Jobs => Some(1),
        Metric::Cost => counts.pages.map(|_| counts.cost),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quotas::Enforcement;

    const DESIGN: i64 = 10;
    const SALES: i64 = 11;
    const DESIGNER: i64 = 1;
    const OTHER: i64 = 2;

    fn rule(id: i64, subject: Subject, metric: Metric, limit: i64) -> Rule {
        Rule {
            id,
            name: format!("rule {id}"),
            enabled: true,
            subject,
            printers: vec![],
            metric,
            period: Period::Month,
            limit,
            enforcement: Enforcement::Hard,
        }
    }

    fn limits(rules: &[Rule]) -> Vec<(i64, i64)> {
        rules.iter().map(|rule| (rule.id, rule.limit)).collect()
    }

    #[test]
    fn the_most_specific_personal_rule_wins() {
        // The spec's example.
        let rules = vec![
            rule(1, Subject::Everyone, Metric::ColorPages, 50),
            rule(2, Subject::Group(DESIGN), Metric::ColorPages, 500),
            rule(3, Subject::Everyone, Metric::Pages, 1000),
        ];
        assert_eq!(
            limits(&applicable(&rules, DESIGNER, &[DESIGN], "office")),
            [(2, 500), (3, 1000)]
        );
        assert_eq!(
            limits(&applicable(&rules, OTHER, &[], "office")),
            [(1, 50), (3, 1000)]
        );

        // The user's own rule beats the group's, even when smaller.
        let mut rules = rules;
        rules.push(rule(4, Subject::User(DESIGNER), Metric::ColorPages, 20));
        assert_eq!(
            limits(&applicable(&rules, DESIGNER, &[DESIGN], "office")),
            [(3, 1000), (4, 20)]
        );
        assert_eq!(
            limits(&applicable(&rules, OTHER, &[], "office")),
            [(1, 50), (3, 1000)]
        );

        // Two groups of the user, one family: the most generous.
        let rules = vec![
            rule(1, Subject::Group(DESIGN), Metric::ColorPages, 500),
            rule(2, Subject::Group(SALES), Metric::ColorPages, 800),
            rule(3, Subject::Group(SALES), Metric::ColorPages, 100),
        ];
        assert_eq!(
            limits(&applicable(&rules, DESIGNER, &[DESIGN, SALES], "office")),
            [(2, 800)]
        );
        assert_eq!(
            limits(&applicable(&rules, DESIGNER, &[DESIGN], "office")),
            [(1, 500)]
        );

        // A pool is kept beside the personal rule of the same family, and only for members.
        let rules = vec![
            rule(1, Subject::Group(DESIGN), Metric::ColorPages, 500),
            rule(2, Subject::Pool(DESIGN), Metric::ColorPages, 2000),
            rule(3, Subject::User(DESIGNER), Metric::ColorPages, 50),
            rule(4, Subject::Pool(SALES), Metric::ColorPages, 9),
        ];
        assert_eq!(
            limits(&applicable(&rules, DESIGNER, &[DESIGN], "office")),
            [(2, 2000), (3, 50)]
        );
        assert!(applicable(&rules, OTHER, &[], "office").is_empty());
    }

    #[test]
    fn other_periods_and_disabled_rules() {
        let mut monthly = rule(1, Subject::Everyone, Metric::Pages, 100);
        let mut weekly = rule(2, Subject::Group(DESIGN), Metric::Pages, 30);
        weekly.period = Period::Week;
        // A different period is a different family: both apply.
        assert_eq!(
            limits(&applicable(
                &[monthly.clone(), weekly.clone()],
                DESIGNER,
                &[DESIGN],
                "office"
            )),
            [(1, 100), (2, 30)]
        );
        // A disabled rule is as if absent, and does not hide the one it would beat.
        let mut group = rule(3, Subject::Group(DESIGN), Metric::Pages, 500);
        group.enabled = false;
        monthly.enabled = true;
        weekly.enabled = false;
        assert_eq!(
            limits(&applicable(
                &[monthly, weekly, group],
                DESIGNER,
                &[DESIGN],
                "office"
            )),
            [(1, 100)]
        );
    }

    #[test]
    fn printer_scopes_are_separate_families() {
        let mut lab = rule(1, Subject::Everyone, Metric::ColorPages, 10);
        lab.printers = vec!["Lab".into()];
        let all = rule(2, Subject::Everyone, Metric::ColorPages, 100);
        let rules = vec![lab.clone(), all];
        // Names compare without case, as in CUPS.
        assert_eq!(
            limits(&applicable(&rules, OTHER, &[], "lab")),
            [(1, 10), (2, 100)]
        );
        assert_eq!(
            limits(&applicable(&rules, OTHER, &[], "office")),
            [(2, 100)]
        );

        // The same printers in another order and case are the same family.
        let mut group = rule(3, Subject::Group(DESIGN), Metric::ColorPages, 40);
        group.printers = vec!["office".into(), "LAB".into()];
        let mut everyone = rule(4, Subject::Everyone, Metric::ColorPages, 5);
        everyone.printers = vec!["lab".into(), "Office".into()];
        assert_eq!(
            limits(&applicable(&[group, everyone], DESIGNER, &[DESIGN], "lab")),
            [(3, 40)]
        );
    }

    #[test]
    fn a_job_counts_by_metric() {
        let counts = Counts {
            pages: Some(6),
            sheets: Some(3),
            color_pages: Some(6),
            mono_pages: Some(0),
            cost: 123,
        };
        assert_eq!(amount(Metric::Pages, &counts), Some(6));
        assert_eq!(amount(Metric::Sheets, &counts), Some(3));
        assert_eq!(amount(Metric::ColorPages, &counts), Some(6));
        assert_eq!(amount(Metric::MonoPages, &counts), Some(0));
        assert_eq!(amount(Metric::Jobs, &counts), Some(1));
        assert_eq!(amount(Metric::Cost, &counts), Some(123));
        let unknown = Counts {
            pages: None,
            sheets: None,
            color_pages: None,
            mono_pages: None,
            cost: 0,
        };
        for metric in [
            Metric::Pages,
            Metric::Sheets,
            Metric::ColorPages,
            Metric::MonoPages,
            Metric::Cost,
        ] {
            assert_eq!(amount(metric, &unknown), None, "{metric:?}");
        }
        assert_eq!(amount(Metric::Jobs, &unknown), Some(1));
    }
}
