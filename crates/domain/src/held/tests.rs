use chrono::{TimeDelta, TimeZone};
use recto_entity::{held_jobs, print_jobs};
use sea_orm::{DatabaseConnection, EntityTrait, PaginatorTrait};

use super::*;
use crate::{
    device_passwords,
    ledger::{self, NewEntry, Prices},
    passwords::PasswordPolicy,
    users::{self, NewLocalUser},
};

const LIFETIME: TimeDelta = TimeDelta::hours(24);
const MAX_BYTES: i64 = 1000;

fn noon() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0).unwrap()
}

async fn user(database: &DatabaseConnection, username: &str, administrator: bool) -> i64 {
    users::create_local(
        database,
        NewLocalUser {
            username: username.to_owned(),
            display_name: username.to_owned(),
            email: None,
            password: "correct horse battery staple".to_owned(),
            is_administrator: administrator,
        },
        &PasswordPolicy { minimum_length: 12 },
    )
    .await
    .unwrap()
    .id
}

async fn device(database: &DatabaseConnection, user_id: i64, name: &str) -> i64 {
    device_passwords::create(database, user_id, name, noon())
        .await
        .unwrap()
        .id
}

fn stored(file: &str, size: i64) -> Stored {
    Stored {
        file: file.to_owned(),
        stored_format: "application/pdf".to_owned(),
        received_format: Some("application/pdf".to_owned()),
        document_pages: Some(2),
        size,
    }
}

fn new_held(user_id: i64, device: Option<i64>, file: &str, now: DateTime<Utc>) -> NewHeld {
    NewHeld {
        user_id,
        device_password_id: device,
        name: format!("{file}.pdf"),
        requested: "{}".to_owned(),
        stored: stored(file, 10),
        now,
        lifetime: LIFETIME,
    }
}

async fn hold(database: &DatabaseConnection, user_id: i64, file: &str, now: DateTime<Utc>) -> i64 {
    receive(database, new_held(user_id, None, file, now), MAX_BYTES)
        .await
        .unwrap()
}

async fn state(database: &DatabaseConnection, id: i64) -> Option<HeldState> {
    held_jobs::Entity::find_by_id(id)
        .one(database)
        .await
        .unwrap()
        .map(|row| HeldState::from_keyword(&row.state).unwrap())
}

async fn row(database: &DatabaseConnection, id: i64) -> held_jobs::Model {
    held_jobs::Entity::find_by_id(id)
        .one(database)
        .await
        .unwrap()
        .expect("the held job exists")
}

fn entry(user_id: i64) -> NewEntry {
    NewEntry {
        user_id,
        username: "alice".to_owned(),
        printer: "office".to_owned(),
        document_name: "a.pdf".to_owned(),
        document_format: "application/pdf".to_owned(),
        document_pages: Some(2),
        copies: 1,
        sides: "one-sided".to_owned(),
        color_mode: "monochrome".to_owned(),
        counts: ledger::counts(Some(2), 1, false, false, Prices::default()),
    }
}

