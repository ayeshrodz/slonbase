//! Branding assets (`/api/branding/assets`): logos, icons and images an installation uses on
//! its dashboard, its sign-in page and in its emails. Each installation (the main one and
//! every project) keeps its own in `_slonbase.assets`, so they go with its backups.
//!
//! Superusers upload, rename and delete them; anyone may read one, since sign-in pages and
//! emails show them to people who aren't signed in. An asset's bytes never change (a new
//! image is a new asset), so they're cached for good.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use deadpool_postgres::GenericClient;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::auth::RequestAuth;
use crate::error::ApiError;
use crate::extract::JsonObject;
use crate::settings::Settings;
use crate::validation::Errors;

/// The largest asset.
pub const MAX_SIZE: usize = 2 * 1024 * 1024;
/// The most assets an installation keeps.
const MAX_ASSETS: i64 = 100;
const ID_LENGTH: usize = 15;

/// An asset id: 15 characters of `[a-z0-9]`.
pub fn valid_id(id: &str) -> bool {
    id.len() == ID_LENGTH
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The image type of `data`, from its first bytes (what the upload says it is doesn't
/// count), or why it isn't accepted.
pub fn sniff(data: &[u8]) -> Result<&'static str, &'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Ok("image/png");
    }
    if data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Ok("image/jpeg");
    }
    if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Ok("image/webp");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Ok("image/gif");
    }
    if data.starts_with(&[0, 0, 1, 0]) {
        return Ok("image/x-icon");
    }
    let text = std::str::from_utf8(data)
        .map_err(|_| "Must be a PNG, JPEG, WebP, GIF, ICO or SVG image.")?;
    if !looks_like_svg(text) {
        return Err("Must be a PNG, JPEG, WebP, GIF, ICO or SVG image.");
    }
    check_svg(text)?;
    Ok("image/svg+xml")
}

fn looks_like_svg(text: &str) -> bool {
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    // an XML declaration and comments may come first
    loop {
        if let Some(r) = rest.strip_prefix("<?xml") {
            match r.find("?>") {
                Some(end) => rest = r[end + 2..].trim_start(),
                None => return false,
            }
        } else if let Some(r) = rest.strip_prefix("<!--") {
            match r.find("-->") {
                Some(end) => rest = r[end + 3..].trim_start(),
                None => return false,
            }
        } else {
            break;
        }
    }
    rest.starts_with("<svg")
}

/// SVG is a document that can run scripts and load other documents. One that could isn't
/// accepted (rather than cleaned, which is easy to get wrong): no scripts, event handlers,
/// embedded documents, DTDs or entities, and no links outside the file but `#fragments` and
/// `data:image/` URLs.
pub fn check_svg(text: &str) -> Result<(), &'static str> {
    let lower = text.to_ascii_lowercase();
    const REFUSED: &[&str] = &[
        "<script",
        "<foreignobject",
        "<iframe",
        "<embed",
        "<object",
        "<!doctype",
        "<!entity",
        "<?xml-stylesheet",
        "javascript:",
        "vbscript:",
        "@import",
        "<handler",
        "<listener",
        "<set ",
        "<animate",
    ];
    if REFUSED.iter().any(|p| lower.contains(p)) {
        return Err("SVG images can't have scripts, animations, embedded documents or DTDs.");
    }
    // event handler attributes (onload=, onclick= ...) anywhere in a tag
    let bytes = lower.as_bytes();
    for (i, w) in bytes.windows(3).enumerate() {
        if w[1..] == *b"on"
            && (w[0].is_ascii_whitespace() || w[0] == b'/' || w[0] == b'"' || w[0] == b'\'')
        {
            let name: String = lower[i + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let after = lower[i + 3 + name.len()..].trim_start();
            if !name.is_empty() && after.starts_with('=') {
                return Err("SVG images can't have event handlers (on… attributes).");
            }
        }
    }
    // href, xlink:href, src and url(...) may only point inside the file or to data: images
    for (pos, _) in lower
        .match_indices("href")
        .chain(lower.match_indices("src"))
    {
        // an attribute: after a space, or a namespace (xlink:href)
        if pos == 0 || !matches!(bytes[pos - 1], b' ' | b'\t' | b'\n' | b'\r' | b':') {
            continue;
        }
        let after = lower[pos..]
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .trim_start();
        if let Some(value) = after.strip_prefix('=') {
            let value = value
                .trim_start()
                .trim_start_matches(['"', '\''])
                .trim_start();
            if !(value.starts_with('#') || value.starts_with("data:image/")) {
                return Err(
                    "SVG images can only link inside the file (#id) or to data:image/ URLs.",
                );
            }
        }
    }
    for (pos, _) in lower.match_indices("url(") {
        let value = lower[pos + 4..]
            .trim_start()
            .trim_start_matches(['"', '\'']);
        if !(value.starts_with('#') || value.starts_with("data:image/")) {
            return Err("SVG images can only link inside the file (#id) or to data:image/ URLs.");
        }
    }
    Ok(())
}

