//! The administration pages, for administrators only: `/administration/users` and
//! `/administration/groups`, which `/administration` leads to. The server loads their data, checks that the user administers
//! recto, and hands each page what it shows as a context; anyone else gets the not-found page,
//! so the pages do not reveal that they exist. Every change is a plain form posted to the
//! server, which works without JavaScript.

pub mod groups;
pub mod users;

use leptos::prelude::*;

use crate::t;

/// The administration pages, which link to each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Users,
    Groups,
}

/// The links between the administration pages, as tabs under the page heading.
#[component]
fn Sections(current: Section) -> impl IntoView {
    let tab = |section: Section, href: &'static str, label: &'static str| {
        let selected = section == current;
        view! {
            <li>
                <a
                    href=href
                    aria-current=selected.then_some("page")
                    class=format!(
                        "-mb-px inline-flex min-h-10 items-center border-b-2 px-3 text-sm font-semibold transition-colors {}",
                        if selected {
                            "border-brand text-brand"
                        } else {
                            "text-muted-foreground hover:text-foreground border-transparent"
                        },
                    )
                >
                    {label}
                </a>
            </li>
        }
    };
    view! {
        <nav aria-label=t!("administration.sections")>
            <ul class="flex gap-1 border-b">
                {tab(Section::Users, "/administration/users", t!("administration.users.tab"))}
                {tab(Section::Groups, "/administration/groups", t!("administration.groups.tab"))}
            </ul>
        </nav>
    }
}

/// The page's data, when the signed-in user administers recto and the server provided it.
#[cfg(feature = "ssr")]
fn page_data<T: Clone + Send + Sync + 'static>() -> Option<(i64, T)> {
    let user = crate::state::current_user().filter(|user| user.is_administrator)?;
    Some((user.id, use_context::<T>()?))
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn page_data<T>() -> Option<(i64, T)> {
    None
}
