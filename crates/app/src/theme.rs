//! The color theme: follow the device, or light, or dark. The choice lives in a cookie, which
//! the server reads to render `<html data-theme>`, so the first paint already has the right
//! colors and nothing needs JavaScript. Without a choice the attribute is absent and the
//! stylesheet follows `prefers-color-scheme`.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::{
    components::icons::{Icon, IconKind},
    t,
};

/// Name of the cookie remembering the chosen theme.
pub const COOKIE: &str = "recto_theme";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Follow the device's setting.
    System,
    Light,
    Dark,
}

impl Theme {
    pub fn parse(value: &str) -> Option<Theme> {
        match value {
            "system" => Some(Theme::System),
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    /// The value of `<html data-theme>`; none for System, so the media query decides.
    pub fn attribute(self) -> Option<&'static str> {
        (self != Theme::System).then(|| self.as_str())
    }

    /// The value of the `color-scheme` meta tag, which tells the browser which colors to use
    /// for its own controls and the page background before the stylesheet loads.
    pub fn color_scheme(self) -> &'static str {
        match self {
            Theme::System => "light dark",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }
}

/// The theme of a request: the last valid value of the theme cookie across all `Cookie`
/// headers, else System.
#[cfg(feature = "ssr")]
pub fn from_headers(headers: &axum::http::HeaderMap) -> Theme {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .filter(|(name, _)| *name == COOKIE)
        .filter_map(|(_, value)| Theme::parse(value))
        .next_back()
        .unwrap_or(Theme::System)
}

/// The theme of the current request. Only the server renders pages; islands never need it.
#[cfg(feature = "ssr")]
pub fn current() -> Theme {
    use_context::<axum::http::request::Parts>()
        .map_or(Theme::System, |parts| from_headers(&parts.headers))
}

#[cfg(not(feature = "ssr"))]
pub fn current() -> Theme {
    Theme::System
}

/// Three buttons posting to `/theme`, which stores the choice and returns to this page. A plain
/// form, so it works without JavaScript.
#[component]
pub fn ThemeSwitch() -> impl IntoView {
    let location = use_location();
    let next = format!(
        "{}{}",
        location.pathname.get_untracked(),
        location.search.get_untracked()
    );
    let current = current();
    let button = |theme: Theme, icon: IconKind, label: &'static str| {
        let pressed = current == theme;
        view! {
            <button
                type="submit"
                name="theme"
                value=theme.as_str()
                aria-label=label
                title=label
                aria-pressed=pressed.to_string()
                class=format!(
                    "inline-flex size-8 items-center justify-center rounded-sm hover:cursor-pointer {}",
                    if pressed { "bg-muted text-foreground" } else { "text-muted-foreground hover:text-foreground" },
                )
            >
                <Icon kind=icon class="size-4" />
            </button>
        }
    };
    view! {
        <form method="post" action="/theme">
            <input type="hidden" name="next" value=next />
            <div role="group" aria-label=t!("theme.label") class="flex items-center gap-0.5 rounded-md border p-0.5">
                {button(Theme::System, IconKind::Desktop, t!("theme.system"))}
                {button(Theme::Light, IconKind::Sun, t!("theme.light"))}
                {button(Theme::Dark, IconKind::Moon, t!("theme.dark"))}
            </div>
        </form>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::*;

    fn cookie(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.append(header::COOKIE, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn reads_the_last_valid_cookie() {
        assert_eq!(from_headers(&cookie("recto_theme=dark")), Theme::Dark);
        assert_eq!(
            from_headers(&cookie("a=1; recto_theme=light; b=2")),
            Theme::Light
        );
        assert_eq!(from_headers(&cookie("recto_theme=purple")), Theme::System);
        assert_eq!(from_headers(&cookie("recto_theme=")), Theme::System);
        assert_eq!(
            from_headers(&cookie("recto_theme=dark; recto_theme=light")),
            Theme::Light
        );
        assert_eq!(from_headers(&HeaderMap::new()), Theme::System);
    }

    #[test]
    fn reads_across_cookie_headers() {
        let mut headers = cookie("recto_theme=dark");
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("recto_theme=light"),
        );
        assert_eq!(from_headers(&headers), Theme::Light);
    }

    #[test]
    fn system_has_no_attribute() {
        assert_eq!(Theme::System.attribute(), None);
        assert_eq!(Theme::Dark.color_scheme(), "dark");
    }
}
