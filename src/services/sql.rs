//! Statements under the SQL console's policy.
//!
//! Whatever runs SQL for a user (the console, a job) goes through here, so none of it can do
//! what the console can't: the console must be on, a read-only console runs reads only, the
//! statement runs as the console's role with its time limits, and only an owner (or the server,
//! for something an owner set up) may run one.

use serde_json::Value;

use crate::core::Core;
use crate::roles::Role;

use super::Failure;
use super::access::Access;

/// The longest statement, in characters.
pub const MAX_STATEMENT_LENGTH: usize = crate::sql_console::MAX_QUERY_LENGTH;

/// What a statement did. Not what it read: rows may hold anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub affected_rows: Value,
    pub rows: usize,
    pub read_only: Value,
}

/// Whether `statement` could be run now, for checking it when it is saved: the console is on,
/// the statement has a sensible length, and a read-only console is only asked to read.
pub fn check(core: &Core, statement: &str) -> Result<(), String> {
    let statement = statement.trim();
    if !core.sql_console {
        return Err("SQL jobs need the SQL console: start slonbase with --sql-console.".into());
    }
    if statement.is_empty() || statement.chars().count() > MAX_STATEMENT_LENGTH {
        return Err("1 to 20 000 characters.".into());
    }
    if !core.sql_console_writes
        && !crate::sql_console::READ_KEYWORDS
            .contains(&crate::sql_console::first_keyword(statement).as_str())
    {
        return Err("The SQL console is read-only, so is this job: start slonbase with --sql-console-writes to change data.".into());
    }
    Ok(())
}

/// Runs `statement` and returns what it did (at most `max_rows` rows are read).
pub async fn run(
    core: &Core,
    access: Access,
    statement: &str,
    max_rows: usize,
) -> Result<Outcome, Failure> {
    if !access.allows(Role::Owner) {
        return Err(Failure::permanent("Only owners can run SQL."));
    }
    let result = crate::sql_console::execute(core, statement, max_rows)
        .await
        .map_err(|e| Failure {
            // the console is off, or read-only for this statement: that won't change by itself
            retry: e.status != axum::http::StatusCode::FORBIDDEN,
            message: e.details.as_deref().unwrap_or(&e.message).to_string(),
        })?;
    Ok(Outcome {
        affected_rows: result["affectedRows"].clone(),
        rows: result["rows"].as_array().map_or(0, Vec::len),
        read_only: result["readOnly"].clone(),
    })
}
