//! Which groups may use a printer or a scanner. A device without a rule is open to everyone;
//! a rule limits it to the members of its groups, and administrators always pass.
//!
//! Devices are named as their service names them (a CUPS printer name, a SANE device id), so a
//! rule can outlive the device: [`forget`] drops it on request, and a device that comes back
//! is restricted again.

use std::collections::{BTreeSet, HashMap};

use chrono::Utc;
use recto_entity::{device_access, device_access_groups, group_memberships};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection,
    EntityTrait, QueryFilter, QueryOrder,
};

use crate::{database::begin_write, error::Result};

/// The kind of device a rule is for: the same name can be a printer and a scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Printer,
    Scanner,
}

/// Who may use a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    Everyone,
    /// The members of these groups (and administrators). Possibly empty: administrators only.
    Groups(Vec<i64>),
}

impl DeviceKind {
    fn as_str(self) -> &'static str {
        match self {
            DeviceKind::Printer => "printer",
            DeviceKind::Scanner => "scanner",
        }
    }
}

/// The restricted devices of a kind by name; open devices have no entry.
pub async fn rules(db: &impl ConnectionTrait, kind: DeviceKind) -> Result<HashMap<String, Access>> {
    let rows = device_access::Entity::find()
        .filter(device_access::Column::Kind.eq(kind.as_str()))
        .all(db)
        .await?;
    let ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let mut groups: HashMap<i64, Vec<i64>> = HashMap::new();
    if !ids.is_empty() {
        let links = device_access_groups::Entity::find()
            .filter(device_access_groups::Column::AccessId.is_in(ids))
            .order_by_asc(device_access_groups::Column::GroupId)
            .all(db)
            .await?;
        for link in links {
            groups
                .entry(link.access_id)
                .or_default()
                .push(link.group_id);
        }
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let groups = groups.remove(&row.id).unwrap_or_default();
            (row.device, Access::Groups(groups))
        })
        .collect())
}

/// Sets who may use a device, in one transaction, so a reader never sees a rule half written.
/// `Everyone` removes the rule.
///
/// On SQLite the transaction takes the write lock first, so two saves of the same device run one
/// after the other. On PostgreSQL they can overlap: both remove the old rule, and the second to
/// insert the new one fails on the unique device name, an error rather than a mixed rule.
pub async fn set(
    db: &DatabaseConnection,
    kind: DeviceKind,
    device: &str,
    access: Access,
) -> Result<()> {
    let transaction = begin_write(db).await?;
    remove(&transaction, kind, device).await?;
    if let Access::Groups(groups) = access {
        let row = device_access::ActiveModel {
            kind: Set(kind.as_str().to_owned()),
            device: Set(device.to_owned()),
            created_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        }
        .insert(&transaction)
        .await?;
        let unique: BTreeSet<i64> = groups.into_iter().collect();
        for group_id in unique {
            device_access_groups::ActiveModel {
                access_id: Set(row.id),
                group_id: Set(group_id),
            }
            .insert(&transaction)
            .await?;
        }
    }
    transaction.commit().await?;
    Ok(())
}

/// Removes the rule of a device.
pub async fn forget(db: &impl ConnectionTrait, kind: DeviceKind, device: &str) -> Result<()> {
    remove(db, kind, device).await
}

/// Deletes the rule; its groups go with it, as the database enforces foreign keys (on SQLite,
/// [`connect`](crate::database::connect) turns them on).
async fn remove(db: &impl ConnectionTrait, kind: DeviceKind, device: &str) -> Result<()> {
    device_access::Entity::delete_many()
        .filter(device_access::Column::Kind.eq(kind.as_str()))
        .filter(device_access::Column::Device.eq(device))
        .exec(db)
        .await?;
    Ok(())
}

/// The ids of the groups a user belongs to.
pub async fn group_ids_of(db: &impl ConnectionTrait, user_id: i64) -> Result<Vec<i64>> {
    let memberships = group_memberships::Entity::find()
        .filter(group_memberships::Column::UserId.eq(user_id))
        .all(db)
        .await?;
    Ok(memberships.into_iter().map(|row| row.group_id).collect())
}

/// Whether a user may use a device with this rule (`None` when it has none).
pub fn allows(access: Option<&Access>, administrator: bool, groups: &[i64]) -> bool {
    match access {
        None | Some(Access::Everyone) => true,
        Some(Access::Groups(allowed)) => {
            administrator || allowed.iter().any(|group| groups.contains(group))
        }
    }
}

/// What one user may use among the devices of a kind, read once for a whole list.
#[derive(Clone, Debug)]
pub struct Usable {
    rules: HashMap<String, Access>,
    administrator: bool,
    groups: Vec<i64>,
}

impl Usable {
    /// Reads the rules and the user's groups. Administrators may use everything, so for them
    /// nothing is read and a broken rule table cannot lock them out.
    pub async fn read(
        db: &impl ConnectionTrait,
        kind: DeviceKind,
        user_id: i64,
        administrator: bool,
    ) -> Result<Self> {
        if administrator {
            return Ok(Self {
                rules: HashMap::new(),
                administrator,
                groups: Vec::new(),
            });
        }
        let rules = rules(db, kind).await?;
        // Without any rule every device is open, and the groups would decide nothing.
        let groups = if rules.is_empty() {
            Vec::new()
        } else {
            group_ids_of(db, user_id).await?
        };
        Ok(Self {
            rules,
            administrator,
            groups,
        })
    }

