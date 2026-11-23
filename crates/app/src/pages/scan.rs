//! `/scan`: the scan island, with the scanners the server knows when it renders the page.

use leptos::prelude::*;

use crate::{
    components::{
        error_text,
        ui::{Alert, Tone},
    },
    functions::list_scanners,
    islands::scan::Scanner,
    layout::{Layout, is_administrator},
    t,
};

#[component]
pub fn ScanPage() -> impl IntoView {
    let scanners = OnceResource::new(list_scanners(false));
    view! {
        <Layout title=t!("scan.title")>
            <noscript>
                <Alert tone=Tone::Warning>{t!("scan.needs_javascript")}</Alert>
            </noscript>
            <Suspense>
                {move || Suspend::new(async move {
                    // Without scanners the island still renders, so Refresh can find them.
                    let (scanners, error) = match scanners.await {
                        Ok(scanners) => (scanners, None),
                        Err(error) => (Vec::new(), Some(error_text(&error))),
                    };
                    let administrator = is_administrator();
                    view! { <Scanner scanners error administrator /> }
                })}
            </Suspense>
        </Layout>
    }
}
