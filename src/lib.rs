//! slonbase: a lightweight PocketBase-style backend powered by PostgreSQL.

pub mod app;
pub mod auth;
pub mod collections;
pub mod config;
pub mod core;
pub mod datetime;
pub mod db;
pub mod error;
pub mod extract;
pub mod filter;
pub mod ids;
pub mod mail;
pub mod migrations;
pub mod realtime;
pub mod records;
pub mod server;
pub mod settings;
pub mod setup;
pub mod storage;
pub mod superusers;
pub mod validation;
