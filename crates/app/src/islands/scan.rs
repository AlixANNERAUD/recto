//! The scan page's controls: choose a scanner and settings, preview, drag a rectangle on the
//! preview to keep only part of the glass, scan, then download or save the result. Everything
//! here talks to the server functions, so it needs JavaScript; the page says so without it.

use leptos::{ev, prelude::*, task::spawn_local};
use recto_scanning::{
    OutputFormat, ScanMode, ScanSettings, ScannerCapabilities, ScannerInfo, Selection,
};

use super::browser;
use crate::{
    branding::app_name,
    components::{
        Failure, error_text, failure,
        icons::{Icon, IconKind},
        ui::{Alert, ButtonVariant, Card, Field, HINT, INPUT, Tone, button_class},
    },
    display::unix_now,
    functions::{ScanOutcome, list_scanners, preview, scan, scanner_capabilities},
    t, tf, tp,
};

/// Reverse proxies give up on a request after a while (nginx after 60 seconds, Cloudflare
/// after 100) with a timeout or a 502 or 504 page, while recto may still be scanning. A failure
/// after this many seconds is taken for that; a quicker one is an ordinary network error.
const SLOW_REQUEST_SECONDS: i64 = 20;

/// What to say when a preview or scan that took `seconds` failed.
fn scan_error_text(error: &ServerFnError, seconds: i64) -> String {
    match failure(error) {
        Failure::Connection if seconds >= SLOW_REQUEST_SECONDS => {
            tf!("scan.errors.connection_lost", name = app_name())
        }
        _ => error_text(error),
    }
}

/// A drag shorter than this, as a fraction of the preview, is a stray tap rather than a
/// selection (the server ignores such selections too).
const MIN_SELECTION: f64 = 0.01;

/// The resolution offered first: 300 dpi, sharp enough for documents and text recognition and
/// still quick, else the closest below it, else the lowest.
fn default_resolution(resolutions: &[u32]) -> u32 {
    resolutions
        .iter()
        .copied()
        .filter(|resolution| *resolution <= 300)
        .max()
        .or_else(|| resolutions.iter().copied().min())
        .unwrap_or(300)
}

fn default_mode(modes: &[ScanMode]) -> ScanMode {
    if modes.contains(&ScanMode::Color) {
        ScanMode::Color
    } else {
        modes.first().copied().unwrap_or(ScanMode::Color)
    }
}

/// The rectangle between where a drag started and where the pointer is, as fractions.
fn rectangle(anchor: (f64, f64), point: (f64, f64)) -> Selection {
    Selection {
        x: anchor.0.min(point.0),
        y: anchor.1.min(point.1),
        width: (anchor.0 - point.0).abs(),
        height: (anchor.1 - point.1).abs(),
    }
}

fn mode_label(mode: ScanMode) -> &'static str {
    match mode {
        ScanMode::Color => t!("scan.modes.color"),
        ScanMode::Gray => t!("scan.modes.gray"),
        ScanMode::Lineart => t!("scan.modes.lineart"),
    }
}

const FORMATS: [OutputFormat; 3] = [OutputFormat::Pdf, OutputFormat::Png, OutputFormat::Jpeg];

fn format_label(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Pdf => t!("scan.formats.pdf"),
        OutputFormat::Png => t!("scan.formats.png"),
        OutputFormat::Jpeg => t!("scan.formats.jpeg"),
    }
}

/// What the island is waiting for, said in its live region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Activity {
    Idle,
    Refreshing,
    Loading,
    Previewing,
    Scanning,
    Saving,
}

impl Activity {
    fn message(self) -> &'static str {
        match self {
            Activity::Idle => "",
            Activity::Refreshing => t!("scan.activity.refreshing"),
            Activity::Loading => t!("scan.activity.loading"),
            Activity::Previewing => t!("scan.activity.previewing"),
            Activity::Scanning => t!("scan.activity.scanning"),
            Activity::Saving => t!("scan.activity.saving"),
        }
    }
}