/// What a release does once the quota check passed: the ledger entry and the released mark in
/// one transaction.
async fn release(database: &DatabaseConnection, job: &HeldJob, now: DateTime<Utc>) -> Result<i64> {
    let transaction = begin_write(database).await?;
    let entry_id = ledger::insert(&transaction, entry(job.user_id), now).await?;
    mark_released(&transaction, job.id, job.claim, entry_id, now).await?;
    transaction.commit().await?;
    Ok(entry_id)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_one_claim_wins() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let bob = user(&database, "bob", true).await;
    let id = hold(&database, alice, "a", noon()).await;
    // Not bob's, administrator or not.
    assert_eq!(claim(&database, bob, id, noon()).await.unwrap(), None);

    let (first, second) = tokio::join!(
        claim(&database, alice, id, noon()),
        claim(&database, alice, id, noon())
    );
    let won: Vec<HeldJob> = [first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(won.len(), 1);
    assert_eq!(won[0].state, HeldState::Releasing);
    assert_eq!(won[0].claim, 1);
    assert_eq!(state(&database, id).await, Some(HeldState::Releasing));
    assert_eq!(claim(&database, alice, id, noon()).await.unwrap(), None);
}

#[tokio::test]
async fn a_row_with_a_ledger_entry_never_returns_to_held() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let printed = hold(&database, alice, "printed", noon()).await;
    let stuck = hold(&database, alice, "stuck", noon()).await;
    let slow = hold(&database, alice, "slow", noon()).await;

    let job = claim(&database, alice, printed, noon())
        .await
        .unwrap()
        .unwrap();
    let entry_id = release(&database, &job, noon()).await.unwrap();
    claim(&database, alice, stuck, noon())
        .await
        .unwrap()
        .unwrap();
    let later = noon() + TimeDelta::minutes(5);
    claim(&database, alice, slow, later).await.unwrap().unwrap();

    // Claimed 16 minutes ago without reaching the ledger: returned.
    let swept = sweep(&database, noon() + TimeDelta::minutes(16))
        .await
        .unwrap();
    assert_eq!(swept.returned, 1);
    assert_eq!((swept.expired, swept.abandoned, swept.released), (0, 0, 0));
    assert_eq!(state(&database, stuck).await, Some(HeldState::Held));
    // Claimed 14 minutes ago: still in progress.
    let swept = sweep(&database, later + TimeDelta::minutes(14))
        .await
        .unwrap();
    assert_eq!(swept.returned, 0);
    assert_eq!(state(&database, slow).await, Some(HeldState::Releasing));
    let swept = sweep(&database, later + TimeDelta::minutes(16))
        .await
        .unwrap();
    assert_eq!(swept.returned, 1);
    assert_eq!(state(&database, slow).await, Some(HeldState::Held));

    // Released with its entry: never back, however long CUPS takes.
    let swept = sweep(&database, noon() + TimeDelta::hours(23))
        .await
        .unwrap();
    assert_eq!(swept.returned, 0);
    let released = row(&database, printed).await;
    assert_eq!(released.state, "released");
    assert_eq!(released.ledger_entry_id, Some(entry_id));
}

#[tokio::test]
async fn a_stale_claim_cannot_release() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let id = hold(&database, alice, "a", noon()).await;
    let first = claim(&database, alice, id, noon()).await.unwrap().unwrap();
    // The first release hangs; the sweep returns the job and a second release claims it.
    sweep(&database, noon() + TimeDelta::minutes(16))
        .await
        .unwrap();
    let later = noon() + TimeDelta::minutes(17);
    let second = claim(&database, alice, id, later).await.unwrap().unwrap();
    assert_eq!(second.claim, 2);

    assert!(matches!(
        release(&database, &first, later).await,
        Err(Error::NotFound)
    ));
    // Its ledger entry went with its transaction.
    assert_eq!(
        print_jobs::Entity::find().count(&database).await.unwrap(),
        0
    );
    // Nor can it put the job back under the second release.
    unclaim(&database, id, first.claim, Some("refused"), later)
        .await
        .unwrap();
    assert_eq!(state(&database, id).await, Some(HeldState::Releasing));

    let entry_id = release(&database, &second, later).await.unwrap();
    assert_eq!(row(&database, id).await.ledger_entry_id, Some(entry_id));
    assert_eq!(
        print_jobs::Entity::find().count(&database).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn a_refused_release_returns_the_job() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let id = hold(&database, alice, "a", noon()).await;

    // Refused before the ledger.
    let job = claim(&database, alice, id, noon()).await.unwrap().unwrap();
    unclaim(&database, id, job.claim, Some("quota"), noon())
        .await
        .unwrap();
    let held = get(&database, alice, id).await.unwrap().unwrap();
    assert_eq!(held.state, HeldState::Held);
    assert_eq!(held.refusal.as_deref(), Some("quota"));

    // Refused by CUPS after the ledger: the entry is refunded and the job returned, together.
    let job = claim(&database, alice, id, noon()).await.unwrap().unwrap();
    // A new attempt forgets the last refusal.
    assert_eq!(job.refusal, None);
    let entry = release(&database, &job, noon()).await.unwrap();
    // A released job never goes back by a plain unclaim: it has a ledger entry.
    unclaim(&database, id, job.claim, Some("refused"), noon())
        .await
        .unwrap();
    assert_eq!(state(&database, id).await, Some(HeldState::Released));
    // Nor for another entry than its own.
    assert!(matches!(
        return_refunded(&database, id, job.claim, entry + 1, "refused", noon()).await,
        Err(Error::NotFound)
    ));
    assert_eq!(state(&database, id).await, Some(HeldState::Released));
    assert!(!refunded(&database, entry).await);
    return_refunded(&database, id, job.claim, entry, "refused", noon())
        .await
        .unwrap();
    let held = get(&database, alice, id).await.unwrap().unwrap();
    assert_eq!(held.state, HeldState::Held);
    assert_eq!(held.ledger_entry_id, None);
    assert_eq!(held.refusal.as_deref(), Some("refused"));
    assert_eq!(held.file.as_deref(), Some("a"));
    assert!(refunded(&database, entry).await);
}

