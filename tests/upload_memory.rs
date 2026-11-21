//! Memory stays bounded while an upload over a file's size cap is refused, alone and many at
//! once: the part is read up to the cap plus one byte and the rest is drained, not kept.
//! The test binary counts allocated bytes (not process memory, which varies from run to run),
//! so the bound can be asserted. See `setup_flow.rs` for how to point the tests at a database.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use common::*;
use slonbase::server;

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(by: usize) {
    let now = CURRENT.fetch_add(by, Ordering::SeqCst) + by;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        CURRENT.fetch_sub(layout.size(), Ordering::SeqCst);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            if new_size >= layout.size() {
                grow(new_size - layout.size());
            } else {
                CURRENT.fetch_sub(layout.size() - new_size, Ordering::SeqCst);
            }
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const BOUNDARY: &str = "----slonbase-memory-boundary";
const CHUNK: usize = 64 * 1024;
/// The file field's cap, and how much the uploads send: far more than the cap, less than the
/// request limit.
const CAP: usize = 1024 * 1024;
const SENT: usize = 48 * 1024 * 1024;

/// A multipart body of one file part of `size` bytes, produced chunk by chunk so that the test
/// does not hold it.
fn streamed_upload(token: &str, size: usize) -> Request<Body> {
    let head = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"blob\"; filename=\"big.bin\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    );
    let tail = format!("\r\n--{BOUNDARY}--\r\n");
    let parts = futures_util::stream::unfold((0usize, 0u8), move |(sent, step)| {
        let head = head.clone();
        let tail = tail.clone();
        async move {
            // a body that arrives over a network is never all there at once: without this, the
            // multipart reader would take every chunk the stream has ready, which is all of them
            tokio::task::yield_now().await;
            match step {
                0 => Some((Bytes::from(head), (0, 1))),
                1 if sent < size => {
                    let n = CHUNK.min(size - sent);
                    Some((Bytes::from(vec![b'x'; n]), (sent + n, 1)))
                }
                1 => Some((Bytes::from(tail), (sent, 2))),
                _ => None,
            }
        }
    });
    Request::builder()
        .method("POST")
        .uri("/api/collections/docs/records")
        .header("authorization", format!("Bearer {token}"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from_stream(futures_util::StreamExt::map(
            parts,
            Ok::<_, std::convert::Infallible>,
        )))
        .unwrap()
}

#[tokio::test]
async fn an_upload_over_the_cap_is_refused_without_being_kept() {
    let Some(tdb) = TestDb::create().await else {
        return;
    };
    let (app, token, _dir) = ready_app(&tdb).await;
    let auth = format!("Bearer {token}");
    let (status, body) = request(
        &app,
        "POST",
        "/api/collections",
        &[("authorization", auth.as_str())],
        Some(json!({
            "name": "docs",
            "fields": [{ "name": "blob", "type": "file", "maxSize": CAP }],
            "createRule": "",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let send = |app: &std::sync::Arc<slonbase::app::App>, token: &str| {
        let req = streamed_upload(token, SENT);
        let router = server::router(app.clone());
        async move {
            let res = router.oneshot(req).await.unwrap();
            let status = res.status();
            let _ = res.into_body().collect().await;
            status
        }
    };

    // one upload: far more is sent than the cap, and about the cap is ever held
    let baseline = CURRENT.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let status = send(&app, &token).await;
    assert!(status.is_client_error(), "refused, got {status}");
    eprintln!(
        "MEM one={}",
        PEAK.load(Ordering::SeqCst).saturating_sub(baseline)
    );
    let one = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    assert!(
        one < 8 * 1024 * 1024,
        "one refused upload of {SENT} bytes held {one} bytes at its peak"
    );

    // many at once stay within a small multiple of the cap, not of what was sent
    let baseline = CURRENT.load(Ordering::SeqCst);
    PEAK.store(baseline, Ordering::SeqCst);
    let statuses = futures_util::future::join_all((0..8).map(|_| send(&app, &token))).await;
    assert!(statuses.iter().all(|s| s.is_client_error()), "{statuses:?}");
    let many = PEAK.load(Ordering::SeqCst).saturating_sub(baseline);
    assert!(
        many < 24 * 1024 * 1024,
        "eight refused uploads of {SENT} bytes each held {many} bytes at their peak"
    );
    tdb.drop_db().await;
}