    pub fn allows(&self, device: &str) -> bool {
        allows(self.rules.get(device), self.administrator, &self.groups)
    }
}

#[cfg(test)]
mod tests {
    use recto_entity::groups;
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    use super::*;
    use crate::users::{Identity, sign_in_with_identity};

    async fn group(db: &sea_orm::DatabaseConnection, name: &str) -> i64 {
        groups::ActiveModel {
            name: Set(name.to_owned()),
            created_at: Set(Utc::now().fixed_offset()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn devices_are_open_to_everyone_by_default() {
        let db = crate::testing::database().await;
        assert!(rules(&db, DeviceKind::Printer).await.unwrap().is_empty());
        assert!(rules(&db, DeviceKind::Scanner).await.unwrap().is_empty());
        assert!(allows(None, false, &[]));
        assert!(allows(Some(&Access::Everyone), false, &[]));
    }

    #[tokio::test]
    async fn a_restriction_round_trips() {
        let db = crate::testing::database().await;
        let first = group(&db, "staff").await;
        set(
            &db,
            DeviceKind::Printer,
            "Office Laser",
            Access::Groups(vec![first]),
        )
        .await
        .unwrap();
        let printers = rules(&db, DeviceKind::Printer).await.unwrap();
        assert_eq!(
            printers,
            HashMap::from([("Office Laser".to_owned(), Access::Groups(vec![first]))])
        );
        // Kinds are separate.
        assert!(rules(&db, DeviceKind::Scanner).await.unwrap().is_empty());

        // Replacing swaps the groups; an empty list is a rule too.
        let second = group(&db, "lab").await;
        set(
            &db,
            DeviceKind::Printer,
            "Office Laser",
            Access::Groups(vec![second]),
        )
        .await
        .unwrap();
        assert_eq!(
            rules(&db, DeviceKind::Printer).await.unwrap()["Office Laser"],
            Access::Groups(vec![second])
        );
        set(
            &db,
            DeviceKind::Printer,
            "Office Laser",
            Access::Groups(vec![]),
        )
        .await
        .unwrap();
        assert_eq!(
            rules(&db, DeviceKind::Printer).await.unwrap()["Office Laser"],
            Access::Groups(vec![])
        );

        set(&db, DeviceKind::Printer, "Office Laser", Access::Everyone)
            .await
            .unwrap();
        assert!(rules(&db, DeviceKind::Printer).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn forgetting_a_device_removes_its_rule() {
        let db = crate::testing::database().await;
        set(&db, DeviceKind::Scanner, "net:a", Access::Groups(vec![]))
            .await
            .unwrap();
        forget(&db, DeviceKind::Scanner, "net:a").await.unwrap();
        assert!(rules(&db, DeviceKind::Scanner).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn removing_a_rule_removes_its_groups() {
        let db = crate::testing::database().await;
        let staff = group(&db, "staff").await;
        set(&db, DeviceKind::Printer, "p", Access::Groups(vec![staff]))
            .await
            .unwrap();
        set(&db, DeviceKind::Printer, "p", Access::Everyone)
            .await
            .unwrap();
        let links = device_access_groups::Entity::find().all(&db).await.unwrap();
        assert!(links.is_empty());
    }

    #[test]
    fn administrators_and_members_may_use_restricted_devices() {
        let rule = Access::Groups(vec![1]);
        assert!(allows(Some(&rule), false, &[1]));
        assert!(allows(Some(&rule), false, &[2, 1]));
        assert!(!allows(Some(&rule), false, &[2]));
        assert!(!allows(Some(&rule), false, &[]));
        assert!(allows(Some(&rule), true, &[]));
        let nobody = Access::Groups(vec![]);
        assert!(!allows(Some(&nobody), false, &[1]));
        assert!(allows(Some(&nobody), true, &[]));
    }

    #[tokio::test]
    async fn deleting_a_group_drops_it_from_rules() {
        let db = crate::testing::database().await;
        let first = group(&db, "staff").await;
        let second = group(&db, "lab").await;
        set(
            &db,
            DeviceKind::Printer,
            "p",
            Access::Groups(vec![first, second]),
        )
        .await
        .unwrap();
        groups::Entity::delete_by_id(first).exec(&db).await.unwrap();
        assert_eq!(
            rules(&db, DeviceKind::Printer).await.unwrap()["p"],
            Access::Groups(vec![second])
        );
    }

    #[tokio::test]
    async fn a_user_has_the_ids_of_their_groups() {
        let db = crate::testing::database().await;
        let identity = Identity {
            issuer: "https://id.example".to_owned(),
            subject: "alice".to_owned(),
            preferred_username: Some("alice".to_owned()),
            groups: Some(vec!["staff".to_owned(), "lab".to_owned()]),
            ..Identity::default()
        };
        let user = sign_in_with_identity(&db, &identity).await.unwrap();
        let ids = group_ids_of(&db, user.id).await.unwrap();
        assert_eq!(ids.len(), 2);
        assert!(group_ids_of(&db, user.id + 100).await.unwrap().is_empty());
    }
}
