//! Whose permissions apply to an operation.
//!
//! A service that can read or change anything (SQL) asks for an [`Access`], so the caller has
//! to say on whose behalf it acts instead of getting the server's reach by default.

use crate::auth::RequestAuth;
use crate::core::Core;
use crate::error::ApiError;
use crate::roles::Role;

/// On whose behalf an operation runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Access {
    /// The server itself: a scheduled job that someone with the right role set up.
    System,
    /// A signed-in superuser with this role.
    Role(Role),
}

impl Access {
    /// Whether this access is at least `min` (the server itself always is).
    pub fn allows(self, min: Role) -> bool {
        match self {
            Access::System => true,
            Access::Role(role) => role >= min,
        }
    }
}

/// The signed-in superuser's role (an error when the request isn't a superuser's).
pub fn role(core: &Core, auth: &RequestAuth) -> Result<Role, ApiError> {
    crate::roles::require_role(core, auth, Role::Viewer).map(|(_, role)| role)
}

/// Requires the signed-in superuser to have at least `min`; `message` is what they are told
/// otherwise.
pub fn require(
    core: &Core,
    auth: &RequestAuth,
    min: Role,
    message: &str,
) -> Result<Role, ApiError> {
    crate::roles::require_role(core, auth, min)
        .map(|(_, role)| role)
        .map_err(|_| ApiError::forbidden(message.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_may_do_anything_and_a_role_what_it_reaches() {
        assert!(Access::System.allows(Role::Owner));
        assert!(Access::Role(Role::Owner).allows(Role::Owner));
        assert!(Access::Role(Role::Admin).allows(Role::Developer));
        assert!(!Access::Role(Role::Admin).allows(Role::Owner));
        assert!(!Access::Role(Role::Viewer).allows(Role::Developer));
    }
}