/// A name for an asset: what the upload was called, without directories or odd characters.
fn clean_name(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = base
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(80)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        "image".to_string()
    } else {
        cleaned
    }
}

/// The URL an asset is served at, under `base` (an installation's path).
pub fn url(base: &str, id: &str) -> String {
    format!("{base}/api/branding/assets/{id}")
}

fn item(row: &tokio_postgres::Row, settings: &Settings) -> Value {
    let id: String = row.get("id");
    let used: Vec<&str> = settings
        .asset_uses()
        .into_iter()
        .filter(|(asset, _)| *asset == id)
        .map(|(_, use_)| use_)
        .collect();
    json!({
        "id": id,
        "name": row.get::<_, String>("name"),
        "mime": row.get::<_, String>("mime"),
        "size": row.get::<_, i32>("size"),
        "created": crate::datetime::format(&row.get("created")),
        "usedBy": used,
    })
}

async fn client(
    app: &App,
) -> Result<(Arc<crate::core::Core>, deadpool_postgres::Object), ApiError> {
    let core = app.core()?;
    let client = core
        .pool
        .get()
        .await
        .map_err(|e| ApiError::internal(crate::db::describe_pool_error(e)))?;
    Ok((core, client))
}

/// `GET /api/branding/assets`: every asset, newest first, with where each is used.
pub async fn list(State(app): State<Arc<App>>, auth: RequestAuth) -> Result<Json<Value>, ApiError> {
    auth.require_superuser()?;
    let (core, client) = client(&app).await?;
    let settings = core.settings();
    let rows = client
        .query(
            "SELECT id, name, mime, size, created FROM _slonbase.assets ORDER BY created DESC, id",
            &[],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    Ok(Json(Value::Array(
        rows.iter().map(|r| item(r, &settings)).collect(),
    )))
}

fn invalid(key: &str, message: &str) -> ApiError {
    let mut errors = Errors::new();
    errors.invalid(key, message);
    ApiError::invalid_data(
        "An error occurred while validating the submitted data.",
        &errors,
    )
}

/// `POST /api/branding/assets`: uploads an image (multipart `file`, at most 2 MB).
pub async fn upload(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    mut multipart: Multipart,
) -> Result<Json<Value>, ApiError> {
    auth.require_superuser()?;
    let mut upload: Option<(String, Vec<u8>)> = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(format!("Invalid upload: {e}.")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let name = clean_name(field.file_name().unwrap_or("image"));
        let mut data = Vec::new();
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(format!("Invalid upload: {e}.")))?
        {
            if data.len() + chunk.len() > MAX_SIZE {
                return Err(invalid("file", "Must be at most 2 MB."));
            }
            data.extend_from_slice(&chunk);
        }
        upload = Some((name, data));
        break;
    }
    let Some((name, data)) = upload else {
        return Err(invalid("file", "Choose an image."));
    };
    if data.is_empty() {
        return Err(invalid("file", "The file is empty."));
    }
    let mime = sniff(&data).map_err(|message| invalid("file", message))?;
    let (core, client) = client(&app).await?;
    let count: i64 = client
        .query_one("SELECT count(*) FROM _slonbase.assets", &[])
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .get(0);
    if count >= MAX_ASSETS {
        return Err(invalid(
            "file",
            "An installation keeps at most 100 assets; delete some first.",
        ));
    }
    let id = crate::ids::random_id(ID_LENGTH);
    let sha = hex::encode(Sha256::digest(&data));
    let row = client
        .query_one(
            "INSERT INTO _slonbase.assets (id, name, mime, size, sha256, data)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id, name, mime, size, created",
            &[&id, &name, &mime, &(data.len() as i32), &sha, &data],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    tracing::info!(asset = %id, name = %name, size = data.len(), "uploaded a branding asset");
    Ok(Json(item(&row, &core.settings())))
}

/// `PATCH /api/branding/assets/{id}`: renames an asset (`{"name": "..."}`).
pub async fn rename(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(id): Path<String>,
    JsonObject(body): JsonObject,
) -> Result<Json<Value>, ApiError> {
    auth.require_superuser()?;
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map(clean_name)
        .ok_or_else(|| invalid("name", "Cannot be blank."))?;
    let (core, client) = client(&app).await?;
    let row = client
        .query_opt(
            "UPDATE _slonbase.assets SET name = $2 WHERE id = $1
             RETURNING id, name, mime, size, created",
            &[&id, &name],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(item(&row, &core.settings())))
}

/// `DELETE /api/branding/assets/{id}`: deletes an asset the settings don't use.
pub async fn delete(
    State(app): State<Arc<App>>,
    auth: RequestAuth,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    auth.require_superuser()?;
    let (core, client) = client(&app).await?;
    let settings = core.settings();
    let uses: Vec<&str> = settings
        .asset_uses()
        .into_iter()
        .filter(|(asset, _)| *asset == id)
        .map(|(_, use_)| use_)
        .collect();
    if !uses.is_empty() {
        return Err(ApiError::bad_request(format!(
            "The image is {}; choose another first.",
            uses.join(" and ")
        )));
    }
    let deleted = client
        .execute("DELETE FROM _slonbase.assets WHERE id = $1", &[&id])
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    if deleted == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/branding/assets/{id}`: the image, for anyone.
pub async fn serve(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if !valid_id(&id) {
        return Err(ApiError::not_found());
    }
    let (_, client) = client(&app).await?;
    let etag_of = |sha: &str| format!("\"{}\"", &sha[..32.min(sha.len())]);
    // a cached copy is still good: its bytes can't have changed
    if let Some(tag) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        let row = client
            .query_opt("SELECT sha256 FROM _slonbase.assets WHERE id = $1", &[&id])
            .await
            .map_err(|e| ApiError::internal(e.into()))?
            .ok_or_else(ApiError::not_found)?;
        let etag = etag_of(row.get(0));
        if tag.split(',').any(|t| t.trim() == etag) {
            let mut response = StatusCode::NOT_MODIFIED.into_response();
            response.headers_mut().insert(
                header::ETAG,
                HeaderValue::from_str(&etag).expect("hex etag"),
            );
            return Ok(response);
        }
    }
    let row = client
        .query_opt(
            "SELECT name, mime, sha256, data FROM _slonbase.assets WHERE id = $1",
            &[&id],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .ok_or_else(ApiError::not_found)?;
    let mime: String = row.get("mime");
    let data: Vec<u8> = row.get("data");
    let mut response = data.into_response();
    let h = response.headers_mut();
    let value = |v: &str| HeaderValue::from_str(v).unwrap_or(HeaderValue::from_static(""));
    h.insert(header::CONTENT_TYPE, value(&mime));
    h.insert(header::ETAG, value(&etag_of(row.get("sha256"))));
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    // opened on its own, an image (an SVG above all) can't run anything or load anything
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; img-src data:; style-src 'unsafe-inline'; sandbox",
        ),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        value(&format!(
            "inline; filename=\"{}\"",
            row.get::<_, String>("name").replace('"', "")
        )),
    );
    Ok(response)
}

/// Fails when `settings` use an asset that doesn't exist.
pub(crate) async fn check_uses(
    client: &impl GenericClient,
    settings: &Settings,
) -> Result<(), ApiError> {
    let fields = [
        ("meta.logo", &settings.meta.logo),
        ("meta.logoDark", &settings.meta.logo_dark),
        ("meta.favicon", &settings.meta.favicon),
        ("login.logo", &settings.login.logo),
        ("login.backgroundAsset", &settings.login.background_asset),
    ];
    let ids: Vec<&str> = fields
        .iter()
        .map(|(_, id)| id.as_str())
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let found: Vec<String> = client
        .query(
            "SELECT id FROM _slonbase.assets WHERE id = ANY($1)",
            &[&ids],
        )
        .await
        .map_err(|e| ApiError::internal(e.into()))?
        .iter()
        .map(|r| r.get(0))
        .collect();
    let mut errors = Errors::new();
    for (key, id) in fields {
        if !id.is_empty() && !found.iter().any(|f| f == id) {
            errors.invalid(key, "There's no such image.");
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(ApiError::invalid_data(
            "An error occurred while validating the submitted data.",
            &errors,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_images() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Ok("image/png"));
        assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Ok("image/jpeg"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Ok("image/webp"));
        assert_eq!(sniff(b"GIF89a..."), Ok("image/gif"));
        assert_eq!(
            sniff(br#"<?xml version="1.0"?><!-- logo --><svg xmlns="http://www.w3.org/2000/svg"><circle r="4" fill="url(#g)"/></svg>"#),
            Ok("image/svg+xml")
        );
        assert!(sniff(b"<html><script>alert(1)</script></html>").is_err());
        assert!(sniff(b"\0\0\0binary").is_err());
    }

    #[test]
    fn refuses_active_svg() {
        for svg in [
            r#"<svg><script>alert(1)</script></svg>"#,
            r#"<svg onload="alert(1)"></svg>"#,
            r#"<svg><rect ONCLICK = "x()"/></svg>"#,
            r#"<svg><foreignObject><iframe src="x"/></foreignObject></svg>"#,
            r#"<svg><a href="javascript:alert(1)"><text>x</text></a></svg>"#,
            r#"<svg><image href="https://evil.example/track.png"/></svg>"#,
            r#"<svg><use xlink:href="other.svg#a"/></svg>"#,
            r#"<svg><rect style="fill:url(https://evil.example/x)"/></svg>"#,
            r#"<svg><style>@import url(x.css);</style></svg>"#,
            r#"<svg><set attributeName="href" to="javascript:x"/></svg>"#,
        ] {
            assert!(sniff(svg.as_bytes()).is_err(), "{svg}");
        }
        for svg in [
            r##"<svg viewBox="0 0 10 10"><defs><linearGradient id="g"/></defs><rect fill="url(#g)" font="bonjour"/><use href="#g"/></svg>"##,
            r#"<svg><image href="data:image/png;base64,AAAA"/><text>contact on time</text></svg>"#,
        ] {
            assert_eq!(sniff(svg.as_bytes()), Ok("image/svg+xml"), "{svg}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(clean_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_name("C:\\logo <b>.png"), "logo b.png");
        assert_eq!(clean_name("<>"), "image");
        assert!(valid_id("abcdefghij12345"));
        assert!(!valid_id("abc"));
        assert!(!valid_id("ABCDEFGHIJ12345"));
    }
}
