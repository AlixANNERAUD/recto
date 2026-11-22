//! Device passwords: created and revoked on the Account page, never a way to sign in, and with
//! a failure limiter of their own.

use std::net::IpAddr;

use axum::http::request::Parts;
use recto::device_auth::{self, DeviceAuth, Refusal};
use recto_domain::device_passwords;

use super::*;

/// The secrets a page shows: text of exactly 32 base32 characters between two tags.
fn secrets_in(html: &str) -> Vec<String> {
    html.split('>')
        .filter_map(|chunk| chunk.split('<').next())
        .filter(|text| {
            text.len() == device_passwords::SECRET_LENGTH
                && text
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
        })
        .map(str::to_owned)
        .collect()
}

async fn owner_of(harness: &Harness, secret: &str) -> Option<String> {
    device_passwords::authenticate(&harness.database, secret, chrono::Utc::now())
        .await
        .unwrap()
        .map(|user| user.username)
}

#[tokio::test]
async fn a_single_sign_on_user_creates_and_revokes_a_device_password() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    // Without a password, the Account page is there for the devices alone.
    let response = harness.send(page("/", Some(&cookie))).await;
    assert!(text(response).await.contains("href=\"/account\""));
    let response = harness.send(page("/account", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("Devices"), "{html}");
    assert!(html.contains("no device passwords yet"), "{html}");
    assert!(!html.contains("action=\"/account/password\""), "{html}");

    let response = harness
        .send(form(
            "/account/devices",
            Some(&cookie),
            &[("name", "  Studio laptop ")],
        ))
        .await;
    // The page itself, never a redirect: the secret stays out of addresses and history.
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(header::LOCATION));
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let html = text(response).await;
    let secrets = secrets_in(&html);
    assert_eq!(secrets.len(), 1, "{html}");
    let secret = &secrets[0];
    assert_eq!(owner_of(&harness, secret).await.as_deref(), Some("carol"));
    assert!(html.contains("Device password for Studio laptop"), "{html}");
    assert!(html.contains("shown only once"), "{html}");
    assert!(html.contains(">carol<"), "{html}");

    // Listed by name, the secret never shown again.
    let response = harness.send(page("/account", Some(&cookie))).await;
    let html = text(response).await;
    assert!(html.contains("Studio laptop"), "{html}");
    // `owner_of` authenticated with it above.
    assert!(html.contains("last used just now"), "{html}");
    assert!(!html.contains(secret.as_str()), "{html}");
    let id = device_passwords::list(&harness.database, harness.user_id("carol").await)
        .await
        .unwrap()[0]
        .id;
    assert!(html.contains(&format!("action=\"/account/devices/{id}/revoke\"")));

    // Another user cannot revoke it, nor learn that it exists.
    let bob = harness.sign_in("bob-subject", "bob").await;
    let revoke = format!("/account/devices/{id}/revoke");
    let response = harness.send(form(&revoke, Some(&bob), &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(owner_of(&harness, secret).await.as_deref(), Some("carol"));

    let response = harness.send(form(&revoke, Some(&cookie), &[])).await;
    assert_redirect(&response, "/account?done=revoked");
    assert_eq!(owner_of(&harness, secret).await, None);
    let response = harness
        .send(page("/account?done=revoked", Some(&cookie)))
        .await;
    let html = text(response).await;
    assert!(html.contains("was revoked"), "{html}");
    assert!(!html.contains("Studio laptop"), "{html}");
    let response = harness.send(form(&revoke, Some(&cookie), &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn refused_device_forms_say_why() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let long = "x".repeat(65);
    let response = harness
        .send(form("/account/devices", Some(&cookie), &[("name", &long)]))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(html.contains("1 to 64 characters"), "{html}");
    // What was typed comes back.
    assert!(html.contains(&format!("value=\"{long}\"")), "{html}");
    assert!(secrets_in(&html).is_empty(), "{html}");

    let carol = harness.user_id("carol").await;
    for number in 0..device_passwords::MAX_PER_USER {
        device_passwords::create(
            &harness.database,
            carol,
            &format!("Device {number}"),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    }
    let response = harness
        .send(form(
            "/account/devices",
            Some(&cookie),
            &[("name", "One more")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(
        html.contains("You already have 20 device passwords"),
        "{html}"
    );

    // Cross-site posts and anonymous visitors are refused like every other form.
    let response = harness
        .send(
            Request::post("/account/devices")
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("name=Evil"))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = harness
        .send(form("/account/devices", None, &[("name", "Phone")]))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_device_password_cannot_sign_in() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let created = device_passwords::create(&harness.database, alice, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let response = harness
        .send(form(
            "/sign-in",
            None,
            &[("login", "alice"), ("password", &created.secret)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(session_cookie(response.headers()).is_none());
    let html = text(response).await;
    assert!(
        html.contains("The username or password is wrong."),
        "{html}"
    );
}

#[tokio::test]
async fn os_integration_off_hides_devices() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let alice = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    let response = harness.send(page("/account", Some(&alice))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    assert!(html.contains("action=\"/account/password\""), "{html}");
    assert!(!html.contains("action=\"/account/devices\""), "{html}");

    let response = harness
        .send(form("/account/devices", Some(&alice), &[("name", "Phone")]))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let id = harness.user_id("alice").await;
    assert!(
        device_passwords::list(&harness.database, id)
            .await
            .unwrap()
            .is_empty()
    );
    let created = device_passwords::create(&harness.database, id, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let revoke = format!("/account/devices/{}/revoke", created.id);
    let response = harness.send(form(&revoke, Some(&alice), &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // A single sign-on user has nothing on the page.
    let carol = harness.sign_in("carol-subject", "carol").await;
    let response = harness.send(page("/", Some(&carol))).await;
    assert!(!text(response).await.contains("href=\"/account\""));
    let response = harness.send(page("/account", Some(&carol))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A request from `peer` with Basic credentials holding `password`.
fn device_request(peer: [u8; 4], password: &str) -> Parts {
    let credentials = data_encoding::BASE64.encode(format!("iphone:{password}").as_bytes());
    let request = from_address(
        Request::post("/ipp/print")
            .header(header::AUTHORIZATION, format!("Basic {credentials}"))
            .body(Body::empty())
            .unwrap(),
        peer,
    );
    request.into_parts().0
}

#[tokio::test]
async fn a_revoked_phone_does_not_block_sign_in() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let devices = DeviceAuth {
        // The test server is plain HTTP.
        plain_allowed: true,
        ..DeviceAuth::new(&harness.auth, "recto")
    };
    // Behind a proxy every client has the proxy's address, which is the peer of the requests
    // the harness sends.
    let revoked = device_request(PEER, &"r".repeat(32));
    let mut refusals = Vec::new();
    for _ in 0..100 {
        refusals.push(
            device_auth::authenticate(&devices, &revoked, None)
                .await
                .unwrap_err(),
        );
    }
    assert!(refusals.contains(&Refusal::TooMany));
    assert!(!devices.limiter.allow(IpAddr::from(PEER)));
    // Password sign-in from the same address is untouched.
    harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
}

#[tokio::test]
async fn a_personal_address_never_leaves_its_secret_in_a_redirect() {
    let harness = Harness::new().await;
    let alice = harness.local_user("alice", ALICE_PASSWORD).await;
    let created = device_passwords::create(&harness.database, alice, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let cookie = harness.sign_in_with_password("alice", ALICE_PASSWORD).await;
    for secret in [created.secret.as_str(), &"a".repeat(32)] {
        for uri in [
            format!("/device/{secret}"),
            format!("/device/{secret}/ipp/print"),
            format!("/device/{secret}/eSCL/ScannerCapabilities?x=1"),
        ] {
            // A browser opening the address, signed in or not, and a device posting to it.
            for request in [
                page(&uri, None),
                page(&uri, Some(&cookie)),
                form(&uri, None, &[]),
            ] {
                let response = harness.send(request).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
                assert!(!response.headers().contains_key(header::LOCATION), "{uri}");
                assert!(text(response).await.is_empty(), "{uri}");
            }
        }
    }
}
