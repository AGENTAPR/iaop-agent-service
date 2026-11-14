//! Agent lifecycle status and the legal transition table.
//!
//! Phase 1 baseline state machine:
//!
//! ```text
//! DRAFT -> PENDING_ACTIVATION -> ACTIVE <-> SUSPENDED -> TERMINATED
//! ```
//!
//! `TERMINATED` is final. `SUSPENDED -> ACTIVE` is a controlled
//! reinstatement, not an unconditional activation — it still requires
//! activation authorization evidence.

use serde::{Deserialize, Serialize};

/// Administrative lifecycle status of an Agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AgentStatus {
    Draft,
    PendingActivation,
    Active,
    Suspended,
    Terminated,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::PendingActivation => "PENDING_ACTIVATION",
            Self::Active => "ACTIVE",
            Self::Suspended => "SUSPENDED",
            Self::Terminated => "TERMINATED",
        }
    }

    /// Parses the database representation. Unknown values are rejected —
    /// the DB check constraint and this parser must stay in sync.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "DRAFT" => Some(Self::Draft),
            "PENDING_ACTIVATION" => Some(Self::PendingActivation),
            "ACTIVE" => Some(Self::Active),
            "SUSPENDED" => Some(Self::Suspended),
            "TERMINATED" => Some(Self::Terminated),
            _ => None,
        }
    }

    /// Whether `self -> target` is a legal lifecycle transition.
    pub fn can_transition_to(&self, target: Self) -> bool {
        use AgentStatus::*;
        matches!(
            (self, target),
            (Draft, PendingActivation)
                | (Draft, Terminated)
                | (PendingActivation, Active)
                | (PendingActivation, Terminated)
                | (Active, Suspended)
                | (Active, Terminated)
                | (Suspended, Active)
                | (Suspended, Terminated)
        )
    }

    /// Whether this status permits selling-related operations to even be
    /// *considered*. This is NOT a selling-eligibility decision — that
    /// belongs to the Eligibility bounded context. `ACTIVE` merely means
    /// administratively active.
    pub fn is_administratively_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminated_is_final() {
        for s in [
            AgentStatus::Draft,
            AgentStatus::PendingActivation,
            AgentStatus::Active,
            AgentStatus::Suspended,
            AgentStatus::Terminated,
        ] {
            assert!(!AgentStatus::Terminated.can_transition_to(s));
        }
    }

    #[test]
    fn no_self_transitions() {
        for s in [
            AgentStatus::Draft,
            AgentStatus::PendingActivation,
            AgentStatus::Active,
            AgentStatus::Suspended,
            AgentStatus::Terminated,
        ] {
            assert!(!s.can_transition_to(s), "{s} must not self-transition");
        }
    }

    #[test]
    fn draft_cannot_skip_to_active() {
        assert!(!AgentStatus::Draft.can_transition_to(AgentStatus::Active));
    }

    #[test]
    fn suspended_cannot_go_to_pending() {
        assert!(!AgentStatus::Suspended.can_transition_to(AgentStatus::PendingActivation));
    }

    #[test]
    fn parse_roundtrip() {
        for s in [
            AgentStatus::Draft,
            AgentStatus::PendingActivation,
            AgentStatus::Active,
            AgentStatus::Suspended,
            AgentStatus::Terminated,
        ] {
            assert_eq!(AgentStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(AgentStatus::parse("NOPE"), None);
    }
}
