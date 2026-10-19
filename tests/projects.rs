//! Projects: each in its own database, served under /p/{name}/. See `setup_flow.rs` for how
//! to point the tests at a database.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;

use common::*;
use slonbase::app::App;

const PROJECT_EMAIL: &str = "owner@example.com";
const PROJECT_PASSWORD: &str = "project-password-1";

async fn eventually(mut check: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..150 {
        if check().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    false
}

async fn database_exists(tdb: &TestDb, name: &str) -> bool {
    let (client, conn) = tokio_postgres::connect(&tdb.admin_url, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(conn);
    client
        .query_opt("SELECT 1 FROM pg_database WHERE datname = $1", &[&name])
        .await
        .unwrap()
        .is_some()
}

async fn sign_in(
    app: &Arc<App>,
    prefix: &str,
    email: &str,
    password: &str,
) -> (StatusCode, String) {
    let (status, body) = request(
        app,
        "POST",
        &format!("{prefix}/api/collections/_superusers/auth-with-password"),
        &[],
        Some(json!({ "identity": email, "password": password })),
    )
    .await;
    (
        status,
        body["token"].as_str().unwrap_or_default().to_string(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_are_separate_installations() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let bearer = format!("Bearer {token}");
    let admin = [("authorization", bearer.as_str())];
    let database = format!("{}_blog", tdb.name);

    // only superusers of the main installation manage projects
    let (status, _) = request(&app, "GET", "/api/projects", &[], None).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "{status}"
    );
    let (status, list) = request(&app, "GET", "/api/projects", &admin, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"], json!([]));

    let (status, res) = request(
        &app,
        "POST",
        "/api/projects",
        &admin,
        Some(json!({ "name": "Not Valid", "superuserEmail": "x", "superuserPassword": "1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for field in ["name", "superuserEmail", "superuserPassword"] {
        assert!(res["data"][field].is_object(), "{field}: {res}");
    }

    let (status, project) = request(
        &app,
        "POST",
        "/api/projects",
        &admin,
        Some(json!({
            "name": "blog", "superuserEmail": PROJECT_EMAIL, "superuserPassword": PROJECT_PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{project}");
    assert_eq!(project["status"], "ready", "{project}");
    assert_eq!(project["database"], database.as_str());
    assert_eq!(project["path"], "/p/blog/");
    assert!(database_exists(&tdb, &database).await);

    let (status, res) = request(
        &app,
        "POST",
        "/api/projects",
        &admin,
        Some(json!({
            "name": "blog", "superuserEmail": PROJECT_EMAIL, "superuserPassword": PROJECT_PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["data"]["name"].is_object(), "{res}");

    // routing
    let (status, health) = request(&app, "GET", "/p/blog/api/health", &[], None).await;
    assert_eq!(status, StatusCode::OK, "{health}");
    let (status, headers, _) = raw_request(&app, "GET", "/p/blog?x=1", &[], vec![]).await;
    assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
    assert_eq!(headers["location"], "/p/blog/?x=1");
    let (status, headers, _) = raw_request(&app, "GET", "/p/blog/", &[], vec![]).await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(headers["location"], "./_/");
    let (status, _, page) = raw_request(&app, "GET", "/p/blog/_/", &[], vec![]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&page).contains("slonbase"));
    let (status, _) = request(&app, "GET", "/p/nope/api/health", &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // projects don't host projects
    let (status, _) = request(&app, "GET", "/p/blog/p/blog/api/health", &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // separate superusers: the main installation's token means nothing in the project
    let (status, _) = request(&app, "GET", "/p/blog/api/collections", &admin, None).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "{status}"
    );
    let (status, _) = sign_in(&app, "/p/blog", ADMIN_EMAIL, ADMIN_PASSWORD).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, owner) = sign_in(&app, "/p/blog", PROJECT_EMAIL, PROJECT_PASSWORD).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = sign_in(&app, "", PROJECT_EMAIL, PROJECT_PASSWORD).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "project superusers can't sign in to the main installation"
    );
    let owner_bearer = format!("Bearer {owner}");
    let owner_auth = [("authorization", owner_bearer.as_str())];
    let (status, _) = request(&app, "GET", "/api/projects", &owner_auth, None).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "{status}"
    );

    // separate data
    let (status, res) = request(
        &app,
        "POST",
        "/p/blog/api/collections",
        &owner_auth,
        Some(json!({
            "name": "posts", "listRule": "", "createRule": "",
            "fields": [{ "name": "title", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, post) = request(
        &app,
        "POST",
        "/p/blog/api/collections/posts/records",
        &[],
        Some(json!({ "title": "Hello from the blog" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{post}");
    let (status, _) = request(&app, "GET", "/api/collections/posts", &admin, None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the main installation has no posts"
    );
    let (_, settings) = request(&app, "GET", "/p/blog/api/settings", &owner_auth, None).await;
    assert_eq!(settings["meta"]["appName"], "blog");
    assert!(
        settings["meta"]["appURL"]
            .as_str()
            .unwrap()
            .ends_with("/p/blog"),
        "{settings}"
    );

    // another instance on the same database serves the project too
    let (_dir2, config2) = temp_config();
    let app2 = new_app(&config2, Some(tdb.url.clone()));
    app2.bootstrap().await.unwrap();
    assert!(
        eventually(async || {
            request(
                &app2,
                "GET",
                "/p/blog/api/collections/posts/records",
                &[],
                None,
            )
            .await
            .1["totalItems"]
                == 1
        })
        .await,
        "the second instance loaded the project"
    );

    // deleting needs the name repeated, then stops the project everywhere
    let (status, _) = request(&app, "DELETE", "/api/projects/blog", &admin, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, deleted) = request(
        &app,
        "DELETE",
        "/api/projects/blog?confirm=blog",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    // backed up first, kept with the project's files
    let backup = std::path::PathBuf::from(deleted["backup"].as_str().unwrap());
    assert!(backup.is_file(), "{deleted}");
    assert!(
        backup.starts_with(deleted["files"].as_str().unwrap()),
        "{deleted}"
    );
    assert!(
        backup.to_string_lossy().contains("blog.deleted-"),
        "{deleted}"
    );
    let (status, _) = request(&app, "GET", "/p/blog/api/health", &[], None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !database_exists(&tdb, &database).await,
        "the database was dropped"
    );
    assert!(
        eventually(
            async || request(&app2, "GET", "/p/blog/api/health", &[], None)
                .await
                .0
                == StatusCode::NOT_FOUND
        )
        .await,
        "the second instance stopped the project"
    );
    let (_, list) = request(&app, "GET", "/api/projects", &admin, None).await;
    assert_eq!(list["items"], json!([]));

    // without a backup
    let (status, res) = request(
        &app,
        "POST",
        "/api/projects",
        &admin,
        Some(json!({
            "name": "scratch", "superuserEmail": PROJECT_EMAIL, "superuserPassword": PROJECT_PASSWORD,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{res}");
    let (status, deleted) = request(
        &app,
        "DELETE",
        "/api/projects/scratch?confirm=scratch&backup=false",
        &admin,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert!(deleted["backup"].is_null(), "{deleted}");
    assert!(!database_exists(&tdb, &format!("{}_scratch", tdb.name)).await);

    drop(app2);
    drop(app);
    tdb.drop_db().await;
}

/// The `project` commands, and `--project` for the other commands, through the binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_commands() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_slonbase"))
            .args(args)
            .arg("--config")
            .arg(dir.path().join("slonbase.toml"))
            .arg("--projects-path")
            .arg(dir.path().join("projects"))
            .env("SLONBASE_DATABASE_URL", &tdb.url)
            .env("RUST_LOG", "error")
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), text)
    };

    let (ok, out) = run(&[
        "setup",
        "--email",
        ADMIN_EMAIL,
        "--password",
        ADMIN_PASSWORD,
    ]);
    assert!(ok, "{out}");
    let (ok, out) = run(&["project", "list"]);
    assert!(ok && out.contains("No projects yet"), "{out}");
    let (ok, out) = run(&[
        "project",
        "create",
        "Bad Name",
        "--email",
        PROJECT_EMAIL,
        "--password",
        PROJECT_PASSWORD,
    ]);
    assert!(!ok && out.contains("name:"), "{out}");
    let (ok, out) = run(&[
        "project",
        "create",
        "shop",
        "--email",
        PROJECT_EMAIL,
        "--password",
        PROJECT_PASSWORD,
    ]);
    assert!(ok && out.contains("Created project shop"), "{out}");
    let (ok, out) = run(&["project", "list"]);
    assert!(
        ok && out.contains("/p/shop/") && out.contains(&format!("{}_shop", tdb.name)),
        "{out}"
    );

    // the other commands work on a project with --project
    let (ok, out) = run(&[
        "superuser",
        "upsert",
        "second@example.com",
        "second-password-1",
        "--project",
        "shop",
    ]);
    assert!(ok, "{out}");
    let (ok, out) = run(&[
        "superuser",
        "otp",
        "second@example.com",
        "--project",
        "nope",
    ]);
    assert!(!ok && out.contains("no project named"), "{out}");

    // a server picks the project up, and the new superuser signs in to it, not to the main one
    let (_dir2, config) = temp_config();
    let app = new_app(&config, Some(tdb.url.clone()));
    app.bootstrap().await.unwrap();
    let mut signed_in = false;
    for _ in 0..150 {
        let (status, _) = request(
            &app,
            "POST",
            "/p/shop/api/collections/_superusers/auth-with-password",
            &[],
            Some(json!({ "identity": "second@example.com", "password": "second-password-1" })),
        )
        .await;
        if status == StatusCode::OK {
            signed_in = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(signed_in);
    let (status, _) = request(
        &app,
        "POST",
        "/api/collections/_superusers/auth-with-password",
        &[],
        Some(json!({ "identity": "second@example.com", "password": "second-password-1" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (ok, out) = run(&["project", "delete", "shop", "--confirm", "shp"]);
    assert!(!ok && out.contains("didn't match"), "{out}");
    let (ok, out) = run(&["project", "delete", "shop", "--confirm", "shop"]);
    assert!(
        ok && out.contains("Deleted project shop") && out.contains("Backup: "),
        "{out}"
    );
    assert!(!database_exists(&tdb, &format!("{}_shop", tdb.name)).await);
    assert!(
        eventually(
            async || request(&app, "GET", "/p/shop/api/health", &[], None)
                .await
                .0
                == StatusCode::NOT_FOUND
        )
        .await,
        "the running server stopped the project"
    );

    drop(app);
    tdb.drop_db().await;
}
