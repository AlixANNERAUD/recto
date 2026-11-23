//! The frame around every signed-in page: a header with the app's logo and name, the user (a link
//! to the Account page for users with a password) and Sign out, and the navigation. On phones the
//! navigation is a row of icons under the header; from the `md` breakpoint it becomes a rail on
//! the side, as in Fluent's navigation drawer.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_location;

use crate::{
    branding::{app_name, has_logo},
    components::icons::{Icon, IconKind},
    language::LanguageSwitch,
    t, tf,
    theme::ThemeSwitch,
};

/// Where the Administration item leads; the Users and Groups pages link to each other.
const ADMINISTRATION: &str = "/administration/users";

/// A page of the navigation: its address, its label, a shorter label for the phone row where
/// five items share 390 pixels, and its icon.
struct Destination {
    href: &'static str,
    label: &'static str,
    short_label: &'static str,
    icon: IconKind,
}

/// The pages of the navigation, in order. Administrators also get Administration, whose pages
/// all count as that item.
fn destinations(administrator: bool) -> Vec<Destination> {
    let page = |href, label, icon| Destination {
        href,
        label,
        short_label: label,
        icon,
    };
    let mut destinations = vec![
        page("/", t!("nav.print"), IconKind::Print),
        page("/printers", t!("nav.printers"), IconKind::Printer),
        page("/jobs", t!("nav.jobs"), IconKind::Jobs),
        page("/scan", t!("nav.scan"), IconKind::Scan),
    ];
    if administrator {
        destinations.push(Destination {
            href: ADMINISTRATION,
            label: t!("nav.administration"),
            short_label: t!("nav.administration_short"),
            icon: IconKind::Administration,
        });
    }
    destinations
}

/// Whether `path` belongs to the navigation item of `href`.
fn is_current(path: &str, href: &str) -> bool {
    if href == ADMINISTRATION {
        path.starts_with("/administration/")
    } else {
        path == href
    }
}

