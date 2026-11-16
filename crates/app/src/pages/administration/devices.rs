//! `/administration/printers` and `/administration/scanners`: each device and the groups that
//! may use it, one row per device. The server answers an accepted form with
//! `<page>?done=<notice>` and a refused one by rendering the page again with its
//! [`DevicesProblem`]. When recto can administer CUPS, the printers' page also adds printers and
//! each row manages its printer, in the same `<details>`.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{
        Alert, ButtonVariant, Card, Empty, Field, INPUT, Tone, button_class, small_button_class,
    },
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
    /// What the printer's actions start from; `None` when recto cannot administer CUPS or the
    /// printer is not listed.
    pub controls: Option<PrinterControls>,
    /// `false` for a rule whose device the service no longer lists.
    pub listed: bool,
}

/// The state of a printer, which decides the actions its row offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrinterControls {
    pub description: String,
    pub location: String,
    pub is_default: bool,
    /// Stopped: the queue holds its jobs.
    pub paused: bool,
    pub accepting_jobs: bool,
}

/// A driver of the Add printer list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverChoice {
    /// The `ppd-name` the form posts.
    pub name: String,
    pub make_and_model: String,
}

/// What the Add printer form was filled in with, kept when CUPS or the checks refuse it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPrinterDraft {
    pub name: String,
    pub address: String,
    /// `everywhere` or a `ppd-name`.
    pub driver: String,
    pub description: String,
    pub location: String,
}

impl Default for NewPrinterDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            driver: "everywhere".into(),
            description: String::new(),
            location: String::new(),
        }
    }
}

/// The Add printer section, present when recto can administer CUPS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrinterTools {
    /// What the driver search asked for.
    pub search: String,
    pub drivers: Vec<DriverChoice>,
    /// The search could not be answered.
    pub search_failed: bool,
    pub draft: NewPrinterDraft,
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
    pub tools: Option<PrinterTools>,
    pub refusal: Option<DevicesProblem>,
}

/// Why a form of the page was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicesProblem {
    /// No device name came with the form.
    NoDevice,
    /// The form did not say whether everyone or some groups may use the device.
    NoAccess,
    /// A chosen group is gone.
    UnknownGroup,
    /// The database failed.
    Unavailable,
    /// The name of a new printer is not a plain one.
    InvalidName,
    /// The name of an existing printer is not one CUPS accepts.
    InvalidQueueName,
    /// A description or location is too long or has control characters.
    InvalidText,
    /// The printer address is not one recto sends to CUPS.
    InvalidAddress,
    /// A printer of that name, whatever its case, is already in CUPS.
    NameTaken,
    /// The removal was not confirmed.
    NeedsConfirmation,
    /// CUPS does not let recto administer it.
    CupsRefused,
    /// CUPS could not reach the printer, in its own words.
    Unreachable(String),
    /// CUPS declined the change, in its own words.
    Rejected(String),
    /// CUPS could not be asked.
    CupsUnavailable,
}

impl DevicesProblem {
    fn message(self) -> AnyView {
        let text = match self {
            DevicesProblem::NoDevice => t!("administration.devices.errors.no_device"),
            DevicesProblem::NoAccess => t!("administration.devices.errors.no_access"),
            DevicesProblem::UnknownGroup => t!("administration.devices.errors.unknown_group"),
            DevicesProblem::Unavailable => t!("administration.devices.errors.unavailable"),
            DevicesProblem::InvalidName => t!("administration.printers.errors.invalid_name"),
            DevicesProblem::InvalidQueueName => {
                t!("administration.printers.errors.invalid_queue_name")
            }
            DevicesProblem::InvalidText => t!("administration.printers.errors.invalid_text"),
            DevicesProblem::InvalidAddress => t!("administration.printers.errors.invalid_address"),
            DevicesProblem::NameTaken => t!("administration.printers.errors.name_taken"),
            DevicesProblem::NeedsConfirmation => {
                t!("administration.printers.errors.needs_confirmation")
            }
            DevicesProblem::CupsUnavailable => {
                t!("administration.printers.errors.cups_unavailable")
            }
            DevicesProblem::CupsRefused => {
                // The setup is the fix, so the message points to the guide.
                return view! {
                    {t!("administration.printers.errors.refused")}
                    " "
                    <a href=SETUP_GUIDE class="underline">
                        {t!("administration.printers.errors.refused_guide")}
                    </a>
                }
                .into_any();
            }
            DevicesProblem::Unreachable(message) => {
                return tf!(
                    "administration.printers.errors.unreachable",
                    message = message
                )
                .into_any();
            }
            DevicesProblem::Rejected(message) => {
                return tf!("administration.printers.errors.rejected", message = message)
                    .into_any();
            }
        };
        text.into_any()
    }
}

