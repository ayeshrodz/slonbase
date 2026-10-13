//! slonbase: a lightweight PocketBase-style backend powered by PostgreSQL.
//!
//! The `slonbase` binary is [`cli::run`] with no extensions. To add hooks, routes and cron
//! jobs, depend on this crate and run the command line from your own `main`:
//!
//! ```no_run
//! use axum::routing::get;
//!
//! #[tokio::main]
//! async fn main() -> std::process::ExitCode {
//!     let mut ext = slonbase::Extensions::default();
//!     ext.hooks.on_record_create_request.add(&["posts"], |mut e, next| async move {
//!         e.record.insert("status".into(), "draft".into());
//!         next.run(e).await
//!     });
//!     ext.route("/hello", get(|| async { "Hello!" }));
//!     slonbase::cli::run(ext).await
//! }
//! ```

use std::sync::Arc;

pub mod app;
pub mod auth;
pub mod backups;
pub mod cli;
pub mod collections;
pub mod config;
pub mod core;
pub mod cron;
pub mod datetime;
pub mod db;
pub mod error;
pub mod extract;
pub mod filter;
pub mod guard;
pub mod hooks;
pub mod http;
pub mod ids;
pub mod ip;
pub mod logs;
pub mod mail;
pub mod migrations;
pub mod oauth2;
pub mod realtime;
pub mod records;
pub mod s3;
pub mod server;
pub mod settings;
pub mod setup;
pub mod sql_console;
pub mod storage;
pub mod superusers;
pub mod ui;
pub mod validation;

/// Hooks, routes and cron jobs added to slonbase (see the crate docs).
pub struct Extensions {
    pub hooks: hooks::Hooks,
    pub(crate) routes: axum::Router<Arc<app::App>>,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            hooks: hooks::Hooks::default(),
            routes: axum::Router::new(),
        }
    }
}

impl Extensions {
    /// Adds a route. Handlers can take `State<Arc<App>>` (with [`app::App::core`] for the
    /// database, collections and settings) and [`auth::RequestAuth`] for the caller.
    pub fn route(&mut self, path: &str, route: axum::routing::MethodRouter<Arc<app::App>>) {
        self.routes = std::mem::take(&mut self.routes).route(path, route);
    }

    /// Merges a router.
    pub fn merge(&mut self, router: axum::Router<Arc<app::App>>) {
        self.routes = std::mem::take(&mut self.routes).merge(router);
    }
}