/// The settings of the chosen scanner, offering only what it supports.
#[component]
fn Settings(
    capabilities: ScannerCapabilities,
    resolution: RwSignal<u32>,
    mode: RwSignal<ScanMode>,
    source: RwSignal<Option<String>>,
    format: RwSignal<OutputFormat>,
) -> impl IntoView {
    let feeder = {
        let sources = capabilities.sources.clone();
        move || {
            source.with(|chosen| {
                sources
                    .iter()
                    .any(|known| Some(&known.name) == chosen.as_ref() && known.is_feeder)
            })
        }
    };
    let resolutions = capabilities
        .resolutions
        .iter()
        .map(|&value| {
            view! {
                <option value=value.to_string() prop:selected=move || resolution.get() == value>
                    {tf!("scan.dpi", value = value)}
                </option>
            }
        })
        .collect_view();
    let modes = capabilities
        .modes
        .iter()
        .map(|&value| {
            view! {
                <option value=value.sane_name() prop:selected=move || mode.get() == value>
                    {mode_label(value)}
                </option>
            }
        })
        .collect_view();
    let sources = (!capabilities.sources.is_empty()).then(|| {
        let feeders = capabilities.sources.clone();
        let choices = capabilities
            .sources
            .iter()
            .map(|known| {
                let name = known.name.clone();
                let value = name.clone();
                let chosen = name.clone();
                view! {
                    <option
                        value=value
                        prop:selected=move || source.with(|source| source.as_deref() == Some(&chosen))
                    >
                        {name}
                    </option>
                }
            })
            .collect_view();
        view! {
            <Field label=t!("scan.source") input_id="source">
                <select
                    id="source"
                    class=INPUT
                    on:change:target=move |event| {
                        let name = event.target().value();
                        // A feeder gives several pages, which only PDF keeps.
                        if feeders.iter().any(|known| known.name == name && known.is_feeder) {
                            format.set(OutputFormat::Pdf);
                        }
                        source.set(Some(name));
                    }
                >
                    {choices}
                </select>
            </Field>
        }
    });
    let formats = FORMATS
        .into_iter()
        .map(|value| {
            view! {
                <option value=value.extension() prop:selected=move || format.get() == value>
                    {format_label(value)}
                </option>
            }
        })
        .collect_view();
    view! {
        <div class="grid gap-4 sm:grid-cols-2">
            <Field label=t!("scan.resolution") input_id="resolution">
                <select
                    id="resolution"
                    class=INPUT
                    on:change:target=move |event| {
                        if let Ok(value) = event.target().value().parse() {
                            resolution.set(value);
                        }
                    }
                >
                    {resolutions}
                </select>
            </Field>
            <Field label=t!("scan.mode") input_id="mode">
                <select
                    id="mode"
                    class=INPUT
                    on:change:target=move |event| {
                        if let Some(value) = ScanMode::from_sane(&event.target().value()) {
                            mode.set(value);
                        }
                    }
                >
                    {modes}
                </select>
            </Field>
            {sources}
            <Field label=t!("scan.format") input_id="format">
                <select
                    id="format"
                    class=INPUT
                    on:change:target=move |event| {
                        let chosen = event.target().value();
                        if let Some(value) = FORMATS.into_iter().find(|format| format.extension() == chosen) {
                            format.set(value);
                        }
                    }
                >
                    {formats}
                </select>
            </Field>
        </div>
        {move || {
            (feeder() && format.get() != OutputFormat::Pdf)
                .then(|| view! { <p class=HINT>{t!("scan.first_page_only")}</p> })
        }}
    }
    .into_any()
}

