//! `/administration/printers` and `/administration/scanners`: each device and the groups that
//! may use it, one row per device. The server answers an accepted form with
//! `<page>?done=<notice>` and a refused one by rendering the page again with its
//! [`DevicesProblem`]. A later change adds the management of the devices themselves to the
//! printers' row, in the same `<details>`.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{Alert, ButtonVariant, Empty, Tone, small_button_class},
    layout::Layout,
    t, tf,
};

/// Which devices a page is about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeviceKind {
    #[default]
    Printer,
    Scanner,
}

impl DeviceKind {
    /// The page of the kind, which its forms post below and return to.
    pub fn path(self) -> &'static str {
        match self {
            DeviceKind::Printer => "/administration/printers",
            DeviceKind::Scanner => "/administration/scanners",
        }
    }
}

/// Who may use a device, as the page shows and edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceAccess {
    Everyone,
    /// By group id. Possibly empty: administrators only.
    Groups(Vec<i64>),
}

/// A device as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    /// The name its service gives it, which the rule is stored under.
    pub name: String,
    /// What helps to tell it apart: a description, a model.
    pub detail: Option<String>,
    pub access: DeviceAccess,
    /// `false` for a rule whose device the service no longer lists.
    pub listed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupChoice {
    pub id: i64,
    pub name: String,
}

/// What the server hands the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DevicesPageData {
    pub kind: DeviceKind,
    /// `None` when the rules could not be loaded.
    pub devices: Option<Vec<DeviceRow>>,
    pub groups: Vec<GroupChoice>,
    /// CUPS or SANE did not answer: only the stored rules are listed.
    pub listing_failed: bool,
    pub refusal: Option<DevicesProblem>,
}

/// Why a form of the page was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicesProblem {
    /// No device name came with the form.
    NoDevice,
    /// The form did not say whether everyone or some groups may use the device.
    NoAccess,
    /// A chosen group is gone.
    UnknownGroup,
    /// The database failed.
    Unavailable,
}

impl DevicesProblem {
    fn message(self) -> &'static str {
        match self {
            DevicesProblem::NoDevice => t!("administration.devices.errors.no_device"),
            DevicesProblem::NoAccess => t!("administration.devices.errors.no_access"),
            DevicesProblem::UnknownGroup => t!("administration.devices.errors.unknown_group"),
            DevicesProblem::Unavailable => t!("administration.devices.errors.unavailable"),
        }
    }
}

/// What an accepted form did, named by the `done` query parameter: only a code travels in the
/// address, so a crafted link cannot put arbitrary text on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Access,
    Forgotten,
}

impl Notice {
    const ALL: [Notice; 2] = [Notice::Access, Notice::Forgotten];

    /// The value of the `done` query parameter.
    pub fn code(self) -> &'static str {
        match self {
            Notice::Access => "access",
            Notice::Forgotten => "forgotten",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notice| notice.code() == code)
    }

    fn message(self) -> &'static str {
        match self {
            Notice::Access => t!("administration.devices.done.access"),
            Notice::Forgotten => t!("administration.devices.done.forgotten"),
        }
    }
}

fn notice() -> Option<&'static str> {
    let query = use_query_map();
    let code = query.read_untracked().get("done")?;
    Notice::from_code(&code).map(Notice::message)
}

#[component]
pub fn PrintersAdministrationPage() -> impl IntoView {
    view! { <DevicesPage kind=DeviceKind::Printer /> }
}

#[component]
pub fn ScannersAdministrationPage() -> impl IntoView {
    view! { <DevicesPage kind=DeviceKind::Scanner /> }
}

#[component]
fn DevicesPage(kind: DeviceKind) -> impl IntoView {
    let Some((_, data)) = page_data::<DevicesPageData>() else {
        return view! { <NotFound /> }.into_any();
    };
    let (section, document_title, intro, empty, empty_body, listing_failed) = match kind {
        DeviceKind::Printer => (
            Section::Printers,
            t!("administration.printers.document_title"),
            t!("administration.printers.intro"),
            t!("administration.printers.empty"),
            t!("administration.printers.empty_body"),
            t!("administration.printers.listing_failed"),
        ),
        DeviceKind::Scanner => (
            Section::Scanners,
            t!("administration.scanners.document_title"),
            t!("administration.scanners.intro"),
            t!("administration.scanners.empty"),
            t!("administration.scanners.empty_body"),
            t!("administration.scanners.listing_failed"),
        ),
    };
    let alert = if let Some(problem) = data.refusal {
        Some(view! { <Alert tone=Tone::Danger>{problem.message()}</Alert> }.into_any())
    } else {
        notice().map(|message| view! { <Alert tone=Tone::Success>{message}</Alert> }.into_any())
    };
    let failed = data
        .listing_failed
        .then(|| view! { <Alert tone=Tone::Danger>{listing_failed}</Alert> });
    let groups = data.groups;
    let list = match data.devices {
        None => view! {
            <Alert tone=Tone::Danger>{t!("administration.devices.load_failed")}</Alert>
        }
        .into_any(),
        // Without the listing, no rule does not mean no device: the alert above says enough.
        Some(devices) if devices.is_empty() && data.listing_failed => ().into_any(),
        Some(devices) if devices.is_empty() => {
            view! { <Empty title=empty body=empty_body /> }.into_any()
        }
        Some(devices) => view! {
            <ul class="bg-card divide-y rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                {devices
                    .into_iter()
                    .map(|device| view! { <Device kind device groups=groups.clone() /> })
                    .collect_view()}
            </ul>
        }
        .into_any(),
    };
    view! {
        <Layout title=t!("administration.title") document_title=document_title>
            <Sections current=section />
            <p class="text-muted-foreground text-sm">{intro}</p>
            {alert}
            {failed}
            {list}
        </Layout>
    }
    .into_any()
}

