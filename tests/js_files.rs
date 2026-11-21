//! Read-only files for hooks (`hooks.filesDir`): `$os.readFile`, `$os.dirFS`, `e.fileFS` and
//! `$filepath`. They are off unless the directory is set, and never leave it. See `setup_flow.rs`
//! for how to point the tests at a database.
#![cfg(feature = "js")]

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{Value, json};

use common::*;
use slonbase::app::App;
use slonbase::config::HooksConfig;

const HOOKS: &str = r#"
const attempt = (e, fn) => {
  try {
    e.json(200, { ok: fn() });
  } catch (err) {
    e.json(200, { error: String(err) });
  }
};
routerAdd("GET", "/api/ft/read", (e) => attempt(e, () => {
  const bytes = $os.readFile(e.request.url.query().get("path"));
  return { length: bytes.length, text: toString(bytes) };
}));
routerAdd("GET", "/api/ft/walk", (e) => attempt(e, () => {
  const seen = [];
  $filepath.walkDir(".", (path, d, err) => { if (!d.isDir()) seen.push(path); });
  return seen;
}));
routerAdd("GET", "/api/ft/glob", (e) => attempt(e, () => $filepath.glob(e.request.url.query().get("pattern"))));
routerAdd("GET", "/api/ft/paths", (e) => attempt(e, () => ({
  join: $filepath.join("a", "b/../c", "d.txt"),
  dir: $filepath.dir("a/b/c.txt"),
  base: $filepath.base("a/b/c.txt"),
  ext: $filepath.ext("a/b/c.tar.gz"),
})));
routerAdd("GET", "/api/ft/serve/{name}", (e) => {
  e.fileFS($os.dirFS("public"), e.request.pathValue("name"));
});
"#;

async fn app_with(
    tdb: &TestDb,
    files_dir: Option<&std::path::Path>,
) -> (Arc<App>, tempfile::TempDir) {
    let hooks_dir = tempfile::tempdir().unwrap();
    std::fs::write(hooks_dir.path().join("main.pb.js"), HOOKS).unwrap();
    let mut ext = slonbase::Extensions::default();
    slonbase::js::load(
        &HooksConfig {
            dir: hooks_dir.path().to_path_buf(),
            pool_size: 2,
            watch: false,
            files_dir: files_dir.map(|d| d.to_path_buf()),
            ..Default::default()
        },
        &mut ext,
    )
    .await
    .unwrap();
    let (app, _token, dir) = ready_app_with(tdb, ext).await;
    // (the hooks directory must outlive the app)
    std::mem::forget(hooks_dir);
    (app, dir)
}

async fn get(app: &Arc<App>, uri: &str) -> Value {
    let (status, res) = request(app, "GET", uri, &[], None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {res}");
    res
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hooks_read_files_only_inside_the_configured_directory() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let files = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(files.path().join("note.txt"), "hello").unwrap();
    std::fs::create_dir_all(files.path().join("pages/blog")).unwrap();
    std::fs::write(files.path().join("pages/index.html"), "<p>hi</p>").unwrap();
    std::fs::write(files.path().join("pages/about.html"), "<p>about</p>").unwrap();
    std::fs::write(files.path().join("pages/blog/post.html"), "<p>post</p>").unwrap();
    std::fs::create_dir_all(files.path().join("public")).unwrap();
    std::fs::write(
        files.path().join("public/logo.png"),
        [0u8, 255, 128, 10, 200],
    )
    .unwrap();
    std::fs::write(outside.path().join("secret.txt"), "TOP-SECRET-CONTENT").unwrap();
    // a link in a served directory to a file that is inside hooks.files_dir, but not in it
    std::fs::write(files.path().join("private.txt"), "PRIVATE-CONTENT").unwrap();
    std::os::unix::fs::symlink(
        files.path().join("private.txt"),
        files.path().join("public/peek.txt"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.txt"),
        files.path().join("link.txt"),
    )
    .unwrap();

    // off by default: refused, with what to set
    let (app, _dir) = app_with(&tdb, None).await;
    let res = get(&app, "/api/ft/read?path=note.txt").await;
    assert!(
        res["error"].as_str().unwrap().contains("hooks.files_dir"),
        "{res}"
    );
    drop(app);
    tdb.drop_db().await;

    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, _dir) = app_with(&tdb, Some(files.path())).await;
    // read: relative to the directory, or absolute inside it
    let res = get(&app, "/api/ft/read?path=note.txt").await;
    assert_eq!(res["ok"], json!({ "length": 5, "text": "hello" }));
    let absolute = files.path().join("note.txt");
    let res = get(&app, &format!("/api/ft/read?path={}", absolute.display())).await;
    assert_eq!(res["ok"]["text"], "hello");
    // never outside: `..`, an absolute path elsewhere, a link that leads out
    for path in [
        "../outside.txt".to_string(),
        "pages/../../etc/passwd".to_string(),
        "/etc/passwd".to_string(),
        "link.txt".to_string(),
        outside.path().join("secret.txt").display().to_string(),
    ] {
        let res = get(&app, &format!("/api/ft/read?path={path}")).await;
        assert!(res["error"].is_string(), "{path} was read: {res}");
        assert!(!res.to_string().contains("TOP-SECRET"), "{path}: {res}");
    }

    // walking and globbing
    let res = get(&app, "/api/ft/walk").await;
    assert_eq!(
        res["ok"],
        json!([
            "link.txt",
            "note.txt",
            "pages/about.html",
            "pages/blog/post.html",
            "pages/index.html",
            "private.txt",
            "public/logo.png",
            "public/peek.txt"
        ])
    );
    let res = get(&app, "/api/ft/glob?pattern=pages/*.html").await;
    assert_eq!(res["ok"], json!(["pages/about.html", "pages/index.html"]));
    let res = get(&app, "/api/ft/glob?pattern=pages/*/post.html").await;
    assert_eq!(res["ok"], json!(["pages/blog/post.html"]));
    let res = get(&app, "/api/ft/paths").await;
    assert_eq!(
        res["ok"],
        json!({ "join": "a/c/d.txt", "dir": "a/b", "base": "c.txt", "ext": ".gz" })
    );

    // a file served whole, bytes and type as they are; a missing one is a 404
    let (status, headers, bytes) =
        raw_request(&app, "GET", "/api/ft/serve/logo.png", &[], Vec::new()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(bytes, [0u8, 255, 128, 10, 200]);
    let (status, _, bytes) =
        raw_request(&app, "GET", "/api/ft/serve/peek.txt", &[], Vec::new()).await;
    assert_ne!(status, StatusCode::OK, "a link out of the served directory");
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE-CONTENT"));
    let (status, _, _) = raw_request(&app, "GET", "/api/ft/serve/nope.png", &[], Vec::new()).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) =
        raw_request(&app, "GET", "/api/ft/serve/..%2Fnote.txt", &[], Vec::new()).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a name that leaves the directory"
    );
    tdb.drop_db().await;
}
