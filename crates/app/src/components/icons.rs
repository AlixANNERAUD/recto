//! Line icons drawn in the current text color. They are decorative: every icon sits next to a
//! visible label or inside a control that has an accessible name.

use leptos::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    /// A document with an arrow: sending a file to print.
    Print,
    Printer,
    Jobs,
    Scan,
    SignOut,
    Refresh,
    /// A person: the user's account.
    Account,
}

impl IconKind {
    fn paths(self) -> &'static [&'static str] {
        match self {
            IconKind::Print => &["M7 3h7l5 5v13H7z", "M14 3v5h5", "M13 18v-6m-3 3 3-3 3 3"],
            IconKind::Printer => &[
                "M7 9V3h10v6",
                "M7 17H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2h-2",
                "M7 14h10v7H7z",
            ],
            IconKind::Jobs => &[
                "M9 6h11M9 12h11M9 18h11",
                "M4.5 6h.01M4.5 12h.01M4.5 18h.01",
            ],
            IconKind::Scan => &[
                "M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3",
                "M4 12h16",
            ],
            IconKind::SignOut => &[
                "M9 20H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h3",
                "M16 16l4-4-4-4",
                "M20 12H10",
            ],
            IconKind::Refresh => &["M20 12a8 8 0 1 1-2.34-5.66", "M20 4v4h-4"],
            IconKind::Account => &[
                "M12 12a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
                "M4 20a8 8 0 0 1 16 0",
            ],
        }
    }
}

#[component]
pub fn Icon(kind: IconKind, #[prop(optional, into)] class: String) -> impl IntoView {
    let size = if class.is_empty() {
        "size-5"
    } else {
        class.as_str()
    };
    view! {
        <svg
            viewBox="0 0 24 24"
            aria-hidden="true"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
            class=format!("shrink-0 {size}")
        >
            {kind.paths().iter().map(|path| view! { <path d=*path /> }).collect_view()}
        </svg>
    }
}