/// Who may use the device, in words.
fn access_text(access: &DeviceAccess, groups: &[GroupChoice]) -> String {
    match access {
        DeviceAccess::Everyone => t!("administration.devices.everyone").to_owned(),
        DeviceAccess::Groups(ids) => {
            let names: Vec<&str> = groups
                .iter()
                .filter(|group| ids.contains(&group.id))
                .map(|group| group.name.as_str())
                .collect();
            if names.is_empty() {
                t!("administration.devices.administrators_only").to_owned()
            } else {
                tf!(
                    "administration.devices.groups",
                    groups = names.join(t!("administration.devices.separator"))
                )
            }
        }
    }
}

#[component]
fn Device(kind: DeviceKind, device: DeviceRow, groups: Vec<GroupChoice>) -> impl IntoView {
    let access = access_text(&device.access, &groups);
    let everyone = device.access == DeviceAccess::Everyone;
    let selected = match &device.access {
        DeviceAccess::Everyone => vec![],
        DeviceAccess::Groups(ids) => ids.clone(),
    };
    let missing = (!device.listed).then(|| {
        view! {
            <span class="text-danger text-xs font-semibold">
                {t!("administration.devices.not_found")}
            </span>
        }
    });
    let detail = device
        .detail
        .map(|detail| view! { <p class="text-muted-foreground text-sm">{detail}</p> });
    let choices = if groups.is_empty() {
        view! {
            <p class="text-muted-foreground text-sm">{t!("administration.devices.no_groups")}</p>
        }
        .into_any()
    } else {
        view! {
            <ul class="grid gap-1">
                {groups
                    .into_iter()
                    .map(|group| {
                        view! {
                            <li>
                                <label class="flex min-h-8 items-center gap-2 text-sm">
                                    <input
                                        type="checkbox"
                                        name="groups"
                                        value=group.id.to_string()
                                        checked=selected.contains(&group.id)
                                        class="accent-brand size-4"
                                    />
                                    {group.name}
                                </label>
                            </li>
                        }
                    })
                    .collect_view()}
            </ul>
        }
        .into_any()
    };
    let forget = (!device.listed).then(|| {
        view! {
            <form method="post" action=format!("{}/forget", kind.path())>
                <input type="hidden" name="device" value=device.name.clone() />
                <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                    {t!("administration.devices.forget")}
                </button>
                <span class="text-muted-foreground ml-2 text-xs">
                    {t!("administration.devices.forget_hint")}
                </span>
            </form>
        }
    });
    view! {
        <li class="grid gap-2 px-4 py-3 md:px-6">
            <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <h2 class="font-semibold break-all">{device.name.clone()}</h2>
                {missing}
            </div>
            {detail}
            <p class="text-sm">{access}</p>
            <details class="text-sm">
                <summary class="text-brand min-h-8 cursor-pointer font-semibold">
                    {t!("administration.devices.change")}
                </summary>
                <div class="grid gap-4 pt-2">
                    <form method="post" action=format!("{}/access", kind.path())>
                        <input type="hidden" name="device" value=device.name.clone() />
                        // Every row has the same controls: the legend says which device these are.
                        <fieldset class="grid gap-3">
                            <legend class="sr-only">
                                {tf!("administration.devices.access_of", device = device.name)}
                            </legend>
                            <label class="flex min-h-8 items-center gap-2">
                                <input
                                    type="radio"
                                    name="access"
                                    value="everyone"
                                    checked=everyone
                                    class="accent-brand size-4"
                                />
                                {t!("administration.devices.choose_everyone")}
                            </label>
                            <label class="flex min-h-8 items-center gap-2">
                                <input
                                    type="radio"
                                    name="access"
                                    value="groups"
                                    checked=!everyone
                                    class="accent-brand size-4"
                                />
                                {t!("administration.devices.choose_groups")}
                            </label>
                            <div class="pl-6">{choices}</div>
                            <p class="text-muted-foreground text-xs">
                                {t!("administration.devices.administrators_note")}
                            </p>
                            <div>
                                <button
                                    type="submit"
                                    class=small_button_class(ButtonVariant::Primary)
                                >
                                    {t!("administration.devices.save")}
                                </button>
                            </div>
                        </fieldset>
                    </form>
                    {forget}
                </div>
            </details>
        </li>
    }
}