/// The longest description or location of a printer the server accepts, in characters. The
/// browser counts `maxlength` in UTF-16 units, never fewer, so it lets through nothing longer.
pub const PRINTER_TEXT_MAX: usize = 1023;

/// The guide to the CUPS setup that lets recto administer printers.
const SETUP_GUIDE: &str = "https://alixanneraud.github.io/recto/how-to/add-printer.html";

/// What an accepted form did, named by the `done` query parameter: only a code travels in the
/// address, so a crafted link cannot put arbitrary text on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Access,
    Forgotten,
    Added,
    Described,
    Default,
    Paused,
    Resumed,
    Accepting,
    Rejecting,
    Removed,
}

impl Notice {
    const ALL: [Notice; 10] = [
        Notice::Access,
        Notice::Forgotten,
        Notice::Added,
        Notice::Described,
        Notice::Default,
        Notice::Paused,
        Notice::Resumed,
        Notice::Accepting,
        Notice::Rejecting,
        Notice::Removed,
    ];

    /// The value of the `done` query parameter.
    pub fn code(self) -> &'static str {
        match self {
            Notice::Access => "access",
            Notice::Forgotten => "forgotten",
            Notice::Added => "added",
            Notice::Described => "described",
            Notice::Default => "default",
            Notice::Paused => "paused",
            Notice::Resumed => "resumed",
            Notice::Accepting => "accepting",
            Notice::Rejecting => "rejecting",
            Notice::Removed => "removed",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|notice| notice.code() == code)
    }

    fn message(self) -> &'static str {
        match self {
            Notice::Access => t!("administration.devices.done.access"),
            Notice::Forgotten => t!("administration.devices.done.forgotten"),
            Notice::Added => t!("administration.printers.done.added"),
            Notice::Described => t!("administration.printers.done.described"),
            Notice::Default => t!("administration.printers.done.default"),
            Notice::Paused => t!("administration.printers.done.paused"),
            Notice::Resumed => t!("administration.printers.done.resumed"),
            Notice::Accepting => t!("administration.printers.done.accepting"),
            Notice::Rejecting => t!("administration.printers.done.rejecting"),
            Notice::Removed => t!("administration.printers.done.removed"),
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
    let add = data.tools.map(|tools| view! { <AddPrinter tools /> });
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
            {add}
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
    // With the printer actions, the section is about more than access.
    let summary = if device.controls.is_some() {
        t!("administration.printers.manage.summary")
    } else {
        t!("administration.devices.change")
    };
    let manage = device
        .controls
        .clone()
        .map(|controls| view! { <ManagePrinter name=device.name.clone() controls /> });
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
                    {summary}
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
                    {manage}
                    {forget}
                </div>
            </details>
        </li>
    }
}