async fn refunded(database: &DatabaseConnection, entry: i64) -> bool {
    print_jobs::Entity::find_by_id(entry)
        .one(database)
        .await
        .unwrap()
        .unwrap()
        .refunded
}

#[tokio::test]
async fn the_sweep_follows_the_rules() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let phone = device(&database, alice, "Phone").await;
    let now = noon() + LIFETIME + TimeDelta::minutes(1);

    let expired = hold(&database, alice, "expired", noon()).await;
    let fresh = hold(&database, alice, "fresh", noon() + TimeDelta::hours(1)).await;
    let abandoned = create_incoming(
        &database,
        alice,
        phone,
        "abandoned.pdf",
        "{}",
        now - TimeDelta::minutes(11),
        LIFETIME,
        MAX_BYTES,
    )
    .await
    .unwrap();
    let waiting = create_incoming(
        &database,
        alice,
        phone,
        "waiting.pdf",
        "{}",
        now - TimeDelta::minutes(5),
        LIFETIME,
        MAX_BYTES,
    )
    .await
    .unwrap();
    let old = hold(&database, alice, "old", noon()).await;
    let job = claim(&database, alice, old, noon()).await.unwrap().unwrap();
    release(&database, &job, noon()).await.unwrap();
    let recent = hold(&database, alice, "recent", noon()).await;
    let job = claim(&database, alice, recent, now - TimeDelta::hours(1))
        .await
        .unwrap()
        .unwrap();
    release(&database, &job, now - TimeDelta::hours(1))
        .await
        .unwrap();

    assert_eq!(
        files(&database).await.unwrap(),
        HashSet::from(["expired", "fresh", "old", "recent"].map(String::from))
    );
    let swept = sweep(&database, now).await.unwrap();
    // Counted apart: only the first are documents nobody released in time.
    assert_eq!((swept.expired, swept.abandoned, swept.released), (1, 1, 1));
    assert_eq!(swept.files, ["expired", "old"]);
    assert_eq!(swept.returned, 0);
    for gone in [expired, abandoned, old] {
        assert_eq!(state(&database, gone).await, None);
    }
    assert_eq!(state(&database, fresh).await, Some(HeldState::Held));
    assert_eq!(state(&database, waiting).await, Some(HeldState::Incoming));
    assert_eq!(state(&database, recent).await, Some(HeldState::Released));
    assert_eq!(sweep(&database, now).await.unwrap(), Swept::default());
}