/// The scan controls. `scanners` is the list the server had when it rendered the page, and
/// `error` why it had none.
#[island]
pub fn Scanner(scanners: Vec<ScannerInfo>, error: Option<String>) -> impl IntoView {
    let first = scanners
        .first()
        .map(|scanner| scanner.id.clone())
        .unwrap_or_default();
    let scanners = RwSignal::new(scanners);
    let selected = RwSignal::new(first);
    let capabilities = RwSignal::new(None::<ScannerCapabilities>);
    let resolution = RwSignal::new(300);
    let mode = RwSignal::new(ScanMode::Color);
    let source = RwSignal::new(None::<String>);
    let format = RwSignal::new(OutputFormat::Pdf);
    let preview_image = RwSignal::new(None::<ScanOutcome>);
    let selection = RwSignal::new(None::<Selection>);
    let anchor = RwSignal::new(None::<(f64, f64)>);
    let result = RwSignal::new(None::<ScanOutcome>);
    let saved = RwSignal::new(None::<Result<String, String>>);
    let activity = RwSignal::new(Activity::Idle);
    let failure = RwSignal::new(error);
    // Controls stay disabled in the server's HTML, and until the island takes over.
    let ready = RwSignal::new(false);
    Effect::new(move |_| ready.set(true));

    let busy = move || activity.get() != Activity::Idle;
    let can_scan = move || ready.get() && !busy() && capabilities.with(Option::is_some);
    // A preview from a feeder would pull the first sheet through, so it needs the flatbed.
    let feeder_chosen = move || {
        capabilities.with(|capabilities| {
            source.with(|chosen| {
                capabilities.as_ref().is_some_and(|capabilities| {
                    capabilities
                        .sources
                        .iter()
                        .any(|known| Some(&known.name) == chosen.as_ref() && known.is_feeder)
                })
            })
        })
    };
    let selectable = move || {
        capabilities.with(|capabilities| {
            capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.area.is_some())
        })
    };

    // Each scanner has its own settings: load them whenever another one is chosen.
    Effect::new(move |_| {
        let id = selected.get();
        capabilities.set(None);
        preview_image.set(None);
        selection.set(None);
        result.set(None);
        if id.is_empty() {
            return;
        }
        activity.set(Activity::Loading);
        spawn_local(async move {
            let loaded = scanner_capabilities(id.clone()).await;
            // Another scanner was chosen meanwhile: its own request will answer.
            if selected.get_untracked() != id {
                return;
            }
            activity.set(Activity::Idle);
            match loaded {
                Ok(loaded) => {
                    resolution.set(default_resolution(&loaded.resolutions));
                    mode.set(default_mode(&loaded.modes));
                    let first_source = loaded.sources.first();
                    source.set(first_source.map(|source| source.name.clone()));
                    if first_source.is_some_and(|source| source.is_feeder) {
                        format.set(OutputFormat::Pdf);
                    }
                    failure.set(None);
                    capabilities.set(Some(loaded));
                }
                Err(error) => failure.set(Some(error_text(&error))),
            }
        });
    });

    let refresh = move |_| {
        activity.set(Activity::Refreshing);
        spawn_local(async move {
            let listed = list_scanners(true).await;
            activity.set(Activity::Idle);
            match listed {
                Ok(listed) => {
                    failure.set(None);
                    let current = selected.get_untracked();
                    if !listed.iter().any(|scanner| scanner.id == current) {
                        selected.set(
                            listed
                                .first()
                                .map(|scanner| scanner.id.clone())
                                .unwrap_or_default(),
                        );
                    } else if capabilities.with_untracked(Option::is_none) {
                        // Its settings failed to load (asleep, busy): setting it again, even to
                        // the same scanner, loads them anew now that it may be back.
                        selected.set(current);
                    }
                    scanners.set(listed);
                }
                Err(error) => failure.set(Some(error_text(&error))),
            }
        });
    };

    let run_preview = move |_| {
        let id = selected.get_untracked();
        let source = source.get_untracked();
        activity.set(Activity::Previewing);
        failure.set(None);
        spawn_local(async move {
            let started = unix_now();
            let outcome = preview(id, source).await;
            activity.set(Activity::Idle);
            match outcome {
                Ok(outcome) => {
                    selection.set(None);
                    preview_image.set(Some(outcome));
                }
                Err(error) => failure.set(Some(scan_error_text(&error, unix_now() - started))),
            }
        });
    };

    let run_scan = move |_| {
        let id = selected.get_untracked();
        let settings = ScanSettings {
            resolution: resolution.get_untracked(),
            mode: mode.get_untracked(),
            source: source.get_untracked(),
            // The server cuts the area from the selection and the scanner's geometry.
            area: None,
        };
        let chosen = selection.get_untracked();
        let format = format.get_untracked();
        activity.set(Activity::Scanning);
        failure.set(None);
        saved.set(None);
        spawn_local(async move {
            let started = unix_now();
            let outcome = scan(id, settings, chosen, format).await;
            activity.set(Activity::Idle);
            match outcome {
                Ok(outcome) => result.set(Some(outcome)),
                Err(error) => failure.set(Some(scan_error_text(&error, unix_now() - started))),
            }
        });
    };

    let save = move |_| {
        let Some(outcome) = result.get_untracked() else {
            return;
        };
        activity.set(Activity::Saving);
        saved.set(None);
        spawn_local(async move {
            let outcome = browser::save_scan(outcome.id).await;
            activity.set(Activity::Idle);
            saved.set(Some(outcome));
        });
    };

    let start_drag = move |event: ev::PointerEvent| {
        if !selectable() {
            return;
        }
        if let Some(point) = browser::point(&event) {
            browser::capture(&event);
            anchor.set(Some(point));
            selection.set(None);
        }
    };
    let drag = move |event: ev::PointerEvent| {
        if let (Some(start), Some(point)) = (anchor.get_untracked(), browser::point(&event)) {
            selection.set(Some(rectangle(start, point)));
        }
    };
    let end_drag = move |_: ev::PointerEvent| {
        anchor.set(None);
        let stray = selection.with_untracked(|selection| {
            selection.is_some_and(|selection| {
                selection.width < MIN_SELECTION || selection.height < MIN_SELECTION
            })
        });
        if stray {
            selection.set(None);
        }
    };

    let scanner_choices = move || {
        scanners
            .get()
            .into_iter()
            .map(|scanner| {
                let id = scanner.id.clone();
                view! {
                    <option value=scanner.id prop:selected=move || selected.get() == id>
                        {format!("{} {}", scanner.vendor, scanner.model)}
                    </option>
                }
            })
            .collect_view()
    };

    let preview_view = move || {
        preview_image.get().map(|image| {
            let overlay = move || {
                selection.get().map(|selection| {
                    view! {
                        <div
                            aria-hidden="true"
                            class="border-brand bg-brand/20 pointer-events-none absolute border-2"
                            style=format!(
                                "left:{:.2}%;top:{:.2}%;width:{:.2}%;height:{:.2}%",
                                selection.x * 100.0,
                                selection.y * 100.0,
                                selection.width * 100.0,
                                selection.height * 100.0,
                            )
                        ></div>
                    }
                })
            };
            view! {
                <Card title=t!("scan.preview_title")>
                    <div class="grid justify-items-start gap-3">
                        <div
                            class=move || {
                                format!(
                                    "relative w-fit max-w-full overflow-hidden rounded-md border select-none {}",
                                    if selectable() { "cursor-crosshair touch-none" } else { "" },
                                )
                            }
                            on:pointerdown=start_drag
                            on:pointermove=drag
                            on:pointerup=end_drag
                            on:pointercancel=end_drag
                        >
                            <img
                                src=format!("/scan/results/{}", image.id)
                                alt=t!("scan.preview_alt")
                                draggable="false"
                                class="block h-auto max-h-[70vh] w-auto max-w-full"
                            />
                            {overlay}
                        </div>
                        <div class="flex flex-wrap items-center gap-3">
                            <p class=HINT>
                                {move || {
                                    if !selectable() {
                                        t!("scan.no_area")
                                    } else if selection.with(Option::is_some) {
                                        t!("scan.selected")
                                    } else {
                                        t!("scan.select_hint")
                                    }
                                }}
                            </p>
                            <button
                                type="button"
                                class=button_class(ButtonVariant::Secondary)
                                disabled=move || selection.with(Option::is_none)
                                on:click=move |_| selection.set(None)
                            >
                                {t!("scan.clear_selection")}
                            </button>
                        </div>
                    </div>
                </Card>
            }
        })
    };

    let result_view = move || {
        result.get().map(|outcome| {
            let download = format!("/scan/results/{}/download", outcome.id);
            // Saved once is enough: a second save would be a second document for Paperless.
            let is_saved = move || saved.with(|saved| matches!(saved, Some(Ok(_))));
            let save_button = outcome.can_save.then(|| {
                view! {
                    <button
                        type="button"
                        class=button_class(ButtonVariant::Secondary)
                        disabled=move || busy() || is_saved()
                        on:click=save
                    >
                        {t!("scan.save")}
                    </button>
                }
            });
            view! {
                <Card title=t!("scan.result")>
                    <p class="font-medium break-all">{outcome.file_name.clone()}</p>
                    <p class=HINT>{tp!("scan.pages", outcome.pages)}</p>
                    <div class="mt-4 flex flex-wrap gap-2">
                        <a href=download download=outcome.file_name class=button_class(ButtonVariant::Primary)>
                            {t!("scan.download")}
                        </a>
                        {save_button}
                    </div>
                    <div role="status" class="mt-3 text-sm font-medium">
                        {move || {
                            saved
                                .get()
                                .map(|saved| match saved {
                                    Ok(path) => {
                                        view! { <p class="text-success">{tf!("scan.saved", path = path)}</p> }
                                            .into_any()
                                    }
                                    Err(message) => view! { <p class="text-danger">{message}</p> }.into_any(),
                                })
                        }}
                    </div>
                </Card>
            }
        })
    };

    view! {
        <div class="grid items-start gap-5 lg:grid-cols-[22rem_minmax(0,1fr)]">
            <Card>
                <div class="grid gap-4">
                    <div class="flex items-end gap-2">
                        <div class="min-w-0 flex-1">
                            <Field label=t!("scan.scanner") input_id="scanner">
                                <select
                                    id="scanner"
                                    class=INPUT
                                    disabled=move || !ready.get() || busy() || scanners.with(Vec::is_empty)
                                    on:change:target=move |event| selected.set(event.target().value())
                                >
                                    {scanner_choices}
                                </select>
                            </Field>
                        </div>
                        <button
                            type="button"
                            class=button_class(ButtonVariant::Secondary)
                            aria-label=t!("scan.refresh")
                            title=t!("scan.refresh")
                            disabled=move || !ready.get() || busy()
                            on:click=refresh
                        >
                            <Icon kind=IconKind::Refresh />
                        </button>
                    </div>
                    {move || {
                        scanners
                            .with(Vec::is_empty)
                            .then(|| view! { <p class=HINT>{t!("scan.no_scanners")}</p> })
                    }}
                    {move || {
                        capabilities
                            .get()
                            .map(|capabilities| {
                                view! { <Settings capabilities resolution mode source format /> }
                            })
                    }}
                    <div class="flex flex-wrap gap-2">
                        <button
                            type="button"
                            class=button_class(ButtonVariant::Secondary)
                            disabled=move || !can_scan() || feeder_chosen()
                            on:click=run_preview
                        >
                            {t!("scan.preview")}
                        </button>
                        <button
                            type="button"
                            class=button_class(ButtonVariant::Primary)
                            disabled=move || !can_scan()
                            on:click=run_scan
                        >
                            {t!("scan.scan")}
                        </button>
                    </div>
                    {move || {
                        feeder_chosen().then(|| view! { <p class=HINT>{t!("scan.preview_needs_flatbed")}</p> })
                    }}
                    <p aria-live="polite" class="text-muted-foreground text-sm empty:hidden">
                        {move || activity.get().message()}
                    </p>
                </div>
            </Card>
            <div class="grid gap-5">
                {move || failure.get().map(|message| view! { <Alert tone=Tone::Danger>{message}</Alert> })}
                {result_view}
                {preview_view}
            </div>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_suit_documents() {
        assert_eq!(default_resolution(&[75, 150, 300, 600]), 300);
        assert_eq!(default_resolution(&[100, 200, 400]), 200);
        assert_eq!(default_resolution(&[600, 1200]), 600);
        assert_eq!(default_resolution(&[]), 300);
        assert_eq!(
            default_mode(&[ScanMode::Gray, ScanMode::Color]),
            ScanMode::Color
        );
        assert_eq!(default_mode(&[ScanMode::Lineart]), ScanMode::Lineart);
    }

    #[test]
    fn slow_failures_point_at_a_proxy_timeout() {
        let gateway = ServerFnError::Deserialization("<html>504 Gateway Time-out</html>".into());
        assert!(scan_error_text(&gateway, 61).contains("may still be finishing"));
        let dropped = ServerFnError::Request("network error".into());
        assert!(scan_error_text(&dropped, 100).contains("read timeout"));
        // A quick failure is an ordinary network error, and recto's own refusals stay as they are.
        assert_eq!(
            scan_error_text(&dropped, 1),
            "Could not reach recto. Check your connection and try again."
        );
        let busy = ServerFnError::new("The scanner is busy.");
        assert_eq!(scan_error_text(&busy, 90), "The scanner is busy.");
    }

    #[test]
    fn drags_in_any_direction_make_the_same_rectangle() {
        let forward = rectangle((0.2, 0.1), (0.6, 0.5));
        let backward = rectangle((0.6, 0.5), (0.2, 0.1));
        assert_eq!(forward, backward);
        assert!((forward.x - 0.2).abs() < 1e-9 && (forward.width - 0.4).abs() < 1e-9);
        assert!((forward.y - 0.1).abs() < 1e-9 && (forward.height - 0.4).abs() < 1e-9);
    }
}
