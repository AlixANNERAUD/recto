//! A printer as the pages show it: its state, reasons and supplies, and the names of its options.

use leptos::prelude::*;
use recto_printing::{ColorMode, PrinterState, ReasonKind, Severity, Sides, StateReason, Supply};

use super::ui::{Badge, Tone};
use crate::{t, tf};

/// The name of a duplex choice, as the print and release forms offer it.
pub fn sides_label(sides: Sides) -> &'static str {
    match sides {
        Sides::OneSided => t!("print.sides.one_sided"),
        Sides::TwoSidedLongEdge => t!("print.sides.long_edge"),
        Sides::TwoSidedShortEdge => t!("print.sides.short_edge"),
    }
}

/// The name of a color choice, as the print and release forms offer it.
pub fn color_label(mode: ColorMode) -> &'static str {
    match mode {
        ColorMode::Color => t!("print.color.color"),
        ColorMode::Monochrome => t!("print.color.monochrome"),
    }
}

/// The overall state in one word, and its tone. An offline printer says so whatever its IPP
/// state, since that is what decides whether the job comes out.
pub fn state_label(printer: &recto_printing::Printer) -> (String, Tone) {
    if !printer.is_online() {
        return (t!("printers.offline").to_string(), Tone::Danger);
    }
    match printer.state {
        PrinterState::Idle => (t!("printers.state.idle").to_string(), Tone::Success),
        PrinterState::Processing => (t!("printers.state.processing").to_string(), Tone::Brand),
        PrinterState::Stopped => (t!("printers.state.stopped").to_string(), Tone::Warning),
    }
}

/// The printer's name in the printer list of the print form, with what sets it apart:
/// `office (default, offline)`.
pub fn option_label(printer: &recto_printing::Printer) -> String {
    let mut details = Vec::new();
    if printer.is_default {
        details.push(t!("print.markers.default").to_string());
    }
    if !printer.is_online() {
        details.push(t!("print.markers.offline").to_string());
    } else if !printer.accepting_jobs {
        details.push(t!("print.markers.not_accepting").to_string());
    }
    if details.is_empty() {
        printer.name.clone()
    } else {
        tf!(
            "print.markers.label",
            name = printer.name,
            details = details.join(", ")
        )
    }
}

fn reason_label(reason: &StateReason) -> String {
    match &reason.kind {
        ReasonKind::Offline => t!("printers.reasons.offline").to_string(),
        ReasonKind::Connecting => t!("printers.reasons.connecting").to_string(),
        ReasonKind::MediaEmpty => t!("printers.reasons.media_empty").to_string(),
        ReasonKind::MediaLow => t!("printers.reasons.media_low").to_string(),
        ReasonKind::MediaJam => t!("printers.reasons.media_jam").to_string(),
        ReasonKind::TonerEmpty => t!("printers.reasons.toner_empty").to_string(),
        ReasonKind::TonerLow => t!("printers.reasons.toner_low").to_string(),
        ReasonKind::MarkerSupplyEmpty => t!("printers.reasons.marker_supply_empty").to_string(),
        ReasonKind::MarkerSupplyLow => t!("printers.reasons.marker_supply_low").to_string(),
        ReasonKind::DoorOpen => t!("printers.reasons.door_open").to_string(),
        ReasonKind::CoverOpen => t!("printers.reasons.cover_open").to_string(),
        ReasonKind::Paused => t!("printers.reasons.paused").to_string(),
        ReasonKind::Other(keyword) => tf!("printers.reasons.other", keyword = keyword),
    }
}

fn severity_tone(severity: Severity) -> Tone {
    match severity {
        Severity::Report => Tone::Neutral,
        Severity::Warning => Tone::Warning,
        Severity::Error => Tone::Danger,
    }
}

/// A supply's own color when it is a plain `#RRGGBB`, which is all a `style` attribute may
/// receive from the printer; otherwise the accent.
fn supply_color(supply: &Supply) -> &str {
    match supply.color.as_deref() {
        Some(color)
            if color.len() == 7
                && color.starts_with('#')
                && color[1..].bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            color
        }
        _ => "var(--brand)",
    }
}

