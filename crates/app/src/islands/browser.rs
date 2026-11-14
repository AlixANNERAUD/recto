//! The browser APIs the islands use. Islands are also rendered on the server, where their event
//! handlers exist but never run, so each function has an inert server twin with the same
//! signature and the islands need no `cfg` of their own.

use leptos::ev;
use uuid::Uuid;

#[cfg(feature = "hydrate")]
mod imp {
    use leptos::{ev, prelude::*};
    use uuid::Uuid;
    use wasm_bindgen::{JsCast, JsValue, closure::Closure};
    use wasm_bindgen_futures::JsFuture;

    use crate::t;

    pub fn check_file(event: &ev::Event, max_size: usize) -> Option<(bool, bool)> {
        let input = event
            .target()?
            .dyn_into::<web_sys::HtmlInputElement>()
            .ok()?;
        let Some(file) = input.files().and_then(|files| files.get(0)) else {
            input.set_custom_validity("");
            return Some((false, false));
        };
        // Sizes are whole bytes, far below 2^53, so the float is exact.
        #[allow(clippy::cast_precision_loss)]
        let too_large = file.size() > max_size as f64;
        // The browser keeps the form from being sent while the input is invalid.
        input.set_custom_validity(if too_large {
            t!("print.errors.too_large")
        } else {
            ""
        });
        Some((too_large, file.type_().starts_with("image/")))
    }

    fn element(event: &ev::PointerEvent) -> Option<web_sys::Element> {
        event.current_target()?.dyn_into::<web_sys::Element>().ok()
    }

    pub fn point(event: &ev::PointerEvent) -> Option<(f64, f64)> {
        let rect = element(event)?.get_bounding_client_rect();
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let x = (f64::from(event.client_x()) - rect.left()) / rect.width();
        let y = (f64::from(event.client_y()) - rect.top()) / rect.height();
        Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
    }

    pub fn capture(event: &ev::PointerEvent) {
        // No native image drag or text selection while drawing the rectangle.
        event.prevent_default();
        if let Some(element) = element(event) {
            let _ = element.set_pointer_capture(event.pointer_id());
        }
    }

    pub fn page_hidden() -> bool {
        document().hidden()
    }

    pub fn on_visible(callback: impl Fn() + 'static) {
        let listener = Closure::<dyn Fn()>::new(move || {
            if !document().hidden() {
                callback();
            }
        });
        let _ = document().add_event_listener_with_callback(
            "visibilitychange",
            listener.as_ref().unchecked_ref(),
        );
        // Listens for as long as the page is open, like the refresh interval.
        listener.forget();
    }

    pub async fn save_scan(id: Uuid) -> Result<String, String> {
        let failed = || t!("common.network_error").to_string();
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        // A same-origin fetch sends the session cookie, and the browser adds the `Origin` and
        // `Sec-Fetch-Site` headers the server's cross-site check looks for.
        let request = window().fetch_with_str_and_init(&format!("/scan/results/{id}/save"), &init);
        let response: web_sys::Response = JsFuture::from(request)
            .await
            .ok()
            .and_then(|response| response.dyn_into().ok())
            .ok_or_else(failed)?;
        let body = match response.json() {
            Ok(body) => JsFuture::from(body).await.map_err(|_| failed())?,
            Err(_) => return Err(failed()),
        };
        let field = |name: &str| {
            js_sys::Reflect::get(&body, &JsValue::from_str(name))
                .ok()
                .and_then(|value| value.as_string())
        };
        if response.ok() {
            field("path").ok_or_else(failed)
        } else {
            Err(field("error").unwrap_or_else(failed))
        }
    }
}

/// Checks the chosen file against the upload limit, blocking the form when it is too large:
/// browsers may show a reset connection instead of the server's refusal for a large upload.
/// Returns `(too_large, is_image)`, or `None` when the event is not from a file input.
pub fn check_file(event: &ev::Event, max_size: usize) -> Option<(bool, bool)> {
    #[cfg(feature = "hydrate")]
    {
        imp::check_file(event, max_size)
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (event, max_size);
        None
    }
}

/// Where a pointer is on the element listening to it, as fractions of its width and height.
pub fn point(event: &ev::PointerEvent) -> Option<(f64, f64)> {
    #[cfg(feature = "hydrate")]
    {
        imp::point(event)
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = event;
        None
    }
}

/// Keeps the pointer's events on the listening element until it is released, so a drag that
/// leaves the preview still ends there, and stops the browser's own dragging and scrolling.
pub fn capture(event: &ev::PointerEvent) {
    #[cfg(feature = "hydrate")]
    imp::capture(event);
    #[cfg(not(feature = "hydrate"))]
    let _ = event;
}

/// Asks the server to save a scan to the output folder: the saved path, or why it failed.
pub async fn save_scan(id: Uuid) -> Result<String, String> {
    #[cfg(feature = "hydrate")]
    {
        imp::save_scan(id).await
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = id;
        Err(String::new())
    }
}

/// Whether the page is in a background tab or minimised.
pub fn page_hidden() -> bool {
    #[cfg(feature = "hydrate")]
    {
        imp::page_hidden()
    }
    #[cfg(not(feature = "hydrate"))]
    {
        false
    }
}

/// Runs `callback` each time the page becomes visible again.
pub fn on_visible(callback: impl Fn() + 'static) {
    #[cfg(feature = "hydrate")]
    imp::on_visible(callback);
    #[cfg(not(feature = "hydrate"))]
    let _ = callback;
}

/// Runs `tick` every `period` for as long as the page is open.
pub fn every(period: std::time::Duration, tick: impl Fn() + 'static) {
    #[cfg(feature = "hydrate")]
    leptos::prelude::set_interval(tick, period);
    #[cfg(not(feature = "hydrate"))]
    let _ = (period, tick);
}
