//! The color theme: follow the device, or light, or dark. The choice lives in a cookie, which
//! the server reads to render `<html data-theme>`, so the first paint already has the right
//! colors and nothing needs JavaScript. Without a choice the attribute is absent and the
//! stylesheet follows `prefers-color-scheme`.

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::{
    coded::coded,
    components::icons::{Icon, IconKind},
    t,
};

/// Name of the cookie remembering the chosen theme.
pub const COOKIE: &str = "recto_theme";

coded! {
    /// The code is the cookie's value and what the theme form posts.
    pub enum Theme {
        /// Follow the device's setting.
        System = "system",
        Light = "light",
        Dark = "dark",
    }
}

impl Theme {
    /// The value of `<html data-theme>`; none for System, so the media query decides.
    pub fn attribute(self) -> Option<&'static str> {
        (self != Theme::System).then(|| self.code())
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
    crate::cookies::values(headers, COOKIE)
        .filter_map(Theme::from_code)
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

/// The current page's path and query, for the `next` field of the switches,
/// which return to it.
fn return_path() -> String {
    let location = use_location();
    // The router's search has no leading `?` when the server renders the page.
    let search = location.search.get_untracked();
    let separator = if search.is_empty() || search.starts_with('?') {
        ""
    } else {
        "?"
    };
    format!("{}{separator}{search}", location.pathname.get_untracked())
}

/// Three buttons posting to `/theme`, which stores the choice and returns to this page.
#[component]
pub fn ThemeSwitch() -> impl IntoView {
    let current = current();
    let button = move |theme: Theme, icon: IconKind, label: &'static str| {
        let pressed = current == theme;
        view! {
            <button
                type="submit"
                name="theme"
                value=theme.code()
                aria-label=label
                title=label
                aria-pressed=pressed.to_string()
                class=format!("inline-flex size-8 items-center justify-center {}", choice_class(pressed))
            >
                <Icon kind=icon class="size-4" />
            </button>
        }
    };
    view! {
        <Switch action="/theme" label=t!("theme.label")>
            {button(Theme::System, IconKind::Desktop, t!("theme.system"))}
            {button(Theme::Light, IconKind::Sun, t!("theme.light"))}
            {button(Theme::Dark, IconKind::Moon, t!("theme.dark"))}
        </Switch>
    }
}

/// A switch of the header (theme, language): a group of buttons posting one choice to `action`,
/// which stores it and returns to this page. A plain form, so it works without JavaScript.
#[component]
pub(crate) fn Switch(
    action: &'static str,
    label: &'static str,
    children: Children,
) -> impl IntoView {
    view! {
        <form method="post" action=action>
            <input type="hidden" name="next" value=return_path() />
            <div role="group" aria-label=label class="flex items-center gap-0.5 rounded-md border p-0.5">
                {children()}
            </div>
        </form>
    }
}

/// The classes shared by a switch's buttons: the pressed one stands out.
pub(crate) fn choice_class(pressed: bool) -> &'static str {
    if pressed {
        "rounded-sm hover:cursor-pointer bg-muted text-foreground"
    } else {
        "rounded-sm hover:cursor-pointer text-muted-foreground hover:text-foreground"
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