#[component]
fn SupplyMeter(supply: Supply) -> impl IntoView {
    let color = supply_color(&supply).to_owned();
    let level_text = match supply.level {
        Some(level) => tf!("printers.level", level = level),
        None => t!("printers.unknown_level").to_string(),
    };
    let fill = supply.level.map(|level| {
        view! {
            <div class="h-full rounded-full" style=format!("width:{level}%;background-color:{color}")></div>
        }
    });
    view! {
        <li class="grid gap-1">
            <div class="flex items-baseline justify-between gap-3 text-sm">
                <span class="min-w-0 truncate">{supply.name.clone()}</span>
                <span class="text-muted-foreground shrink-0 tabular-nums">{level_text}</span>
            </div>
            // The text above already says the level; the bar is its picture.
            <div aria-hidden="true" class="bg-muted h-2 overflow-hidden rounded-full border">
                {fill}
            </div>
        </li>
    }
}

/// The status of one printer, for a card: its name is a second-level heading.
#[component]
pub fn PrinterStatus(printer: recto_printing::Printer) -> impl IntoView {
    let (state, tone) = state_label(&printer);
    let details = [
        printer.description.clone(),
        printer.location.clone(),
        printer.make_and_model.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|detail| !detail.is_empty() && *detail != printer.name)
    .collect::<Vec<_>>()
    .join(" · ");
    let reasons: Vec<_> = printer
        .reasons
        .iter()
        // The state badge already says the printer is offline.
        .filter(|reason| reason.kind != ReasonKind::Offline || printer.is_online())
        .collect();
    let reasons = (!reasons.is_empty()).then(|| {
        let items = reasons.into_iter().map(|reason| {
            let tone = severity_tone(reason.severity);
            view! {
                <li class="flex items-center gap-2 text-sm">
                    <span aria-hidden="true" class=format!("size-2 shrink-0 rounded-full {}", tone.dot())></span>
                    {reason_label(reason)}
                </li>
            }
        });
        view! { <ul class="mt-3 grid gap-1">{items.collect_view()}</ul> }
    });
    let not_accepting = (!printer.accepting_jobs && printer.is_online()).then(|| {
        view! { <Badge tone=Tone::Warning>{t!("printers.not_accepting")}</Badge> }
    });
    let default = printer
        .is_default
        .then(|| view! { <Badge>{t!("printers.default")}</Badge> });
    let supplies = (!printer.supplies.is_empty()).then(|| {
        view! {
            <div class="mt-4 grid gap-2">
                <h3 class="text-muted-foreground text-xs font-semibold tracking-wide uppercase">
                    {t!("printers.supplies")}
                </h3>
                <ul class="grid gap-3">
                    {printer
                        .supplies
                        .iter()
                        .cloned()
                        .map(|supply| view! { <SupplyMeter supply /> })
                        .collect_view()}
                </ul>
            </div>
        }
    });
    view! {
        <div>
            <div class="flex flex-wrap items-start justify-between gap-2">
                <div class="min-w-0">
                    <h2 class="truncate font-semibold">{printer.name.clone()}</h2>
                    {(!details.is_empty())
                        .then(|| view! { <p class="text-muted-foreground text-sm">{details}</p> })}
                </div>
                <div class="flex flex-wrap gap-1.5">
                    <Badge tone>{state}</Badge>
                    {not_accepting}
                    {default}
                </div>
            </div>
            {reasons}
            {supplies}
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn supply(color: Option<&str>) -> Supply {
        Supply {
            name: "Black".into(),
            level: Some(50),
            color: color.map(str::to_owned),
            kind: "toner".into(),
        }
    }

    #[test]
    fn only_plain_hex_colors_reach_the_style() {
        assert_eq!(supply_color(&supply(Some("#00AAFF"))), "#00AAFF");
        assert_eq!(supply_color(&supply(Some("#;x:url"))), "var(--brand)");
        assert_eq!(supply_color(&supply(None)), "var(--brand)");
    }
}
