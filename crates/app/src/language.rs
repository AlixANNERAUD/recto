//! The language switch: one button per embedded locale, posting to `/language`, which stores the
//! choice in the locale cookie ([`i18n::COOKIE`]) and returns to the page. A plain form, so it
//! works without JavaScript, like the theme switch.

use leptos::prelude::*;

use crate::{i18n, t, theme::return_path};

/// The language buttons, each labelled with its tag and named in its own language.
#[component]
pub fn LanguageSwitch() -> impl IntoView {
    let next = return_path();
    let current = i18n::locale();
    view! {
        <form method="post" action="/language">
            <input type="hidden" name="next" value=next />
            <div role="group" aria-label=t!("language.label") class="flex items-center gap-0.5 rounded-md border p-0.5">
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
                                    "inline-flex h-8 min-w-8 items-center justify-center rounded-sm px-1 text-xs font-medium uppercase hover:cursor-pointer {}",
                                    if pressed { "bg-muted text-foreground" } else { "text-muted-foreground hover:text-foreground" },
                                )
                            >
                                {tag}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>
        </form>
    }
}
