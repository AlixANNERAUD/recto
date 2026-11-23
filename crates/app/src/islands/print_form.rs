//! The print form. Rendered on the server it is a plain multipart form posting to `/print`, with
//! the options of the preselected printer; once the island runs, choosing another printer
//! updates the option fields and the status card, and a file over the upload limit is caught
//! before it is sent.

use leptos::prelude::*;
use recto_printing::{Capabilities, Printer, Sides};

use super::browser;
use crate::{
    components::{
        printer::{PrinterStatus, color_label, option_label, sides_label},
        ui::{ButtonVariant, Card, FILE_INPUT, Field, INPUT, button_class},
    },
    display::file_size,
    t, tf,
};

/// The printer chosen when the page opens: the CUPS default, else the first one.
fn preselected(printers: &[Printer]) -> String {
    printers
        .iter()
        .find(|printer| printer.is_default)
        .or_else(|| printers.first())
        .map(|printer| printer.name.clone())
        .unwrap_or_default()
}

/// Copies, duplex and color, each only when the printer offers a choice. A missing field means
/// the printer's default to the server.
#[component]
fn OptionFields(capabilities: Capabilities) -> impl IntoView {
    let copies = (capabilities.max_copies > 1).then(|| {
        view! {
            <Field label=t!("print.copies") input_id="copies">
                <input
                    id="copies"
                    name="copies"
                    type="number"
                    inputmode="numeric"
                    min="1"
                    max=capabilities.max_copies.to_string()
                    value="1"
                    required
                    class=INPUT
                />
            </Field>
        }
    });
    let two_sided: Vec<Sides> = capabilities
        .sides
        .iter()
        .copied()
        .filter(|sides| *sides != Sides::OneSided)
        .collect();
    let sides = (!two_sided.is_empty()).then(|| {
        let choices = std::iter::once(Sides::OneSided)
            .chain(two_sided)
            .map(|sides| view! { <option value=sides.keyword()>{sides_label(sides)}</option> })
            .collect_view();
        view! {
            <Field label=t!("print.sides.label") input_id="sides">
                <select id="sides" name="sides" class=INPUT>
                    {choices}
                </select>
            </Field>
        }
    });
    let color = (capabilities.color_modes.len() > 1).then(|| {
        let choices = capabilities
            .color_modes
            .iter()
            .map(|mode| view! { <option value=mode.keyword()>{color_label(*mode)}</option> })
            .collect_view();
        view! {
            <Field label=t!("print.color.label") input_id="color_mode">
                <select id="color_mode" name="color_mode" class=INPUT>
                    {choices}
                </select>
            </Field>
        }
    });
    view! { <div class="grid gap-4 sm:grid-cols-3">{copies} {sides} {color}</div> }.into_any()
}

#[island]
pub fn PrintForm(printers: Vec<Printer>, max_upload_size: usize) -> impl IntoView {
    let initial = preselected(&printers);
    let selected = RwSignal::new(initial.clone());
    let printers = StoredValue::new(printers);
    let current = Memo::new(move |_| {
        let name = selected.get();
        printers.with_value(|printers| {
            printers
                .iter()
                .find(|printer| printer.name == name)
                .cloned()
        })
    });
    let too_large = RwSignal::new(false);
    let is_image = RwSignal::new(false);
    let on_file = move |event: leptos::ev::Event| {
        if let Some((large, image)) = browser::check_file(&event, max_upload_size) {
            too_large.set(large);
            is_image.set(image);
        }
    };
    let choices = printers.with_value(|printers| {
        printers
            .iter()
            .map(|printer| {
                let chosen = printer.name == initial;
                view! {
                    <option value=printer.name.clone() selected=chosen>
                        {option_label(printer)}
                    </option>
                }
            })
            .collect_view()
    });
    view! {
        <div class="grid items-start gap-5 lg:grid-cols-[minmax(0,1fr)_20rem]">
            <Card>
                <form method="post" action="/print" enctype="multipart/form-data" class="grid gap-5">
                    <Field
                        label=t!("print.file")
                        input_id="file"
                        hint=tf!("print.file_hint", size = file_size(max_upload_size))
                    >
                        <input
                            id="file"
                            name="file"
                            type="file"
                            accept=".pdf,.jpg,.jpeg,.png"
                            required
                            aria-describedby="file-hint"
                            class=FILE_INPUT
                            on:change=on_file
                        />
                    </Field>
                    <p role="alert" class="text-danger -mt-3 text-sm font-medium empty:hidden">
                        {move || too_large.get().then(|| t!("print.errors.too_large"))}
                    </p>
                    <Field label=t!("print.printer") input_id="printer">
                        // Not restored by the browser on reload or back: the options and
                        // the status below follow `selected`, which only a change updates.
                        <select
                            id="printer"
                            name="printer"
                            class=INPUT
                            autocomplete="off"
                            on:change:target=move |event| selected.set(event.target().value())
                        >
                            {choices}
                        </select>
                    </Field>
                    {move || {
                        current
                            .get()
                            .map(|printer| view! { <OptionFields capabilities=printer.capabilities /> })
                    }}
                    // Images are one page; the field comes back if a PDF is chosen again.
                    <Show when=move || !is_image.get()>
                        <Field
                            label=t!("print.pages.label")
                            input_id="page_ranges"
                            hint=t!("print.pages.hint")
                        >
                            <input
                                id="page_ranges"
                                name="page_ranges"
                                type="text"
                                inputmode="numeric"
                                autocomplete="off"
                                placeholder=t!("print.pages.placeholder")
                                aria-describedby="page_ranges-hint"
                                class=INPUT
                            />
                        </Field>
                    </Show>
                    <div>
                        <button type="submit" class=button_class(ButtonVariant::Primary)>
                            {t!("print.submit")}
                        </button>
                    </div>
                </form>
            </Card>
            <section aria-label=t!("print.status") class="bg-card rounded-lg border p-4 shadow-[0_1px_2px_rgba(0,0,0,0.08)] md:p-6">
                {move || current.get().map(|printer| view! { <PrinterStatus printer /> })}
            </section>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use recto_printing::PrinterState;

    use super::*;

    fn printer(name: &str, is_default: bool) -> Printer {
        Printer {
            name: name.into(),
            description: None,
            location: None,
            make_and_model: None,
            is_default,
            state: PrinterState::Idle,
            reasons: vec![],
            accepting_jobs: true,
            supplies: vec![],
            capabilities: Capabilities {
                max_copies: 1,
                sides: vec![],
                color_modes: vec![],
            },
        }
    }

    #[test]
    fn the_default_printer_is_preselected() {
        let printers = [printer("a", false), printer("b", true)];
        assert_eq!(preselected(&printers), "b");
        assert_eq!(preselected(&printers[..1]), "a");
        assert_eq!(preselected(&[]), "");
    }
}