/// Whether the signed-in user administers recto.
#[cfg(feature = "ssr")]
fn is_administrator() -> bool {
    crate::state::current_user().is_some_and(|user| user.is_administrator)
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn is_administrator() -> bool {
    false
}

/// The name of the signed-in user, for the header, and whether they have an Account page: it
/// changes a password, for users with one while local accounts are on, and holds device
/// passwords, for everyone while OS integration is on.
#[cfg(feature = "ssr")]
fn user_name() -> Option<(String, bool)> {
    let user = crate::state::current_user()?;
    let has_account = crate::pages::account::has_account_page(&user);
    Some((user.name, has_account))
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn user_name() -> Option<(String, bool)> {
    None
}

const NAV_ITEM: &str = "relative flex flex-col items-center gap-0.5 rounded-md px-0.5 py-1.5 text-[11px] font-medium transition-colors hover:bg-muted md:flex-row md:gap-3 md:px-3 md:py-2 md:text-sm";

/// The current page's item: the accent color and a bar, under the item on phones and before it
/// on the side rail.
const NAV_CURRENT: &str = "text-brand bg-brand-subtle hover:bg-brand-subtle";
const NAV_INDICATOR: &str = "absolute bg-brand rounded-full bottom-0 left-1/4 right-1/4 h-[3px] md:left-0 md:right-auto md:top-1/4 md:bottom-1/4 md:h-auto md:w-[3px]";

/// The operator's logo, else the printer glyph on the accent. Decoration: the name follows it.
#[component]
fn Mark() -> impl IntoView {
    if has_logo() {
        view! { <img src="/brand/logo" alt="" class="h-7 w-auto max-w-32 shrink-0" /> }.into_any()
    } else {
        view! {
            <span aria-hidden="true" class="bg-primary text-primary-foreground grid size-7 shrink-0 place-items-center rounded-md">
                <Icon kind=IconKind::Printer class="size-4" />
            </span>
        }
        .into_any()
    }
}

/// `title` is the page's heading, and its document title unless `document_title` is given, for
/// pages that share a heading.
#[component]
pub fn Layout(
    #[prop(into)] title: String,
    #[prop(optional, into)] document_title: Option<String>,
    children: Children,
) -> impl IntoView {
    let path = use_location().pathname.get_untracked();
    let administrator = is_administrator();
    let switches = use_context::<ShownOnce>().is_none();
    // Rendered twice, for the phone row and the side rail; CSS shows one of them.
    let items = |phone: bool| {
        destinations(administrator)
        .into_iter()
        .map(|Destination { href, label, short_label, icon }| {
            let current = is_current(&path, href);
            let label = if phone { short_label } else { label };
            view! {
                <li>
                    <a
                        href=href
                        class=format!(
                            "{NAV_ITEM} {}",
                            if current { NAV_CURRENT } else { "text-muted-foreground hover:text-foreground" },
                        )
                        aria-current=current.then_some("page")
                    >
                        {current.then(|| view! { <span aria-hidden="true" class=NAV_INDICATOR></span> })}
                        <Icon kind=icon />
                        // A label too long for its cell (French, a narrow phone) is cut short
                        // rather than running into the next one.
                        <span class="max-w-full truncate">{label}</span>
                    </a>
                </li>
            }
        })
        .collect_view()
    };
    let user = user_name().map(|(name, has_account)| {
        let label = if has_account {
            // On phones the name is hidden and the icon alone leads to the account.
            let current = path == "/account";
            view! {
                <a
                    href="/account"
                    class=format!(
                        "hover:bg-muted inline-flex min-h-10 max-w-56 items-center gap-2 rounded-md px-2.5 text-sm font-medium {}",
                        if current { "text-brand" } else { "text-muted-foreground hover:text-foreground" },
                    )
                    aria-label=tf!("nav.account_of", name = name.clone())
                    aria-current=current.then_some("page")
                >
                    <Icon kind=IconKind::Account />
                    <span class="hidden truncate sm:inline">{name.clone()}</span>
                </a>
            }
            .into_any()
        } else {
            view! {
                <span class="text-muted-foreground hidden max-w-48 truncate text-sm sm:inline" title=tf!("nav.signed_in_as", name = name.clone())>
                    {name.clone()}
                </span>
            }
            .into_any()
        };
        view! {
            {label}
            <form method="post" action="/auth/logout">
                <button
                    type="submit"
                    class="hover:bg-muted inline-flex min-h-10 items-center gap-2 rounded-md px-2.5 text-sm font-medium hover:cursor-pointer"
                    aria-label=tf!("nav.sign_out_as", name = name)
                >
                    <Icon kind=IconKind::SignOut />
                    <span class="hidden sm:inline">{t!("auth.sign_out")}</span>
                </button>
            </form>
        }
    });
    let document_title = tf!(
        "app.page_title",
        page = document_title.unwrap_or_else(|| title.clone()),
        name = app_name()
    );
    view! {
        <Title text=document_title />
        <a
            href="#main"
            class="bg-card sr-only z-30 rounded-md px-3 py-2 focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
        >
            {t!("nav.skip")}
        </a>
        <header class="bg-card sticky top-0 z-20 border-b">
            <div class="flex h-12 items-center gap-3 px-4">
                // A long brand name gives way to the user's controls.
                <a href="/" class="flex min-w-0 items-center gap-2 font-semibold">
                    <Mark />
                    <span class="truncate">{app_name()}</span>
                </a>
                <div class="ml-auto flex items-center gap-2">
                    {switches.then(|| view! { <Switches /> })}
                    {user}
                </div>
            </div>
            // On phones the navigation stays in the sticky header, one tap away on every page.
            <nav aria-label=t!("nav.label") class="border-t px-2 py-1 md:hidden">
                <ul class=format!(
                    "grid gap-1 {}",
                    if administrator { "grid-cols-5" } else { "grid-cols-4" },
                )>{items(true)}</ul>
            </nav>
        </header>
        <div class="md:flex">
            <nav
                aria-label=t!("nav.label")
                class="bg-card hidden shrink-0 border-r p-2 md:sticky md:top-12 md:block md:h-[calc(100vh-3rem)] md:w-52"
            >
                <ul class="grid gap-1">{items(false)}</ul>
            </nav>
            <main id="main" class="mx-auto w-full max-w-5xl min-w-0 px-4 py-5 md:px-8 md:py-8">
                <h1 class="mb-5 text-2xl font-semibold">{title}</h1>
                <div class="grid gap-5">{children()}</div>
            </main>
        </div>
    }
    .into_any()
}

/// Marks a page that cannot be shown again, such as one holding a secret shown once: its header
/// leaves out the switches, which reload the page with a GET and would lose what it shows.
#[derive(Clone, Copy, Debug)]
pub struct ShownOnce;

/// The language and theme switches.
#[component]
fn Switches() -> impl IntoView {
    view! {
        <div class="flex items-center gap-2">
            <LanguageSwitch />
            <ThemeSwitch />
        </div>
    }
}

/// The frame of the pages shown without a signed-in user (sign-in errors, no access): the app's
/// logo and name and one centered card.
#[component]
pub fn Bare(
    #[prop(into)] title: String,
    /// Whether to show the language and theme switches; the offline page has none, since they
    /// cannot work there.
    #[prop(default = true)]
    switch: bool,
    children: Children,
) -> impl IntoView {
    let document_title = tf!("app.page_title", page = title.clone(), name = app_name());
    view! {
        <Title text=document_title />
        <main class="mx-auto grid min-h-screen max-w-md content-center gap-4 p-4">
            <div class="flex items-center justify-between gap-2">
                <p class="flex min-w-0 items-center gap-2 font-semibold">
                    <Mark />
                    <span class="truncate">{app_name()}</span>
                </p>
                {switch.then(|| view! { <Switches /> })}
            </div>
            <section class="bg-card grid gap-4 rounded-lg border p-6 shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                <h1 class="text-xl font-semibold">{title}</h1>
                {children()}
            </section>
        </main>
    }
    .into_any()
}
