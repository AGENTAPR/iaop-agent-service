//! AgentIdentityLink — maps an external authentication identity
//! (Keycloak/OIDC issuer + subject) to an Agent record.
//!
//! Hard rules:
//! - links are historical: unlinking sets `unlinked_at`, never deletes
//! - uniqueness of the ACTIVE mapping is `(issuer, subject)` — `sub` is
//!   only unique within its issuer
//! - no credential material is ever stored here
//! - linking is a controlled operation, not an automatic side effect of
//!   agent creation

use chrono::{DateTime, Utc};

use crate::error::AgentError;
use crate::ids::{AgentId, IdentityLinkId};

/// Link lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityLinkStatus {
    Active,
    Unlinked,
}

impl IdentityLinkStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Unlinked => "UNLINKED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ACTIVE" => Some(Self::Active),
            "UNLINKED" => Some(Self::Unlinked),
            _ => None,
        }
    }
}

/// A link between an Agent and an external identity provider subject.
#[derive(Debug, Clone)]
pub struct AgentIdentityLink {
    id: IdentityLinkId,
    agent_id: AgentId,
    identity_provider: String,
    issuer: String,
    subject: String,
    status: IdentityLinkStatus,
    linked_at: DateTime<Utc>,
    unlinked_at: Option<DateTime<Utc>>,
}

impl AgentIdentityLink {
    /// Creates a new ACTIVE link. Validation is minimal here — uniqueness
    /// and actor authorization are enforced by the application/repository
    /// layer (they require DB state).
    pub fn link(
        agent_id: AgentId,
        identity_provider: String,
        issuer: String,
        subject: String,
        now: DateTime<Utc>,
    ) -> Result<Self, AgentError> {
        if identity_provider.trim().is_empty() || identity_provider.len() > 80 {
            return Err(AgentError::Validation(
                "identity_provider must be 1-80 chars".into(),
            ));
        }
        if issuer.trim().is_empty() || issuer.len() > 512 {
            return Err(AgentError::Validation("issuer must be 1-512 chars".into()));
        }
        if subject.trim().is_empty() || subject.len() > 255 {
            return Err(AgentError::Validation("subject must be 1-255 chars".into()));
        }
        Ok(Self {
            id: IdentityLinkId::new(),
            agent_id,
            identity_provider,
            issuer,
            subject,
            status: IdentityLinkStatus::Active,
            linked_at: now,
            unlinked_at: None,
        })
    }

    /// Rehydrates from persistence.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: IdentityLinkId,
        agent_id: AgentId,
        identity_provider: String,
        issuer: String,
        subject: String,
        status: IdentityLinkStatus,
        linked_at: DateTime<Utc>,
        unlinked_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id,
            agent_id,
            identity_provider,
            issuer,
            subject,
            status,
            linked_at,
            unlinked_at,
        }
    }

    /// Marks the link unlinked (history preserved).
    pub fn unlink(&mut self, now: DateTime<Utc>) -> Result<(), AgentError> {
        if self.status == IdentityLinkStatus::Unlinked {
            return Err(AgentError::IdentityLinkConflict(
                "link is already unlinked".into(),
            ));
        }
        self.status = IdentityLinkStatus::Unlinked;
        self.unlinked_at = Some(now);
        Ok(())
    }

    pub fn id(&self) -> IdentityLinkId {
        self.id
    }
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }
    pub fn identity_provider(&self) -> &str {
        &self.identity_provider
    }
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn subject(&self) -> &str {
        &self.subject
    }
    pub fn status(&self) -> IdentityLinkStatus {
        self.status
    }
    pub fn linked_at(&self) -> DateTime<Utc> {
        self.linked_at
    }
    pub fn unlinked_at(&self) -> Option<DateTime<Utc>> {
        self.unlinked_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_unlink_preserves_history() {
        let l = AgentIdentityLink::link(
            AgentId::new(),
            "keycloak".into(),
            "https://keycloak.local/realms/iaop".into(),
            "user-123".into(),
            Utc::now(),
        )
        .unwrap();
        assert_eq!(l.status(), IdentityLinkStatus::Active);
        let mut l2 = l.clone();
        l2.unlink(Utc::now()).unwrap();
        assert_eq!(l2.status(), IdentityLinkStatus::Unlinked);
        assert!(l2.unlinked_at().is_some());
        assert!(l2.unlink(Utc::now()).is_err());
    }

    #[test]
    fn rejects_empty_fields() {
        assert!(
            AgentIdentityLink::link(
                AgentId::new(),
                " ".into(),
                "iss".into(),
                "sub".into(),
                Utc::now()
            )
            .is_err()
        );
    }
}
