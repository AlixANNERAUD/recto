//! Interface primitives in the spirit of Fluent 2 (Thaw's look, without depending on it):
//! buttons, fields, cards, badges and alerts. They are plain class strings and components, so
//! they render on the server and inside islands alike.

use leptos::prelude::*;

const BUTTON_BASE: &str = "inline-flex items-center justify-center gap-2 rounded-md text-sm font-semibold whitespace-nowrap transition-colors hover:cursor-pointer disabled:cursor-not-allowed disabled:opacity-50";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ButtonVariant {
    /// The one main action of a form.
    #[default]
    Primary,
    /// Other actions: an outlined button on the card.
    Secondary,
    /// Actions in a row of controls, without a frame until hovered.
    Subtle,
}

pub fn button_class(variant: ButtonVariant) -> String {
    format!("{BUTTON_BASE} min-h-10 px-4 {}", button_colors(variant))
}

/// A smaller button, for actions repeated on every row of a table.
pub fn small_button_class(variant: ButtonVariant) -> String {
    format!("{BUTTON_BASE} min-h-8 px-3 {}", button_colors(variant))
}

fn button_colors(variant: ButtonVariant) -> &'static str {
    match variant {
        ButtonVariant::Primary => {
            "bg-primary text-primary-foreground hover:bg-primary-hover disabled:hover:bg-primary"
        }
        ButtonVariant::Secondary => "border border-input bg-card text-foreground hover:bg-muted",
        ButtonVariant::Subtle => "text-foreground hover:bg-muted",
    }
}

/// Text, number and select controls. `text-base` on phones keeps iOS from zooming in on focus.
pub const INPUT: &str = "block min-h-10 w-full min-w-0 rounded-md border border-input bg-card px-3 py-2 text-base text-foreground placeholder:text-muted-foreground disabled:opacity-50 md:text-sm";

pub const FILE_INPUT: &str =
    "block w-full min-w-0 text-sm text-muted-foreground file:hover:cursor-pointer";

pub const LABEL: &str = "text-sm font-semibold";

pub const HINT: &str = "text-muted-foreground text-sm";

/// A label above its control, with an optional hint below that the control should name in its
/// `aria-describedby` (`{input_id}-hint`).
#[component]
pub fn Field(
    #[prop(into)] label: String,
    #[prop(into)] input_id: String,
    #[prop(optional, into)] hint: Option<String>,
    children: Children,
) -> impl IntoView {
    let hint_id = format!("{input_id}-hint");
    view! {
        <div class="grid content-start gap-1.5">
            <label class=LABEL for=input_id>
                {label}
            </label>
            {children()}
            {hint.map(|hint| view! { <p id=hint_id class=HINT>{hint}</p> })}
        </div>
    }
}

/// A surface grouping related content, with an optional heading.
#[component]
pub fn Card(
    #[prop(optional, into)] title: Option<String>,
    #[prop(optional, into)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <section class=format!(
            "bg-card rounded-lg border p-4 shadow-[0_1px_2px_rgba(0,0,0,0.08)] md:p-6 {class}",
        )>
            {title.map(|title| view! { <h2 class="mb-4 text-base font-semibold">{title}</h2> })}
            {children()}
        </section>
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Neutral,
    Brand,
    Success,
    Warning,
    Danger,
}

impl Tone {
    fn badge(self) -> &'static str {
        match self {
            Tone::Neutral => "bg-muted text-foreground",
            Tone::Brand => "bg-brand-subtle text-brand",
            Tone::Success => "bg-success-subtle text-success",
            Tone::Warning => "bg-warning-subtle text-warning",
            Tone::Danger => "bg-danger-subtle text-danger",
        }
    }

    /// The small dot before a status, which repeats the tone of its text for sighted users.
    pub fn dot(self) -> &'static str {
        match self {
            Tone::Neutral => "bg-muted-foreground",
            Tone::Brand => "bg-brand",
            Tone::Success => "bg-success",
            Tone::Warning => "bg-warning",
            Tone::Danger => "bg-danger",
        }
    }
}

#[component]
pub fn Badge(#[prop(optional)] tone: Tone, children: Children) -> impl IntoView {
    view! {
        <span class=format!(
            "inline-flex w-fit items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-semibold whitespace-nowrap {}",
            tone.badge(),
        )>
            <span aria-hidden="true" class=format!("size-1.5 rounded-full {}", tone.dot())></span>
            {children()}
        </span>
    }
}

/// A message about the page or the last action. Errors are announced at once (`role=alert`),
/// the others politely (`role=status`).
#[component]
pub fn Alert(#[prop(optional)] tone: Tone, children: Children) -> impl IntoView {
    let (colors, role) = match tone {
        Tone::Danger => ("border-danger/40 bg-danger-subtle text-danger", "alert"),
        Tone::Warning => ("border-warning/40 bg-warning-subtle text-warning", "status"),
        Tone::Success => ("border-success/40 bg-success-subtle text-success", "status"),
        Tone::Brand | Tone::Neutral => ("bg-card text-foreground", "status"),
    };
    view! {
        <div role=role class=format!("rounded-lg border px-4 py-3 text-sm font-medium {colors}")>
            {children()}
        </div>
    }
}

/// What a list shows when it has nothing to list.
#[component]
pub fn Empty(#[prop(into)] title: String, #[prop(into)] body: String) -> impl IntoView {
    view! {
        <div class="bg-card rounded-lg border border-dashed p-8 text-center">
            <p class="font-semibold">{title}</p>
            <p class="text-muted-foreground mt-1 text-sm">{body}</p>
        </div>
    }
}
