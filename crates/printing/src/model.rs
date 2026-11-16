//! Printers and jobs as the interface shows them.

use serde::{Deserialize, Serialize};

use crate::{ColorMode, JobState, PrinterState, ReasonKind, Severity, Sides, StateReason, Supply};

/// What a printer supports, so the form only offers working choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub max_copies: u32,
    pub sides: Vec<Sides>,
    pub color_modes: Vec<ColorMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Printer {
    pub name: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub make_and_model: Option<String>,
    pub is_default: bool,
    pub state: PrinterState,
    pub reasons: Vec<StateReason>,
    pub accepting_jobs: bool,
    pub supplies: Vec<Supply>,
    pub capabilities: Capabilities,
}

impl Printer {
    /// A stopped printer without an error reason is only paused, which is not "offline".
    pub fn is_online(&self) -> bool {
        let offline = self.reasons.iter().any(|r| r.kind == ReasonKind::Offline);
        let failed = self.state == PrinterState::Stopped
            && self.reasons.iter().any(|r| r.severity == Severity::Error);
        !offline && !failed
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: i32,
    pub printer: String,
    pub name: Option<String>,
    pub owner: Option<String>,
    pub state: JobState,
    pub pages: Option<u32>,
    /// Unix seconds.
    pub created: Option<i64>,
}

/// What CUPS reports about one job: enough to tell whether it printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobStatus {
    pub state: JobState,
    /// `job-impressions-completed`; `None` when the printer does not report it.
    pub impressions_completed: Option<u32>,
}

impl Job {
    /// Whether the job can still be canceled or is still being worked on.
    pub fn is_active(&self) -> bool {
        !matches!(
            self.state,
            JobState::Canceled | JobState::Aborted | JobState::Completed
        )
    }
}

/// A document ready to send to CUPS.
#[cfg(feature = "service")]
#[derive(Clone, Debug, PartialEq)]
pub struct PrintRequest {
    pub printer: String,
    pub document: bytes::Bytes,
    pub format: crate::DocumentFormat,
    pub job_name: String,
    pub user: String,
    pub options: crate::PrintOptions,
}

/// Where the driver of a new printer comes from.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Driver {
    /// The printer describes itself (IPP Everywhere): CUPS queries it, so the address must
    /// answer.
    Everywhere,
    /// A `ppd-name` from the list CUPS gives (`PrinterAdmin::drivers`).
    Ppd(String),
}

/// A printer to add to CUPS.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewPrinter {
    /// The queue name.
    pub name: String,
    /// Where the printer is, such as `ipp://192.168.1.20/ipp/print`.
    pub device_uri: String,
    pub driver: Driver,
    pub description: Option<String>,
    pub location: Option<String>,
}

/// A driver CUPS can install.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DriverInfo {
    /// The `ppd-name` to give back in [`Driver::Ppd`].
    pub name: String,
    pub make_and_model: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printer(state: PrinterState, reasons: &[&str]) -> Printer {
        Printer {
            name: "office".into(),
            description: None,
            location: None,
            make_and_model: None,
            is_default: false,
            state,
            reasons: reasons
                .iter()
                .filter_map(|r| StateReason::parse(r))
                .collect(),
            accepting_jobs: true,
            supplies: vec![],
            capabilities: Capabilities {
                max_copies: 99,
                sides: vec![Sides::OneSided],
                color_modes: vec![ColorMode::Color],
            },
        }
    }

    #[test]
    fn online_depends_on_reasons_and_state() {
        assert!(printer(PrinterState::Idle, &["none"]).is_online());
        assert!(printer(PrinterState::Idle, &["toner-low-warning"]).is_online());
        assert!(!printer(PrinterState::Idle, &["offline-report"]).is_online());
        assert!(!printer(PrinterState::Stopped, &["media-jam-error"]).is_online());
        assert!(printer(PrinterState::Stopped, &["paused-report"]).is_online());
        // CUPS sets this briefly at the start of most jobs on a healthy printer.
        assert!(printer(PrinterState::Processing, &["connecting-to-device"]).is_online());
        assert!(printer(PrinterState::Stopped, &["connecting-to-device"]).is_online());
    }

    #[test]
    fn active_jobs() {
        let job = |state| Job {
            id: 1,
            printer: "p".into(),
            name: None,
            owner: None,
            state,
            pages: None,
            created: None,
        };
        assert!(job(JobState::Held).is_active());
        assert!(job(JobState::Stopped).is_active());
        assert!(!job(JobState::Completed).is_active());
    }
}