#[tokio::test]
async fn the_cap_is_checked_before_and_at_insert() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let bob = user(&database, "bob", false).await;
    let phone = device(&database, alice, "Phone").await;

    // Bytes: 900 of 1000 held.
    let big = receive(
        &database,
        NewHeld {
            stored: stored("big", 900),
            ..new_held(alice, None, "big", noon())
        },
        MAX_BYTES,
    )
    .await
    .unwrap();
    assert!(room(&database, alice, 100, MAX_BYTES).await.unwrap());
    assert!(!room(&database, alice, 101, MAX_BYTES).await.unwrap());
    let too_big = NewHeld {
        stored: stored("too-big", 101),
        ..new_held(alice, None, "too-big", noon())
    };
    assert!(matches!(
        receive(&database, too_big, MAX_BYTES).await,
        Err(Error::Invalid(Problem::TooManyHeldJobs { .. }))
    ));
    let incoming = create_incoming(
        &database,
        alice,
        phone,
        "late.pdf",
        "{}",
        noon(),
        LIFETIME,
        MAX_BYTES,
    )
    .await
    .unwrap();
    assert!(matches!(
        attach(&database, phone, incoming, stored("late", 101), MAX_BYTES).await,
        Err(Error::Invalid(Problem::TooManyHeldJobs { .. }))
    ));
    assert_eq!(state(&database, incoming).await, Some(HeldState::Incoming));
    // Released jobs no longer count.
    let job = claim(&database, alice, big, noon()).await.unwrap().unwrap();
    release(&database, &job, noon()).await.unwrap();
    assert!(room(&database, alice, 990, MAX_BYTES).await.unwrap());
    assert!(
        attach(&database, phone, incoming, stored("late", 100), MAX_BYTES)
            .await
            .unwrap()
    );

    // Jobs: the incoming job attached above is one of 20.
    for number in 1..MAX_JOBS_PER_USER {
        hold(&database, alice, &format!("job-{number}"), noon()).await;
    }
    assert!(!room(&database, alice, 0, MAX_BYTES).await.unwrap());
    assert!(matches!(
        receive(
            &database,
            new_held(alice, None, "one-more", noon()),
            MAX_BYTES
        )
        .await,
        Err(Error::Invalid(Problem::TooManyHeldJobs { .. }))
    ));
    assert!(matches!(
        create_incoming(
            &database,
            alice,
            phone,
            "one-more.pdf",
            "{}",
            noon(),
            LIFETIME,
            MAX_BYTES,
        )
        .await,
        Err(Error::Invalid(Problem::TooManyHeldJobs { .. }))
    ));
    // The limits are per user.
    assert!(room(&database, bob, 0, MAX_BYTES).await.unwrap());
    hold(&database, bob, "bob", noon()).await;
}

