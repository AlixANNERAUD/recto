//! Groups of users. They come from the identity provider's groups claim, applied at each
//! sign-in (see [`crate::users::sign_in_with_identity`]); administrators cannot edit them.

use std::collections::HashMap;

use recto_entity::{group_memberships, groups, users};
use sea_orm::{ConnectionTrait, EntityTrait, QueryOrder};

use crate::error::Result;

/// Every group by name, with its members by username. Groups without members are listed too:
/// they are never deleted automatically.
pub async fn list(
    database: &impl ConnectionTrait,
) -> Result<Vec<(groups::Model, Vec<users::Model>)>> {
    let groups = groups::Entity::find()
        .order_by_asc(groups::Column::Name)
        .all(database)
        .await?;
    let users: HashMap<i64, users::Model> = users::Entity::find()
        .all(database)
        .await?
        .into_iter()
        .map(|user| (user.id, user))
        .collect();
    let mut members: HashMap<i64, Vec<users::Model>> = HashMap::new();
    for membership in group_memberships::Entity::find().all(database).await? {
        if let Some(user) = users.get(&membership.user_id) {
            members
                .entry(membership.group_id)
                .or_default()
                .push(user.clone());
        }
    }
    Ok(groups
        .into_iter()
        .map(|group| {
            let mut members = members.remove(&group.id).unwrap_or_default();
            members.sort_by(|first, second| first.username.cmp(&second.username));
            (group, members)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::users::{Identity, sign_in_with_identity};

    fn member_of(subject: &str, groups: &[&str]) -> Identity {
        Identity {
            issuer: "https://id.example".to_owned(),
            subject: subject.to_owned(),
            preferred_username: Some(subject.to_owned()),
            groups: Some(groups.iter().map(|group| (*group).to_owned()).collect()),
            ..Identity::default()
        }
    }

    #[tokio::test]
    async fn groups_are_listed_with_their_members() {
        let database = crate::testing::database().await;
        sign_in_with_identity(&database, &member_of("carol", &["family"]))
            .await
            .unwrap();
        sign_in_with_identity(&database, &member_of("alice", &["family", "office"]))
            .await
            .unwrap();
        sign_in_with_identity(&database, &member_of("alice", &["family"]))
            .await
            .unwrap();

        let listed: Vec<(String, Vec<String>)> = list(&database)
            .await
            .unwrap()
            .into_iter()
            .map(|(group, members)| {
                (
                    group.name,
                    members.into_iter().map(|member| member.username).collect(),
                )
            })
            .collect();

        assert_eq!(
            listed,
            [
                (
                    "family".to_owned(),
                    vec!["alice".to_owned(), "carol".to_owned()]
                ),
                ("office".to_owned(), vec![]),
            ]
        );
    }
}
