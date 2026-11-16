//! The operator's branding: the name, the accent color, the logo and the icons. The server loads
//! it once at startup (crate `recto`, module `branding`) and hands it to pages in [`AppState`].
//! Islands only need the name, which they read back from the page.
//!
//! [`AppState`]: crate::state::AppState

use leptos::prelude::*;

/// The name when none is configured.
pub const DEFAULT_NAME: &str = "recto";

#[cfg(feature = "ssr")]
pub use server::*;

#[cfg(feature = "ssr")]
mod server {
    use std::sync::{Arc, LazyLock};

    use axum::body::Bytes;

    /// The accent color when none is configured, the Fluent brand blue of the stylesheet.
    pub const DEFAULT_COLOR: &str = "#0f6cbd";

    /// What the operator configured, checked and with its derived parts computed.
    #[derive(Clone, Debug)]
    pub struct Branding {
        /// The header, the document titles and the manifest's name.
        pub name: String,
        /// Where space is short: the manifest's `short_name` and the home screen label.
        pub short_name: String,
        /// The accent, as `#rrggbb`: the browser's `theme-color` and the manifest's.
        pub color: String,
        /// CSS overriding the stylesheet's accent tokens; `None` for the default color, whose
        /// tokens the stylesheet already holds.
        pub style: Option<String>,
        pub logo: Option<BrandFile>,
        pub icons: BrandIcons,
    }

    /// A file served as is, with the type read from its content.
    #[derive(Clone)]
    pub struct BrandFile {
        pub bytes: Bytes,
        pub content_type: &'static str,
    }

    /// The application icons, PNG.
    #[derive(Clone)]
    pub struct BrandIcons {
        pub any_192: Bytes,
        pub any_512: Bytes,
        /// Drawn to be cropped to any shape: the artwork keeps to the middle 80 %.
        pub maskable_512: Bytes,
        pub apple_180: Bytes,
    }

    // Byte counts rather than the bytes, which would bury a failed assertion's message.
    impl std::fmt::Debug for BrandFile {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("BrandFile")
                .field("bytes", &self.bytes.len())
                .field("content_type", &self.content_type)
                .finish()
        }
    }

    impl std::fmt::Debug for BrandIcons {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("BrandIcons")
                .field("any_192", &self.any_192.len())
                .field("any_512", &self.any_512.len())
                .field("maskable_512", &self.maskable_512.len())
                .field("apple_180", &self.apple_180.len())
                .finish()
        }
    }

    /// The branding of the current request. Outside a request, which never renders a page,
    /// the default name and color without icons.
    pub fn branding() -> Arc<Branding> {
        static UNBRANDED: LazyLock<Arc<Branding>> = LazyLock::new(|| {
            Arc::new(Branding {
                name: super::DEFAULT_NAME.to_owned(),
                short_name: super::DEFAULT_NAME.to_owned(),
                color: DEFAULT_COLOR.to_owned(),
                style: None,
                logo: None,
                icons: BrandIcons {
                    any_192: Bytes::new(),
                    any_512: Bytes::new(),
                    maskable_512: Bytes::new(),
                    apple_180: Bytes::new(),
                },
            })
        });
        crate::state::app_state().map_or_else(|| Arc::clone(&UNBRANDED), |state| state.branding)
    }
}

/// The brand in the document head, after the stylesheet so that the accent's style wins: the
/// names, the browser's color, the icons. Islands read the name back from `application-name`.
#[cfg(feature = "ssr")]
pub fn head() -> impl IntoView {
    let branding = branding();
    let style = branding
        .style
        .clone()
        .map(|css| view! { <style>{css}</style> });
    view! {
        <meta name="application-name" content=branding.name.clone() />
        <meta name="apple-mobile-web-app-title" content=branding.short_name.clone() />
        <meta name="theme-color" content=branding.color.clone() />
        <link rel="icon" href="/brand/icon-192.png" sizes="192x192" type="image/png" />
        <link rel="apple-touch-icon" href="/brand/apple-touch-icon.png" />
        {style}
    }
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
pub fn head() -> impl IntoView {}

/// Whether the operator gave a logo, served at `/brand/logo`.
#[cfg(feature = "ssr")]
pub fn has_logo() -> bool {
    branding().logo.is_some()
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
pub fn has_logo() -> bool {
    false
}

/// The name of the app, for messages that mention it.
#[cfg(feature = "ssr")]
pub fn app_name() -> String {
    branding().name.clone()
}

/// The name of the app, which the server wrote into `<meta name="application-name">`.
#[cfg(not(feature = "ssr"))]
pub fn app_name() -> String {
    document()
        .query_selector(r#"meta[name="application-name"]"#)
        .ok()
        .flatten()
        .and_then(|meta| meta.get_attribute("content"))
        .unwrap_or_else(|| DEFAULT_NAME.to_owned())
}