/// Of two results, one succeeded and the other was refused for the cap, not for another error.
fn assert_one_too_many(first: Result<()>, second: Result<()>) {
    let refused = |result: &Result<()>| {
        matches!(result, Err(Error::Invalid(Problem::TooManyHeldJobs { .. })))
    };
    assert!(
        (first.is_ok() && refused(&second)) || (refused(&first) && second.is_ok()),
        "{first:?} {second:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_jobs_cannot_pass_the_cap_together() {
    // On PostgreSQL each insert runs on its own connection: without the user's row locked, two
    // jobs at 19 would both see room for one more.
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let phone = device(&database, alice, "Phone").await;
    for number in 1..MAX_JOBS_PER_USER {
        hold(&database, alice, &format!("job-{number}"), noon()).await;
    }
    for _ in 0..3 {
        let (first, second) = tokio::join!(
            receive(&database, new_held(alice, None, "a", noon()), MAX_BYTES),
            receive(&database, new_held(alice, None, "b", noon()), MAX_BYTES),
        );
        assert_one_too_many(first.map(|_| ()), second.map(|_| ()));
        let waiting = held_jobs::Entity::find()
            .filter(held_jobs::Column::UserId.eq(alice))
            .count(&database)
            .await
            .unwrap();
        assert_eq!(waiting, MAX_JOBS_PER_USER);
        // Back to 19 for the next round, and the same race with Create-Job.
        let newest = held_jobs::Entity::find()
            .filter(held_jobs::Column::UserId.eq(alice))
            .order_by_desc(held_jobs::Column::Id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        held_jobs::Entity::delete_by_id(newest.id)
            .exec(&database)
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            create_incoming(
                &database,
                alice,
                phone,
                "c.pdf",
                "{}",
                noon(),
                LIFETIME,
                MAX_BYTES
            ),
            create_incoming(
                &database,
                alice,
                phone,
                "d.pdf",
                "{}",
                noon(),
                LIFETIME,
                MAX_BYTES
            ),
        );
        assert_one_too_many(first.map(|_| ()), second.map(|_| ()));
        let newest = held_jobs::Entity::find()
            .filter(held_jobs::Column::UserId.eq(alice))
            .order_by_desc(held_jobs::Column::Id)
            .one(&database)
            .await
            .unwrap()
            .unwrap();
        held_jobs::Entity::delete_by_id(newest.id)
            .exec(&database)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn an_incoming_job_has_room_for_its_document() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let phone = device(&database, alice, "Phone").await;
    receive(
        &database,
        NewHeld {
            stored: stored("big", 800),
            ..new_held(alice, None, "big", noon())
        },
        MAX_BYTES,
    )
    .await
    .unwrap();
    for number in 2..MAX_JOBS_PER_USER {
        hold(&database, alice, &format!("job-{number}"), noon()).await;
    }
    let incoming = create_incoming(
        &database,
        alice,
        phone,
        "late.pdf",
        "{}",
        noon(),
        LIFETIME,
        MAX_BYTES,
    )
    .await
    .unwrap();
    // The queue is full, but the incoming job is in it already: only its document is new.
    assert!(!room(&database, alice, 0, MAX_BYTES).await.unwrap());
    let used = 800 + 10 * (MAX_JOBS_PER_USER as i64 - 2);
    let left = MAX_BYTES - used;
    assert!(
        room_for(&database, alice, incoming, left, MAX_BYTES)
            .await
            .unwrap()
    );
    assert!(
        !room_for(&database, alice, incoming, left + 1, MAX_BYTES)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn delete_only_takes_held_rows() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let bob = user(&database, "bob", true).await;
    let phone = device(&database, alice, "Phone").await;
    let laptop = device(&database, alice, "Laptop").await;
    let held = hold(&database, alice, "held", noon()).await;
    let releasing = hold(&database, alice, "releasing", noon()).await;
    claim(&database, alice, releasing, noon())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(delete(&database, alice, releasing).await.unwrap(), None);
    assert_eq!(
        state(&database, releasing).await,
        Some(HeldState::Releasing)
    );
    // Not bob's, administrator or not.
    assert_eq!(delete(&database, bob, held).await.unwrap(), None);
    assert_eq!(
        delete(&database, alice, held).await.unwrap(),
        Some(Deleted {
            file: Some("held".to_owned())
        })
    );
    assert_eq!(state(&database, held).await, None);
    assert_eq!(delete(&database, alice, held).await.unwrap(), None);

    // A device cancels its own jobs still waiting for their document.
    let incoming = create_incoming(
        &database,
        alice,
        phone,
        "a.pdf",
        "{}",
        noon(),
        LIFETIME,
        MAX_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(
        delete_for_device(&database, laptop, incoming)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        delete_for_device(&database, phone, incoming).await.unwrap(),
        Some(Deleted { file: None })
    );
    // A held job is completed for the device: only its owner deletes it, on the Jobs page.
    let sent = receive(
        &database,
        new_held(alice, Some(phone), "sent", noon()),
        MAX_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(
        delete_for_device(&database, phone, sent).await.unwrap(),
        None
    );
    assert_eq!(state(&database, sent).await, Some(HeldState::Held));
    claim(&database, alice, sent, noon())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        delete_for_device(&database, phone, sent).await.unwrap(),
        None
    );
    assert_eq!(state(&database, sent).await, Some(HeldState::Releasing));
}

#[tokio::test]
async fn a_device_counts_its_incoming_jobs() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let phone = device(&database, alice, "Phone").await;
    let laptop = device(&database, alice, "Laptop").await;
    for (device, name) in [(phone, "a.pdf"), (phone, "b.pdf"), (laptop, "c.pdf")] {
        create_incoming(
            &database,
            alice,
            device,
            name,
            "{}",
            noon(),
            LIFETIME,
            MAX_BYTES,
        )
        .await
        .unwrap();
    }
    // A job with its document is held, no longer incoming.
    receive(
        &database,
        new_held(alice, Some(phone), "sent", noon()),
        MAX_BYTES,
    )
    .await
    .unwrap();
    assert_eq!(incoming_count(&database, phone).await.unwrap(), 2);
    assert_eq!(incoming_count(&database, laptop).await.unwrap(), 1);
}

#[tokio::test]
async fn an_expired_job_cannot_be_claimed() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let id = hold(&database, alice, "old", noon()).await;
    let expiry = noon() + LIFETIME;
    assert_eq!(claim(&database, alice, id, expiry).await.unwrap(), None);
    assert_eq!(state(&database, id).await, Some(HeldState::Held));
    assert!(
        claim(&database, alice, id, expiry - TimeDelta::seconds(1))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_job_names_a_device_of_its_owner_only() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let bob = user(&database, "bob", false).await;
    let bobs_phone = device(&database, bob, "Phone").await;
    assert!(matches!(
        receive(
            &database,
            new_held(alice, Some(bobs_phone), "mixed", noon()),
            MAX_BYTES
        )
        .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        create_incoming(
            &database,
            alice,
            bobs_phone,
            "mixed.pdf",
            "{}",
            noon(),
            LIFETIME,
            MAX_BYTES,
        )
        .await,
        Err(Error::NotFound)
    ));
    assert!(
        list_for_device(&database, bobs_phone)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn device_views_are_scoped() {
    let database = crate::testing::database().await;
    let alice = user(&database, "alice", false).await;
    let bob = user(&database, "bob", true).await;
    let phone = device(&database, alice, "Phone").await;
    let laptop = device(&database, alice, "Laptop").await;
    let from_phone = receive(
        &database,
        new_held(alice, Some(phone), "phone", noon()),
        MAX_BYTES,
    )
    .await
    .unwrap();
    let later = noon() + TimeDelta::minutes(1);
    let from_laptop = receive(
        &database,
        new_held(alice, Some(laptop), "laptop", later),
        MAX_BYTES,
    )
    .await
    .unwrap();
    let incoming = create_incoming(
        &database, alice, laptop, "next.pdf", "{}", later, LIFETIME, MAX_BYTES,
    )
    .await
    .unwrap();
    let ids = |jobs: Vec<HeldJob>| jobs.iter().map(|job| job.id).collect::<Vec<_>>();

    assert_eq!(
        ids(list_for_device(&database, phone).await.unwrap()),
        [from_phone]
    );
    assert_eq!(
        ids(list_for_device(&database, laptop).await.unwrap()),
        [incoming, from_laptop]
    );
    assert!(
        get_for_device(&database, phone, from_laptop)
            .await
            .unwrap()
            .is_none()
    );
    let job = get_for_device(&database, laptop, from_laptop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.device_password_id, Some(laptop));
    assert_eq!(job.name, "laptop.pdf");
    assert_eq!(job.stored_format.as_deref(), Some("application/pdf"));
    assert_eq!(job.document_pages, Some(2));
    assert_eq!(job.size, 10);
    assert_eq!(job.received_at, later);
    assert_eq!(job.expires_at, later + LIFETIME);

    // The Jobs page: the user's jobs from every device, without the incoming one.
    assert_eq!(
        ids(list(&database, alice).await.unwrap()),
        [from_laptop, from_phone]
    );
    assert!(list(&database, bob).await.unwrap().is_empty());
    assert!(get(&database, bob, from_phone).await.unwrap().is_none());
    assert!(get(&database, alice, from_phone).await.unwrap().is_some());

    // Attaching is the device's own.
    assert!(
        !attach(&database, phone, incoming, stored("next", 10), MAX_BYTES)
            .await
            .unwrap()
    );
    assert!(
        attach(&database, laptop, incoming, stored("next", 10), MAX_BYTES)
            .await
            .unwrap()
    );
    let attached = get(&database, alice, incoming).await.unwrap().unwrap();
    assert_eq!(attached.state, HeldState::Held);
    assert_eq!(attached.file.as_deref(), Some("next"));
    assert!(
        !attach(&database, laptop, incoming, stored("again", 10), MAX_BYTES)
            .await
            .unwrap()
    );

    // A revoked password leaves its jobs to their owner.
    device_passwords::revoke(&database, alice, laptop)
        .await
        .unwrap();
    assert_eq!(
        get(&database, alice, from_laptop)
            .await
            .unwrap()
            .unwrap()
            .device_password_id,
        None
    );
}
