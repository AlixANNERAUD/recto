//! `/administration/groups`: each group and its members. Groups come from the identity
//! provider's groups claim at each sign-in, so the page only reads them.

use leptos::prelude::*;

use super::{Section, Sections, page_data};
use crate::{
    NotFound,
    components::ui::{Alert, Empty, Tone},
    layout::Layout,
    t, tp,
};

/// A group as the page lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub name: String,
    /// By username.
    pub members: Vec<GroupMember>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    pub username: String,
    pub name: String,
    pub is_disabled: bool,
}

/// What the server hands the page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupsPageData {
    /// `None` when they could not be loaded.
    pub groups: Option<Vec<GroupRow>>,
}

#[component]
pub fn GroupsPage() -> impl IntoView {
    let Some((_, data)) = page_data::<GroupsPageData>() else {
        return view! { <NotFound /> }.into_any();
    };
    let content = match data.groups {
        None => view! {
            <Alert tone=Tone::Danger>{t!("administration.groups.load_failed")}</Alert>
        }
        .into_any(),
        Some(groups) if groups.is_empty() => view! {
            <Empty title=t!("administration.groups.empty") body=t!("administration.groups.empty_body") />
        }
        .into_any(),
        Some(groups) => view! {
            <ul class="bg-card divide-y rounded-lg border shadow-[0_1px_2px_rgba(0,0,0,0.08)]">
                {groups.into_iter().map(|group| view! { <Group group /> }).collect_view()}
            </ul>
        }
        .into_any(),
    };
    view! {
        <Layout title=t!("administration.title") document_title=t!("administration.groups.document_title")>
            <Sections current=Section::Groups />
            <p class="text-muted-foreground text-sm">{t!("administration.groups.intro")}</p>
            {content}
        </Layout>
    }
    .into_any()
}

#[component]
fn Group(group: GroupRow) -> impl IntoView {
    let count = tp!("administration.groups.members", group.members.len());
    let members = if group.members.is_empty() {
        view! { <p class="text-muted-foreground text-sm">{t!("administration.groups.no_members")}</p> }
            .into_any()
    } else {
        view! {
            <ul class="flex flex-wrap gap-2">
                {group.members.into_iter().map(|member| view! { <Member member /> }).collect_view()}
            </ul>
        }
        .into_any()
    };
    view! {
        <li class="grid gap-2 px-4 py-3 md:px-6">
            <div class="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                <h2 class="font-semibold break-all">{group.name}</h2>
                <span class="text-muted-foreground text-sm">{count}</span>
            </div>
            {members}
        </li>
    }
}

#[component]
fn Member(member: GroupMember) -> impl IntoView {
    let disabled = member.is_disabled.then(|| {
        view! { <span class="text-danger text-xs font-semibold">{t!("administration.groups.disabled")}</span> }
    });
    view! {
        <li class="bg-muted inline-flex items-baseline gap-1.5 rounded-full px-3 py-1 text-sm">
            <span class="font-medium">{member.name}</span>
            <span class="text-muted-foreground">{member.username}</span>
            {disabled}
        </li>
    }
}
