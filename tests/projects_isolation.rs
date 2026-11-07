//! A project is its own installation: a credential of one installation (a superuser token, an
//! API key, a user token) must mean nothing in any other, whichever way it is addressed. See
//! `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;

async fn with(
    app: &Arc<App>,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let auth = format!("Bearer {token}");
    request(app, method, uri, &[("authorization", auth.as_str())], body).await
}

async fn make_project(app: &Arc<App>, owner: &str, name: &str) -> String {
    let (status, res) = with(
        app,
        owner,
        "POST",
        "/api/projects",
        Some(json!({
            "name": name,
            "superuserEmail": format!("{name}@example.com"),
            "superuserPassword": "project-password-1"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, login) = request(
        app,
        "POST",
        &format!("/p/{name}/api/collections/_superusers/auth-with-password"),
        &[],
        Some(
            json!({ "identity": format!("{name}@example.com"), "password": "project-password-1" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{login}");
    login["token"].as_str().unwrap().to_string()
}

/// An API key (owner role) of the installation at `prefix`.
async fn key_in(app: &Arc<App>, token: &str, prefix: &str) -> String {
    let (status, key) = with(
        app,
        token,
        "POST",
        &format!("{prefix}/api/team/keys"),
        Some(json!({ "name": "isolation", "role": "owner" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{key}");
    key["key"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credentials_stop_at_their_installation() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, main, _dir) = ready_app(&tdb).await;
    let a = make_project(&app, &main, "alpha").await;
    let b = make_project(&app, &main, "beta").await;
    // something to find in each
    for (prefix, token, marker) in [
        ("", &main, "main"),
        ("/p/alpha", &a, "alpha"),
        ("/p/beta", &b, "beta"),
    ] {
        let (status, res) = with(
            &app,
            token,
            "POST",
            &format!("{prefix}/api/collections"),
            Some(json!({ "name": format!("{marker}_data"), "fields": [{ "name": "t", "type": "text" }] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{res}");
    }
    let key_main = key_in(&app, &main, "").await;
    let key_a = key_in(&app, &a, "/p/alpha").await;

    // (credential, where it is from) against every installation: only its own answers
    let installations = [("main", ""), ("alpha", "/p/alpha"), ("beta", "/p/beta")];
    let credentials = [
        ("main token", "main", &main),
        ("alpha token", "alpha", &a),
        ("beta token", "beta", &b),
        ("main key", "main", &key_main),
        ("alpha key", "alpha", &key_a),
    ];
    for (what, home, credential) in credentials {
        for (name, prefix) in installations {
            let (status, res) = with(
                &app,
                credential,
                "GET",
                &format!("{prefix}/api/collections"),
                None,
            )
            .await;
            if name == home {
                assert_eq!(status, StatusCode::OK, "{what} in its own {name}: {res}");
            } else {
                assert!(
                    !status.is_success(),
                    "{what} listed collections of {name}: {status} {res}"
                );
                assert!(
                    !res.to_string().contains("_data"),
                    "{what} saw {name}'s collections: {res}"
                );
            }
            // writes and the account itself
            if name != home {
                let (status, res) = with(
                    &app,
                    credential,
                    "POST",
                    &format!("{prefix}/api/collections"),
                    Some(json!({ "name": "planted", "fields": [] })),
                )
                .await;
                assert!(!status.is_success(), "{what} created in {name}: {res}");
                let (status, res) = with(
                    &app,
                    credential,
                    "GET",
                    &format!("{prefix}/api/collections/_superusers/records"),
                    None,
                )
                .await;
                assert!(
                    !status.is_success(),
                    "{what} listed {name}'s superusers: {res}"
                );
            }
        }
        // management of projects belongs to the main installation's owners
        if home != "main" {
            let (status, res) = with(&app, credential, "GET", "/api/projects", None).await;
            assert!(!status.is_success(), "{what} listed projects: {res}");
            let (status, res) = with(
                &app,
                credential,
                "POST",
                "/api/projects",
                Some(json!({ "name": "evil", "superuserEmail": "e@example.com", "superuserPassword": "evil-password-1" })),
            )
            .await;
            assert!(!status.is_success(), "{what} created a project: {res}");
        }
    }
    // a project's own owner can't reach another project through the main installation's
    // project commands either
    let (status, res) = with(
        &app,
        &a,
        "PATCH",
        "/api/projects/beta/settings",
        Some(json!({})),
    )
    .await;
    assert!(
        !status.is_success(),
        "alpha token changed beta's settings: {res}"
    );
    let (status, res) = with(&app, &a, "DELETE", "/api/projects/beta", None).await;
    assert!(!status.is_success(), "alpha token deleted beta: {res}");

    // the project is still there
    let (status, _) = with(&app, &main, "GET", "/api/projects/beta", None).await;
    assert_eq!(status, StatusCode::OK);

    tdb.drop_db().await;
}
