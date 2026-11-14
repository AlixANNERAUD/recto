//! The frame around every signed-in page: a header with the app name, the user and Sign out,
//! and the navigation. On phones the navigation is a row of icons under the header; from the
//! `md` breakpoint it becomes a rail on the side, as in Fluent's navigation drawer.

use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_location;

use crate::{
    components::icons::{Icon, IconKind},
    t, tf,
};

/// The pages of the navigation, in order.
fn destinations() -> [(&'static str, String, IconKind); 4] {
    [
        ("/", t!("nav.print").to_string(), IconKind::Print),
        (
            "/printers",
            t!("nav.printers").to_string(),
            IconKind::Printer,
        ),
        ("/jobs", t!("nav.jobs").to_string(), IconKind::Jobs),
        ("/scan", t!("nav.scan").to_string(), IconKind::Scan),
    ]
}

/// The name of the signed-in user, for the header.
#[cfg(feature = "ssr")]
fn user_name() -> Option<String> {
    crate::state::current_user().map(|user| user.name.unwrap_or(user.username))
}

/// Pages render on the server only.
#[cfg(not(feature = "ssr"))]
fn user_name() -> Option<String> {
    None
}

const NAV_ITEM: &str = "relative flex flex-col items-center gap-0.5 rounded-md px-2 py-1.5 text-xs font-medium transition-colors hover:bg-muted md:flex-row md:gap-3 md:px-3 md:py-2 md:text-sm";

/// The current page's item: the accent color and a bar, under the item on phones and before it
/// on the side rail.
const NAV_CURRENT: &str = "text-brand bg-brand-subtle hover:bg-brand-subtle";
const NAV_INDICATOR: &str = "absolute bg-brand rounded-full bottom-0 left-1/4 right-1/4 h-[3px] md:left-0 md:right-auto md:top-1/4 md:bottom-1/4 md:h-auto md:w-[3px]";

#[component]
pub fn Layout(#[prop(into)] title: String, children: Children) -> impl IntoView {
    let path = use_location().pathname.get_untracked();
    // Rendered twice, for the phone row and the side rail; CSS shows one of them.
    let items = || {
        destinations()
        .into_iter()
        .map(|(href, label, icon)| {
            let current = path == href;
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
                        <span>{label}</span>
                    </a>
                </li>
            }
        })
        .collect_view()
    };
    let user = user_name().map(|name| {
        view! {
            <span class="text-muted-foreground hidden max-w-48 truncate text-sm sm:inline" title=tf!("nav.signed_in_as", name = name.clone())>
                {name.clone()}
            </span>
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
    let document_title = tf!("app.page_title", page = title.clone());
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
                <a href="/" class="flex items-center gap-2 font-semibold">
                    <span aria-hidden="true" class="bg-primary text-primary-foreground grid size-7 place-items-center rounded-md">
                        <Icon kind=IconKind::Printer class="size-4" />
                    </span>
                    {t!("app.name")}
                </a>
                <div class="ml-auto flex items-center gap-2">{user}</div>
            </div>
            // On phones the navigation stays in the sticky header, one tap away on every page.
            <nav aria-label=t!("nav.label") class="border-t px-2 py-1 md:hidden">
                <ul class="grid grid-cols-4 gap-1">{items()}</ul>
            </nav>
        </header>
        <div class="md:flex">
            <nav
                aria-label=t!("nav.label")
                class="bg-card hidden shrink-0 border-r p-2 md:sticky md:top-12 md:block md:h-[calc(100vh-3rem)] md:w-52"
            >
                <ul class="grid gap-1">{items()}</ul>
            </nav>
            <main id="main" class="mx-auto w-full max-w-5xl min-w-0 px-4 py-5 md:px-8 md:py-8">
                <h1 class="mb-5 text-2xl font-semibold">{title}</h1>
                <div class="grid gap-5">{children()}</div>
            </main>
        </div>
    }
    .into_any()
}

/// The frame of the pages shown without a signed-in user (sign-in errors, no access): the app
/// name and one centered card.
#[component]
pub fn Bare(#[prop(into)] title: String, children: Children) -> impl IntoView {
    let document_title = tf!("app.page_title", page = title.clone());
    view! {
        <Title text=document_title />
        <main class="mx-auto grid min-h-screen max-w-md content-center gap-4 p-4">
            <p class="flex items-center gap-2 font-semibold">
                <span aria-hidden="true" class="bg-primary text-primary-foreground grid size-7 place-items-center rounded-md">
                    <Icon kind=IconKind::Printer class="size-4" />
                </span>
                {t!("app.name")}
            </p>
            <section class="bg-card grid gap-4 rounded-lg border p-6 shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                <h1 class="text-xl font-semibold">{title}</h1>
                {children()}
            </section>
        </main>
    }
    .into_any()
}
