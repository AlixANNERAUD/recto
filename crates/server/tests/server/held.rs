//! Held print jobs on the server: swept whether or not OS integration is on, and no file left
//! behind that no row names.

use std::time::SystemTime;

use chrono::{TimeDelta, Utc};
use recto::serve::{remove_orphan_held_files, sweep_held_once};
use recto_domain::held::{self, NewHeld, Stored};

use super::*;

/// Writes a document and holds it for alice, received at `received_at`.
async fn hold(harness: &Harness, alice: i64, received_at: chrono::DateTime<Utc>) -> (i64, String) {
    let file = harness.held.write(PDF).unwrap();
    let id = held::receive(
        &harness.database,
        NewHeld {
            user_id: alice,
            device_password_id: None,
            name: "minutes.pdf".into(),
            requested: "{}".into(),
            stored: Stored {
                file: file.clone(),
                stored_format: "application/pdf".into(),
                received_format: Some("application/pdf".into()),
                document_pages: Some(1),
                size: PDF.len() as i64,
            },
            now: received_at,
            lifetime: TimeDelta::hours(24),
        },
        10 * 1024 * 1024,
    )
    .await
    .unwrap();
    (id, file)
}

#[tokio::test]
async fn the_sweep_runs_with_os_integration_off() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    let alice = harness.local_user("alice", "correct horse battery").await;
    let now = Utc::now();
    let (expired, expired_file) = hold(&harness, alice, now - TimeDelta::hours(25)).await;
    let (kept, kept_file) = hold(&harness, alice, now - TimeDelta::hours(1)).await;

    sweep_held_once(&harness.database, &harness.held, now).await;

    assert!(
        held::get(&harness.database, alice, expired)
            .await
            .unwrap()
            .is_none()
    );
    assert!(harness.held.read(&expired_file).is_err());
    assert!(
        held::get(&harness.database, alice, kept)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(harness.held.read(&kept_file).unwrap(), PDF);
}

#[tokio::test]
async fn files_no_row_names_are_removed() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", "correct horse battery").await;
    let (_, kept) = hold(&harness, alice, Utc::now()).await;
    // A crash between writing the document and recording it.
    let orphan = harness.held.write(PDF).unwrap();

    // While recto runs, a document just written may be waiting for its row.
    sweep_held_once(&harness.database, &harness.held, Utc::now()).await;
    assert_eq!(harness.held.read(&orphan).unwrap(), PDF);

    // At startup nothing is being written.
    let removed =
        remove_orphan_held_files(&harness.database, &harness.held, SystemTime::now()).await;
    assert_eq!(removed, 1);
    assert!(harness.held.read(&orphan).is_err());
    assert_eq!(harness.held.read(&kept).unwrap(), PDF);
}
