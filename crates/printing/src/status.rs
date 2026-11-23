//! Printer and job state, as reported by CUPS.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PrinterState {
    Idle,
    Processing,
    Stopped,
}

impl PrinterState {
    /// Maps the IPP `printer-state` enum (3 to 5).
    pub fn from_ipp(value: i32) -> Option<PrinterState> {
        Some(match value {
            3 => PrinterState::Idle,
            4 => PrinterState::Processing,
            5 => PrinterState::Stopped,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Report,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReasonKind {
    Offline,
    /// CUPS is opening the connection to the printer, which it does at the start of most jobs.
    Connecting,
    MediaEmpty,
    MediaLow,
    MediaJam,
    TonerEmpty,
    TonerLow,
    MarkerSupplyEmpty,
    MarkerSupplyLow,
    DoorOpen,
    CoverOpen,
    Paused,
    Other(String),
}

/// One entry of `printer-state-reasons`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateReason {
    pub kind: ReasonKind,
    pub severity: Severity,
}

impl StateReason {
    /// Parses a `printer-state-reasons` keyword. `none` means no reason at all.
    pub fn parse(keyword: &str) -> Option<StateReason> {
        if keyword == "none" {
            return None;
        }
        // RFC 8011: a missing suffix means the same as `-error`.
        let (base, severity) = if let Some(base) = keyword.strip_suffix("-report") {
            (base, Severity::Report)
        } else if let Some(base) = keyword.strip_suffix("-warning") {
            (base, Severity::Warning)
        } else if let Some(base) = keyword.strip_suffix("-error") {
            (base, Severity::Error)
        } else {
            (keyword, Severity::Error)
        };
        // A printer that is only being connected to, or paused, is not offline: CUPS sends both
        // without a suffix (`connecting-to-device` at the start of most jobs on a healthy
        // printer, `paused` after Pause-Printer), so they are reports whatever the suffix says.
        match base {
            "connecting-to-device" => {
                return Some(StateReason {
                    kind: ReasonKind::Connecting,
                    severity: Severity::Report,
                });
            }
            "paused" => {
                return Some(StateReason {
                    kind: ReasonKind::Paused,
                    severity: Severity::Report,
                });
            }
            _ => {}
        }
        let kind = match base {
            "offline" => ReasonKind::Offline,
            "media-empty" => ReasonKind::MediaEmpty,
            "media-low" => ReasonKind::MediaLow,
            "media-jam" => ReasonKind::MediaJam,
            "toner-empty" => ReasonKind::TonerEmpty,
            "toner-low" => ReasonKind::TonerLow,
            "marker-supply-empty" => ReasonKind::MarkerSupplyEmpty,
            "marker-supply-low" => ReasonKind::MarkerSupplyLow,
            "door-open" => ReasonKind::DoorOpen,
            "cover-open" => ReasonKind::CoverOpen,
            other => ReasonKind::Other(other.to_owned()),
        };
        Some(StateReason { kind, severity })
    }
}

/// A consumable (ink, toner) with its remaining level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supply {
    pub name: String,
    /// Percent; `None` when the printer does not know.
    pub level: Option<u8>,
    /// `#RRGGBB`.
    pub color: Option<String>,
    pub kind: String,
}

/// Builds supplies from the parallel `marker-*` attributes. CUPS can return lists of different
/// lengths for a misbehaving printer, so the shortest one wins rather than panicking.
pub fn supplies(
    names: &[String],
    levels: &[i32],
    colors: &[String],
    types: &[String],
) -> Vec<Supply> {
    names
        .iter()
        .zip(levels)
        .zip(colors)
        .zip(types)
        .map(|(((name, level), color), kind)| Supply {
            name: name.clone(),
            // Negative levels are the IPP codes for unknown or unavailable.
            level: (*level >= 0).then(|| u8::try_from((*level).min(100)).unwrap_or(0)),
            // A multi-color marker lists several colors in one value, which a single swatch
            // cannot show.
            color: (color.len() == 7 && color.starts_with('#')).then(|| color.clone()),
            kind: kind.clone(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Pending,
    Held,
    Processing,
    Stopped,
    Canceled,
    Aborted,
    Completed,
}

impl JobState {
    /// Maps the IPP `job-state` enum (3 to 9).
    pub fn from_ipp(value: i32) -> Option<JobState> {
        Some(match value {
            3 => JobState::Pending,
            4 => JobState::Held,
            5 => JobState::Processing,
            6 => JobState::Stopped,
            7 => JobState::Canceled,
            8 => JobState::Aborted,
            9 => JobState::Completed,
            _ => return None,
        })
    }

    /// The IPP `job-state` keyword, as recto stores it.
    pub fn keyword(self) -> &'static str {
        match self {
            JobState::Pending => "pending",
            JobState::Held => "pending-held",
            JobState::Processing => "processing",
            JobState::Stopped => "processing-stopped",
            JobState::Canceled => "canceled",
            JobState::Aborted => "aborted",
            JobState::Completed => "completed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parses_state_reasons() {
        let reason = |kind, severity| Some(StateReason { kind, severity });
        assert_eq!(
            StateReason::parse("media-empty-error"),
            reason(ReasonKind::MediaEmpty, Severity::Error)
        );
        assert_eq!(
            StateReason::parse("toner-low-warning"),
            reason(ReasonKind::TonerLow, Severity::Warning)
        );
        assert_eq!(
            StateReason::parse("offline-report"),
            reason(ReasonKind::Offline, Severity::Report)
        );
        assert_eq!(
            StateReason::parse("connecting-to-device"),
            reason(ReasonKind::Connecting, Severity::Report)
        );
        assert_eq!(StateReason::parse("none"), None);
        assert_eq!(
            StateReason::parse("foo-bar"),
            reason(ReasonKind::Other("foo-bar".into()), Severity::Error)
        );
    }

    #[test]
    fn builds_supplies() {
        let built = supplies(
            &strings(&["Cyan", "Black", "Extra"]),
            &[-1, 80],
            &strings(&["#00FFFF", "none", "#000000"]),
            &strings(&["toner", "toner", "toner"]),
        );
        assert_eq!(built.len(), 2);
        assert_eq!(built[0].level, None);
        assert_eq!(built[0].color.as_deref(), Some("#00FFFF"));
        assert_eq!(built[1].level, Some(80));
        assert_eq!(built[1].color, None);
    }

    #[test]
    fn printer_state_from_ipp() {
        assert_eq!(PrinterState::from_ipp(3), Some(PrinterState::Idle));
        assert_eq!(PrinterState::from_ipp(4), Some(PrinterState::Processing));
        assert_eq!(PrinterState::from_ipp(5), Some(PrinterState::Stopped));
        assert_eq!(PrinterState::from_ipp(0), None);
    }

    #[test]
    fn job_state_from_ipp() {
        assert_eq!(JobState::from_ipp(3), Some(JobState::Pending));
        assert_eq!(JobState::from_ipp(5), Some(JobState::Processing));
        assert_eq!(JobState::from_ipp(9), Some(JobState::Completed));
        assert_eq!(JobState::from_ipp(42), None);
    }
}
