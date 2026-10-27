//! `strftime()` in filters. `strftime_cases.tsv` holds results of SQLite's strftime (as bundled
//! with PocketBase 0.40), one case per line: format, time value, modifiers, expected (`NULL`).
//! See `setup_flow.rs` for how to point the tests at a database.

mod common;

use common::*;

#[tokio::test]
async fn matches_sqlite() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let pool = tdb.pool();
    slonbase::migrations::run(&pool).await.unwrap();
    let client = pool.get().await.unwrap();
    let statement = client
        .prepare("SELECT coalesce(_slonbase.strftime($1, $2, $3, 1704067200000), 'NULL')")
        .await
        .unwrap();
    let mut failures = Vec::new();
    let cases = include_str!("strftime_cases.tsv");
    for line in cases.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        let (expected, args) = parts.split_last().unwrap();
        let modifiers: Vec<String> = args[2..].iter().map(|m| m.to_string()).collect();
        let row = client
            .query_one(&statement, &[&args[0], &args[1], &modifiers])
            .await
            .unwrap();
        let got: String = row.get(0);
        if got != *expected {
            failures.push(format!("{args:?}: SQLite {expected:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(cases.lines().count() > 200);

    // 'now' is the request's time (2024-01-01 00:00:00 UTC here)
    for (args, expected) in [
        (vec!["%Y-%m-%d %H:%M:%f", "now"], "2024-01-01 00:00:00.000"),
        (vec!["%s", "NOW", "+1 day"], "1704153600"),
        (vec!["%s", "subsec"], "1704067200.000"),
    ] {
        let modifiers: Vec<String> = args[2..].iter().map(|m| m.to_string()).collect();
        let row = client
            .query_one(&statement, &[&args[0], &args[1], &modifiers])
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), expected, "{args:?}");
    }
    drop(client);
    drop(pool);
    tdb.drop_db().await;
}

/// Date fields compile to `to_char()` for the common formats and modifiers: the same cases
/// (`strftime_date_cases.tsv`: value, format, modifiers, SQLite's result) through a date field
/// and through a text field (the full implementation) must both match SQLite.
#[tokio::test]
async fn date_fields_match_sqlite() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let headers = [("authorization", auth.as_str())];
    let (status, res) = request(
        &app,
        "POST",
        "/api/collections",
        &headers,
        Some(serde_json::json!({
            "name": "times",
            "fields": [{ "name": "at", "type": "date" }, { "name": "raw", "type": "text" }],
        })),
    )
    .await;
    assert_eq!(status, 200, "{res}");

    let quote = |s: &str| format!("'{}'", s.replace('\'', "\\'"));
    let mut failures = Vec::new();
    let cases = include_str!("strftime_date_cases.tsv");
    for line in cases.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        let (expected, args) = parts.split_last().unwrap();
        let (value, rest) = args.split_first().unwrap();
        let (status, record) = request(
            &app,
            "POST",
            "/api/collections/times/records",
            &headers,
            Some(serde_json::json!({ "at": value, "raw": value })),
        )
        .await;
        assert_eq!(status, 200, "{record}");
        let id = record["id"].as_str().unwrap();
        for field in ["at", "raw"] {
            let mut call = vec![quote(rest[0]), field.to_string()];
            call.extend(rest[1..].iter().map(|m| quote(m)));
            let filter = format!(
                "id = '{id}' && strftime({}) = {}",
                call.join(", "),
                quote(expected)
            );
            let q: String = form_urlencoded::byte_serialize(filter.as_bytes()).collect();
            let (status, list) = request(
                &app,
                "GET",
                &format!("/api/collections/times/records?filter={q}"),
                &headers,
                None,
            )
            .await;
            if status != 200 || list["totalItems"] != 1 {
                failures.push(format!("{field} {line:?}: {status} {list}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    drop(app);
    tdb.drop_db().await;
}