/// The Add printer section. Searching the drivers is a GET form of its own, so it works without
/// JavaScript; it reloads the page with the matches in the driver list.
#[component]
fn AddPrinter(tools: PrinterTools) -> impl IntoView {
    let draft = tools.draft;
    let listed = tools
        .drivers
        .iter()
        .any(|driver| driver.name == draft.driver);
    // A driver chosen before a refusal stays selectable although the search is gone.
    let kept = (draft.driver != "everywhere" && !listed).then(|| {
        let name = draft.driver.clone();
        view! { <option value=name.clone() selected>{name.clone()}</option> }
    });
    let options = tools
        .drivers
        .into_iter()
        .map(|driver| {
            let selected = driver.name == draft.driver;
            view! {
                <option value=driver.name selected=selected>
                    {driver.make_and_model}
                </option>
            }
        })
        .collect_view();
    let failed = tools
        .search_failed
        .then(|| view! { <Alert tone=Tone::Danger>{t!("administration.printers.add.search_failed")}</Alert> });
    view! {
        <Card title=t!("administration.printers.add.title")>
            <div class="grid gap-4">
                <p class="text-muted-foreground text-sm">{t!("administration.printers.add.intro")}</p>
                <form method="get" action=DeviceKind::Printer.path() class="grid gap-2 sm:grid-cols-[1fr_auto] sm:items-end">
                    <Field
                        label=t!("administration.printers.add.search")
                        input_id="driver-search"
                        hint=t!("administration.printers.add.search_hint")
                    >
                        <input
                            id="driver-search"
                            name="drivers"
                            type="search"
                            value=tools.search
                            autocomplete="off"
                            aria-describedby="driver-search-hint"
                            class=INPUT
                        />
                    </Field>
                    <div>
                        <button type="submit" class=button_class(ButtonVariant::Secondary)>
                            {t!("administration.printers.add.search_submit")}
                        </button>
                    </div>
                </form>
                {failed}
                <form
                    method="post"
                    action=format!("{}/add", DeviceKind::Printer.path())
                    class="grid gap-4 sm:grid-cols-2"
                >
                    <Field
                        label=t!("administration.printers.add.name")
                        input_id="new-printer-name"
                        hint=t!("administration.printers.add.name_hint")
                    >
                        <input
                            id="new-printer-name"
                            name="name"
                            type="text"
                            value=draft.name
                            required
                            maxlength="127"
                            pattern="[A-Za-z0-9_.\\-]+"
                            autocomplete="off"
                            autocapitalize="none"
                            spellcheck="false"
                            aria-describedby="new-printer-name-hint"
                            class=INPUT
                        />
                    </Field>
                    <Field
                        label=t!("administration.printers.add.address")
                        input_id="new-printer-address"
                        hint=t!("administration.printers.add.address_hint")
                    >
                        <input
                            id="new-printer-address"
                            name="address"
                            type="text"
                            value=draft.address
                            required
                            autocomplete="off"
                            autocapitalize="none"
                            spellcheck="false"
                            placeholder="ipp://192.168.1.20/ipp/print"
                            aria-describedby="new-printer-address-hint"
                            class=INPUT
                        />
                    </Field>
                    <Field
                        label=t!("administration.printers.add.driver")
                        input_id="new-printer-driver"
                        hint=t!("administration.printers.add.driver_hint")
                    >
                        <select
                            id="new-printer-driver"
                            name="driver"
                            aria-describedby="new-printer-driver-hint"
                            class=INPUT
                        >
                            <option value="everywhere" selected=draft.driver == "everywhere">
                                {t!("administration.printers.add.everywhere")}
                            </option>
                            {kept}
                            {options}
                        </select>
                    </Field>
                    <Field
                        label=t!("administration.printers.add.description")
                        input_id="new-printer-description"
                    >
                        <input
                            id="new-printer-description"
                            name="description"
                            type="text"
                            value=draft.description
                            maxlength=PRINTER_TEXT_MAX.to_string()
                            autocomplete="off"
                            class=INPUT
                        />
                    </Field>
                    <Field
                        label=t!("administration.printers.add.location")
                        input_id="new-printer-location"
                    >
                        <input
                            id="new-printer-location"
                            name="location"
                            type="text"
                            value=draft.location
                            maxlength=PRINTER_TEXT_MAX.to_string()
                            autocomplete="off"
                            class=INPUT
                        />
                    </Field>
                    <div class="sm:col-span-2">
                        <button type="submit" class=button_class(ButtonVariant::Primary)>
                            {t!("administration.printers.add.submit")}
                        </button>
                    </div>
                </form>
            </div>
        </Card>
    }
}

