//! Permission model and role→permission mapping.
//!
//! Authorization is evaluated per-operation in handlers. Roles never
//! imply unrestricted access by themselves — they map to explicit
//! technical permissions. Resource-level organizational scoping is a
//! documented Phase 1 gap (see AUTHENTICATION_AUTHORIZATION.md): until
//! scope data exists, scope-restricted checks deny rather than assume.

use crate::principal::Principal;

/// Technical permissions for agent registry operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Create,
    Read,
    Activate,
    Suspend,
    Reinstate,
    Terminate,
    IdentityLink,
    HistoryRead,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "agent:create",
            Self::Read => "agent:read",
            Self::Activate => "agent:activate",
            Self::Suspend => "agent:suspend",
            Self::Reinstate => "agent:reinstate",
            Self::Terminate => "agent:terminate",
            Self::IdentityLink => "agent:identity:link",
            Self::HistoryRead => "agent:history:read",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "agent:create" => Self::Create,
            "agent:read" => Self::Read,
            "agent:activate" => Self::Activate,
            "agent:suspend" => Self::Suspend,
            "agent:reinstate" => Self::Reinstate,
            "agent:terminate" => Self::Terminate,
            "agent:identity:link" => Self::IdentityLink,
            "agent:history:read" => Self::HistoryRead,
            _ => return None,
        })
    }
}

/// Role → permission mapping (Keycloak realm roles).
/// `PLATFORM_ADMIN` and `AGENT_ADMIN` get the full set; operational
/// roles get the subsets appropriate to their function.
fn role_permissions(role: &str) -> &'static [Permission] {
    use Permission::*;
    match role {
        "PLATFORM_ADMIN" | "AGENT_ADMIN" => &[
            Create,
            Read,
            Activate,
            Suspend,
            Reinstate,
            Terminate,
            IdentityLink,
            HistoryRead,
        ],
        "AGENT_OPERATIONS" => &[
            Read,
            Activate,
            Suspend,
            Reinstate,
            Terminate,
            IdentityLink,
            HistoryRead,
        ],
        "AGENT_LIFECYCLE_OFFICER" => &[Read, Activate, Suspend, Reinstate, Terminate, HistoryRead],
        "AGENT_AUDITOR" => &[Read, HistoryRead],
        _ => &[],
    }
}

/// Effective permission set for a principal: explicit `agent:*` scopes
/// plus permissions derived from roles.
pub fn permissions_for(principal: &Principal) -> Vec<Permission> {
    let mut set: Vec<Permission> = Vec::new();
    for scope in principal.scopes.iter().chain(principal.roles.iter()) {
        if let Some(p) = Permission::parse(scope) {
            if !set.contains(&p) {
                set.push(p);
            }
        }
    }
    for role in &principal.roles {
        for p in role_permissions(role) {
            if !set.contains(p) {
                set.push(*p);
            }
        }
    }
    set
}

/// Whether the principal holds `perm`.
pub fn has_permission(principal: &Principal, perm: Permission) -> bool {
    permissions_for(principal).contains(&perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal(roles: &[&str], scopes: &[&str]) -> Principal {
        Principal {
            subject: "u".into(),
            issuer: "iss".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            agent_scope: None,
        }
    }

    #[test]
    fn admin_gets_all() {
        let p = principal(&["AGENT_ADMIN"], &[]);
        assert!(has_permission(&p, Permission::Terminate));
        assert!(has_permission(&p, Permission::IdentityLink));
    }

    #[test]
    fn auditor_is_read_only() {
        let p = principal(&["AGENT_AUDITOR"], &[]);
        assert!(has_permission(&p, Permission::Read));
        assert!(has_permission(&p, Permission::HistoryRead));
        assert!(!has_permission(&p, Permission::Activate));
        assert!(!has_permission(&p, Permission::Create));
    }

    #[test]
    fn explicit_scope_grants_single_permission() {
        let p = principal(&[], &["agent:read"]);
        assert!(has_permission(&p, Permission::Read));
        assert!(!has_permission(&p, Permission::Terminate));
    }

    #[test]
    fn unknown_role_gets_nothing() {
        let p = principal(&["SOME_OTHER_ROLE"], &[]);
        assert!(!has_permission(&p, Permission::Read));
    }
}
