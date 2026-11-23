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
        .user()
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
    // The addresses to add the printer with: the plain one, and the personal one, which
    // carries the secret.
    assert!(html.contains(">ipp://recto.test:80/ipp/print<"), "{html}");
    assert!(
        html.contains(&format!(">ipp://recto.test:80/device/{secret}/ipp/print<")),
        "{html}"
    );

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
    let response = harness
        .send(form(
            "/account/devices",
            Some(&cookie),
            &[("name", "Work\tlaptop")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let html = text(response).await;
    assert!(
        html.contains(
            "The device name cannot contain line breaks, tabs or other control characters."
        ),
        "{html}"
    );
    assert!(!html.contains("1 to 64 characters"), "{html}");

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
async fn a_flood_of_device_guesses_does_not_block_sign_in() {
    let harness = Harness::new().await;
    harness.local_user("alice", ALICE_PASSWORD).await;
    let devices = DeviceAuth {
        // The test server is plain HTTP.
        plain_allowed: true,
        ..DeviceAuth::new(&harness.auth, "recto")
    };
    // Behind a proxy every client has the proxy's address, which is the peer of the requests
    // the harness sends.
    // A secret refused once is refused again without counting, so only new guesses drain the
    // budget.
    let mut refusals = Vec::new();
    for number in 0..100_u64 {
        let mut bytes = [0; 20];
        bytes[..8].copy_from_slice(&number.to_be_bytes());
        let guess = data_encoding::BASE32_NOPAD
            .encode(&bytes)
            .to_ascii_lowercase();
        refusals.push(
            device_auth::authenticate(&devices, &device_request(PEER, &guess), None)
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
                let posted = request.method() == axum::http::Method::POST;
                let response = harness.send(request).await;
                // The printer's address exists, but not for a device on the network over
                // plain HTTP, whatever its secret.
                let expected = if posted && uri.ends_with("/ipp/print") {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NOT_FOUND
                };
                assert_eq!(response.status(), expected, "{uri}");
                assert!(!response.headers().contains_key(header::LOCATION), "{uri}");
                assert!(text(response).await.is_empty(), "{uri}");
            }
        }
    }
}

#[tokio::test]
async fn administrators_end_a_users_device_passwords() {
    let harness = Harness::new().await;
    let admin = harness.administrator("root-subject", "root").await;
    let bob_id = harness.local_user("bob", ALICE_PASSWORD).await;
    let now = chrono::Utc::now();
    let phone = device_passwords::create(&harness.database, bob_id, "Phone", now)
        .await
        .unwrap();
    device_passwords::create(&harness.database, bob_id, "Laptop", now)
        .await
        .unwrap();

    // The Users page counts them, with a form that revokes them all.
    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    let html = text(response).await;
    assert!(html.contains("2 device passwords"), "{html}");
    let revoke = format!("/administration/users/{bob_id}/devices/revoke");
    assert!(html.contains(&format!("action=\"{revoke}\"")), "{html}");
    assert!(html.contains("Revoke devices"), "{html}");

    // Only administrators may, and only from recto's own pages.
    let bob = harness.sign_in_with_password("bob", ALICE_PASSWORD).await;
    let response = harness.send(form(&revoke, Some(&bob), &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = harness
        .send(
            Request::post(revoke.as_str())
                .header(header::COOKIE, &admin)
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        owner_of(&harness, &phone.secret).await.as_deref(),
        Some("bob")
    );

    let response = harness.send(form(&revoke, Some(&admin), &[])).await;
    let done = format!("/administration/users?done=devices_revoked&user={bob_id}");
    assert_redirect(&response, &done);
    assert_eq!(owner_of(&harness, &phone.secret).await, None);
    let html = text(harness.send(page(&done, Some(&admin))).await).await;
    assert!(
        html.contains("The device passwords of bob were revoked"),
        "{html}"
    );
    assert!(!html.contains(&format!("action=\"{revoke}\"")), "{html}");
    let response = harness
        .send(form(
            "/administration/users/999999/devices/revoke",
            Some(&admin),
            &[],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Setting the password revokes the ones made since; bob's own change would not.
    let tablet = device_passwords::create(&harness.database, bob_id, "Tablet", now)
        .await
        .unwrap();
    let response = harness
        .send(form(
            &format!("/administration/users/{bob_id}/password"),
            Some(&admin),
            &[("password", "a brand new password")],
        ))
        .await;
    assert_redirect(
        &response,
        &format!("/administration/users?done=password_set&user={bob_id}"),
    );
    assert_eq!(owner_of(&harness, &tablet.secret).await, None);
}

#[tokio::test]
async fn os_integration_off_hides_the_device_counts() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    let admin = harness.administrator("root-subject", "root").await;
    let bob_id = harness.local_user("bob", ALICE_PASSWORD).await;
    let phone = device_passwords::create(&harness.database, bob_id, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let response = harness
        .send(page("/administration/users", Some(&admin)))
        .await;
    let html = text(response).await;
    assert!(!html.contains("1 device password"), "{html}");
    assert!(!html.contains("Revoke devices"), "{html}");
    let revoke = format!("/administration/users/{bob_id}/devices/revoke");
    let response = harness.send(form(&revoke, Some(&admin), &[])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        owner_of(&harness, &phone.secret).await.as_deref(),
        Some("bob")
    );
}

#[tokio::test]
async fn device_forms_refuse_cross_site_posts() {
    let harness = Harness::new().await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let carol = harness.user_id("carol").await;
    let phone = device_passwords::create(&harness.database, carol, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let revoke = format!("/account/devices/{}/revoke", phone.id);
    for (uri, fields) in [
        ("/account/devices", vec![("name", "Evil")]),
        (revoke.as_str(), vec![]),
    ] {
        let mut request = form(uri, Some(&cookie), &fields);
        let headers = request.headers_mut();
        headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
        let response = harness.send(request).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
        // Nothing of the secret page, which only a same-site post may get.
        assert!(secrets_in(&text(response).await).is_empty(), "{uri}");
    }
    let listed = device_passwords::list(&harness.database, carol)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        owner_of(&harness, &phone.secret).await.as_deref(),
        Some("carol")
    );
}

/// A form post from the pages of a recto served over HTTPS.
fn secure_form(uri: &str, cookie: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let mut request = form(uri, Some(cookie), fields);
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://recto.test".parse().unwrap());
    request
}

/// Asks for the profile of `secret` and follows the redirect to it, as a browser does.
async fn download_profile(harness: &Harness, cookie: &str, secret: &str) -> Response<Body> {
    let response = harness
        .send(secure_form(
            "/account/devices/profile",
            cookie,
            &[("secret", secret)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let target = location(&response);
    assert!(target.starts_with("/account/devices/profile/"), "{target}");
    // The address the browser keeps in its history holds a ticket, never the secret.
    assert!(!target.contains(secret), "{target}");
    harness.send(page(&target, Some(cookie))).await
}

/// A property list value, enough of the format to check a profile's structure.
#[derive(Debug, PartialEq)]
enum Plist {
    Dict(Vec<(String, Plist)>),
    Array(Vec<Plist>),
    String(String),
    Integer(i64),
    Bool(bool),
}

impl Plist {
    fn parse(xml: &str) -> Plist {
        use quick_xml::{Reader, events::Event};
        // Whitespace between elements is gathered too, and dropped by the next start tag.
        let mut reader = Reader::from_str(xml);
        let mut stack: Vec<(String, Vec<Plist>)> = Vec::new();
        let mut text = String::new();
        let mut root = None;
        loop {
            match reader.read_event().unwrap() {
                Event::Start(start) => {
                    let name = start.name().as_ref().to_owned();
                    stack.push((name, Vec::new()));
                    text.clear();
                }
                Event::Empty(empty) => {
                    let value = match empty.name().as_ref() {
                        "true" => Plist::Bool(true),
                        "false" => Plist::Bool(false),
                        other => panic!("unexpected <{other}/>"),
                    };
                    stack.last_mut().unwrap().1.push(value);
                }
                Event::Text(content) => text.push_str(&content.xml10_content()),
                Event::GeneralRef(reference) => {
                    let name = reference.xml10_content();
                    text.push_str(quick_xml::escape::resolve_predefined_entity(&name).unwrap());
                }
                Event::End(_) => {
                    let (name, children) = stack.pop().unwrap();
                    let value = match name.as_str() {
                        "plist" => {
                            root = children.into_iter().next();
                            continue;
                        }
                        "dict" => {
                            let mut entries = Vec::new();
                            let mut children = children.into_iter();
                            while let Some(key) = children.next() {
                                let Plist::String(key) = key else {
                                    panic!("a dict key that is not a <key>")
                                };
                                entries.push((key, children.next().expect("a value per key")));
                            }
                            Plist::Dict(entries)
                        }
                        "array" => Plist::Array(children),
                        "key" | "string" => Plist::String(std::mem::take(&mut text)),
                        "integer" => Plist::Integer(std::mem::take(&mut text).parse().unwrap()),
                        other => panic!("unexpected <{other}>"),
                    };
                    stack.last_mut().unwrap().1.push(value);
                }
                Event::Eof => break,
                _ => {}
            }
        }
        root.expect("a <plist> with one value")
    }

    fn get(&self, key: &str) -> &Plist {
        let Plist::Dict(entries) = self else {
            panic!("{self:?} is not a dict")
        };
        let mut found = entries.iter().filter(|(name, _)| name == key);
        let value = &found
            .next()
            .unwrap_or_else(|| panic!("no {key} in {self:?}"))
            .1;
        assert!(found.next().is_none(), "{key} twice");
        value
    }

    fn string(&self, key: &str) -> &str {
        match self.get(key) {
            Plist::String(value) => value,
            other => panic!("{key} is {other:?}"),
        }
    }

    fn only(&self) -> &Plist {
        match self {
            Plist::Array(items) if items.len() == 1 => &items[0],
            other => panic!("{other:?} is not an array of one"),
        }
    }
}

#[tokio::test]
async fn the_profile_points_at_the_personal_address() {
    let harness = Harness::with(Setup {
        public_url: "https://recto.test/",
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    // The page showing a new password offers its profile, posting the secret back: it is never
    // in a link.
    let response = harness
        .send(secure_form(
            "/account/devices",
            &cookie,
            &[("name", "Carol's <iPhone> & co")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    let secret = secrets_in(&html).remove(0);
    assert!(
        html.contains("action=\"/account/devices/profile\""),
        "{html}"
    );
    assert!(
        html.contains(&format!("name=\"secret\" value=\"{secret}\"")),
        "{html}"
    );
    assert!(html.contains("Download for iPhone, iPad and Mac"), "{html}");
    let carol = harness.user_id("carol").await;
    let id = device_passwords::list(&harness.database, carol)
        .await
        .unwrap()[0]
        .id;

    let response = download_profile(&harness, &cookie, &secret).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/x-apple-aspen-config"
    );
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"recto.mobileconfig\""
    );
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let xml = text(response).await;
    assert!(
        xml.contains("<string>recto printer (Carol's &lt;iPhone&gt; &amp; co)</string>"),
        "{xml}"
    );

    let profile = Plist::parse(&xml);
    assert_eq!(profile.string("PayloadType"), "Configuration");
    assert_eq!(
        profile.string("PayloadDisplayName"),
        "recto printer (Carol's <iPhone> & co)"
    );
    assert_eq!(profile.get("PayloadVersion"), &Plist::Integer(1));
    let identifier = profile.string("PayloadIdentifier");
    assert!(identifier.ends_with(&format!(".{id}")), "{identifier}");
    uuid::Uuid::parse_str(profile.string("PayloadUUID")).unwrap();
    let payload = profile.get("PayloadContent").only();
    assert_eq!(payload.string("PayloadType"), "com.apple.airprint");
    assert_eq!(payload.get("PayloadVersion"), &Plist::Integer(1));
    assert!(payload.string("PayloadIdentifier").starts_with(identifier));
    let printer = payload.get("AirPrint").only();
    assert_eq!(printer.string("IPAddress"), "recto.test");
    assert_eq!(printer.get("Port"), &Plist::Integer(443));
    assert_eq!(printer.get("ForceTLS"), &Plist::Bool(true));
    assert_eq!(
        printer.string("ResourcePath"),
        format!("/device/{secret}/ipp/print")
    );

    // The download can be asked for again while the page is open.
    let again = download_profile(&harness, &cookie, &secret).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(text(again).await, xml);
}

#[tokio::test]
async fn a_revoked_or_foreign_secret_gets_no_profile() {
    let harness = Harness::with(Setup {
        public_url: "https://recto.test/",
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let bob = harness.sign_in("bob-subject", "bob").await;
    let carol = harness.user_id("carol").await;
    let phone = device_passwords::create(&harness.database, carol, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let ask = |cookie: &str, secret: &str| {
        secure_form("/account/devices/profile", cookie, &[("secret", secret)])
    };

    // Someone else's secret, a malformed one and none at all.
    for (who, secret) in [
        (&bob, phone.secret.as_str()),
        (&cookie, &"a".repeat(32)),
        (&cookie, "not a secret"),
        (&cookie, ""),
    ] {
        let response = harness.send(ask(who, secret)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{secret}");
        assert!(!response.headers().contains_key(header::LOCATION));
    }
    // Anonymous and cross-site posts are refused like every other form.
    let mut request = form(
        "/account/devices/profile",
        None,
        &[("secret", &phone.secret)],
    );
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://recto.test".parse().unwrap());
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let mut request = ask(&cookie, &phone.secret);
    let headers = request.headers_mut();
    headers.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
    let response = harness.send(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!text(response).await.contains(&phone.secret));

    // A ticket is the asking user's alone, and ends with the password.
    let response = harness.send(ask(&cookie, &phone.secret)).await;
    let ticket = location(&response);
    let response = harness.send(page(&ticket, Some(&bob))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!text(response).await.contains(&phone.secret));
    let response = harness.send(get(&ticket)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        device_passwords::revoke(&harness.database, carol, phone.id)
            .await
            .unwrap()
    );
    let response = harness.send(page(&ticket, Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(!text(response).await.contains(&phone.secret));
    let response = harness.send(ask(&cookie, &phone.secret)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = harness
        .send(page("/account/devices/profile/unknown", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_new_device_password_shows_the_scan_addresses_of_usable_scanners() {
    use recto_domain::access::{self, Access, DeviceKind};
    use recto_scanning::{ScannerInfo, ScannerService as _, escl};
    let scanner = |id: &str, vendor: &str, model: &str| ScannerInfo {
        id: id.into(),
        vendor: vendor.into(),
        model: model.into(),
        kind: "flatbed scanner".into(),
    };
    let capabilities = MockScannerService::default()
        .capabilities("mock:0")
        .await
        .unwrap();
    let harness = Harness::with(Setup {
        scanner: Some(MockScannerService::new(
            vec![
                scanner("canon:0", "Canon", "LiDE 300"),
                scanner("epson:0", "Epson", "V39"),
            ],
            capabilities,
        )),
        ..Setup::default()
    })
    .await;
    // Administrators only: carol may not use it.
    access::set(
        &harness.database,
        DeviceKind::Scanner,
        "epson:0",
        Access::Groups(vec![]),
    )
    .await
    .unwrap();
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let response = harness
        .send(form(
            "/account/devices",
            Some(&cookie),
            &[("name", "Laptop")],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = text(response).await;
    let secret = &secrets_in(&html)[0];
    let canon = escl::scanner_key("canon:0");
    assert!(html.contains("Canon LiDE 300"), "{html}");
    assert!(
        html.contains(&format!(
            ">http://recto.test:80/device/{secret}/eSCL/{canon}/<"
        )),
        "{html}"
    );
    assert!(!html.contains(&escl::scanner_key("epson:0")), "{html}");
    assert!(!html.contains("Epson V39"), "{html}");
}

#[tokio::test]
async fn os_integration_off_offers_no_profile() {
    let harness = Harness::with(Setup {
        os_integration: false,
        ..Setup::default()
    })
    .await;
    let cookie = harness.sign_in("carol-subject", "carol").await;
    let carol = harness.user_id("carol").await;
    let phone = device_passwords::create(&harness.database, carol, "Phone", chrono::Utc::now())
        .await
        .unwrap();
    let response = harness
        .send(form(
            "/account/devices/profile",
            Some(&cookie),
            &[("secret", &phone.secret)],
        ))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