/// A button alone in a form that posts the printer's name to one of the printer actions.
fn action_form(action: &'static str, printer: &str, label: &'static str) -> impl IntoView {
    view! {
        <form method="post" action=format!("{}/{action}", DeviceKind::Printer.path())>
            <input type="hidden" name="printer" value=printer.to_owned() />
            <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                {label}
            </button>
        </form>
    }
}

/// The actions on one printer: its texts, the default, pausing, accepting and removing.
#[component]
fn ManagePrinter(name: String, controls: PrinterControls) -> impl IntoView {
    // Always there, so every row has the same controls; the default printer's is spent.
    let default = view! {
        <form method="post" action=format!("{}/default", DeviceKind::Printer.path())>
            <input type="hidden" name="printer" value=name.clone() />
            <button
                type="submit"
                disabled=controls.is_default
                class=small_button_class(ButtonVariant::Secondary)
            >
                {if controls.is_default {
                    t!("administration.printers.manage.is_default")
                } else {
                    t!("administration.printers.manage.make_default")
                }}
            </button>
        </form>
    };
    let pause = if controls.paused {
        action_form("resume", &name, t!("administration.printers.manage.resume")).into_any()
    } else {
        action_form("pause", &name, t!("administration.printers.manage.pause")).into_any()
    };
    let accept = if controls.accepting_jobs {
        action_form("reject", &name, t!("administration.printers.manage.reject")).into_any()
    } else {
        action_form("accept", &name, t!("administration.printers.manage.accept")).into_any()
    };
    view! {
        <div class="grid gap-4 border-t pt-4">
            // Every row has the same buttons: the heading says which printer they act on.
            <h3 class="font-semibold">
                {tf!("administration.printers.manage.title", printer = name.clone())}
            </h3>
            <form
                method="post"
                action=format!("{}/describe", DeviceKind::Printer.path())
                class="grid gap-3 sm:grid-cols-2"
            >
                <input type="hidden" name="printer" value=name.clone() />
                <label class="grid gap-1.5 text-sm font-semibold">
                    {t!("administration.printers.manage.description")}
                    <input
                        name="description"
                        type="text"
                        value=controls.description
                        maxlength=PRINTER_TEXT_MAX.to_string()
                        autocomplete="off"
                        class=format!("{INPUT} font-normal")
                    />
                </label>
                <label class="grid gap-1.5 text-sm font-semibold">
                    {t!("administration.printers.manage.location")}
                    <input
                        name="location"
                        type="text"
                        value=controls.location
                        maxlength=PRINTER_TEXT_MAX.to_string()
                        autocomplete="off"
                        class=format!("{INPUT} font-normal")
                    />
                </label>
                <div class="sm:col-span-2">
                    <button type="submit" class=small_button_class(ButtonVariant::Primary)>
                        {t!("administration.printers.manage.save_texts")}
                    </button>
                </div>
            </form>
            <div class="flex flex-wrap items-center gap-2">{default} {pause} {accept}</div>
            <form
                method="post"
                action=format!("{}/remove", DeviceKind::Printer.path())
                class="grid gap-2"
            >
                <input type="hidden" name="printer" value=name.clone() />
                <label class="flex min-h-8 items-center gap-2">
                    <input
                        type="checkbox"
                        name="confirm"
                        value="yes"
                        required
                        class="accent-brand size-4"
                    />
                    {tf!("administration.printers.manage.confirm", printer = name)}
                </label>
                <div>
                    <button type="submit" class=small_button_class(ButtonVariant::Secondary)>
                        {t!("administration.printers.manage.remove")}
                    </button>
                </div>
            </form>
        </div>
    }
}
