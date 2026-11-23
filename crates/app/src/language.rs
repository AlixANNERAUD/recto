//! The language switch: one button per embedded locale, posting to `/language`, which stores the
//! choice in the locale cookie ([`i18n::COOKIE`]) and returns to the page. A plain form, so it
//! works without JavaScript, like the theme switch.

use leptos::prelude::*;

use crate::{
    i18n, t,
    theme::{Switch, choice_class},
};

/// The language buttons, each labelled with its tag and named in its own language. On phones,
/// where the header is short of room, the current language's button is hidden: the others are
/// what there is to press.
#[component]
pub fn LanguageSwitch() -> impl IntoView {
    let current = i18n::locale();
    view! {
        <Switch action="/language" label=t!("language.label")>
            {i18n::languages()
                .map(|(locale, tag, name)| {
                    let pressed = locale == current;
                    view! {
                        <button
                            type="submit"
                            name="locale"
                            value=tag
                            lang=tag
                            aria-label=name
                            title=name
                            aria-pressed=pressed.to_string()
                            class=format!(
                                "{} h-8 min-w-8 items-center justify-center px-1 text-xs font-medium uppercase {}",
                                if pressed { "hidden sm:inline-flex" } else { "inline-flex" },
                                choice_class(pressed),
                            )
                        >
                            {tag}
                        </button>
                    }
                })
                .collect_view()}
        </Switch>
    }
}
