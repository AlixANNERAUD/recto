//! `/printers`: every printer with its state, reasons and supplies.

use leptos::prelude::*;

use super::failure_alert;
use crate::{
    components::{
        printer::PrinterStatus,
        ui::{Card, Empty},
    },
    functions::list_printers,
    layout::Layout,
    t,
};

#[component]
pub fn PrintersPage() -> impl IntoView {
    let printers = OnceResource::new(list_printers());
    view! {
        <Layout title=t!("printers.title")>
            <Suspense>
                {move || Suspend::new(async move {
                    match printers.await {
                        Ok(printers) if printers.is_empty() => {
                            view! { <Empty title=t!("printers.empty") body=t!("printers.empty_body") /> }
                                .into_any()
                        }
                        Ok(printers) => {
                            view! {
                                <div class="grid gap-4 md:grid-cols-2">
                                    {printers
                                        .into_iter()
                                        .map(|printer| view! { <Card><PrinterStatus printer /></Card> })
                                        .collect_view()}
                                </div>
                            }
                                .into_any()
                        }
                        Err(error) => failure_alert(&error).into_any(),
                    }
                })}
            </Suspense>
        </Layout>
    }
}
