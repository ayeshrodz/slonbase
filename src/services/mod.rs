//! Services: the things features are built from, each behind one interface.
//!
//! A feature (cron jobs today) sends mail, calls URLs, runs SQL and asks who may do what through
//! these modules, never through another feature's code. That keeps the rules of each in one
//! place: which addresses can be called, how many people an email can go to, what the SQL
//! console's policy allows, which role may do something.
//!
//! - [`access`]: whose permissions apply to an operation.
//! - [`outbound`]: HTTP requests to addresses a user typed in.
//! - [`mail`]: email to addresses a user typed in.
//! - [`sql`]: statements under the SQL console's policy.
//!
//! Records are read and written through the record service on [`crate::core::Core`]
//! (`find_records_by_filter`, `count_records`, `delete_record`, ...), which already is one.

pub mod access;
pub mod mail;
pub mod outbound;
pub mod sql;

/// Why an operation of a service failed, and whether trying again could help: a missing
/// setting or a refused address won't fix itself, a timeout or a busy database might.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub message: String,
    pub retry: bool,
}

impl Failure {
    /// A failure that may pass: worth another try.
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: true,
        }
    }

    /// A failure that trying again can't change.
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: false,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
