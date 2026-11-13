//! Typed domain identifiers.
//!
//! Each identifier is a newtype over UUIDv7 so IDs are time-ordered and
//! cannot be accidentally interchanged at compile time. Raw UUIDs never
//! appear in the domain model.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! typed_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a new time-ordered (UUIDv7) identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(id: Uuid) -> Self {
                Self(id)
            }

            pub fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }
    };
}

typed_id!(AgentId, "Identifier of an Agent aggregate.");
typed_id!(PersonId, "Identifier of a Person identity (human being).");
typed_id!(AppointmentId, "Identifier of an AgentAppointment record.");
typed_id!(IdentityLinkId, "Identifier of an AgentIdentityLink record.");
typed_id!(
    CommandId,
    "Identifier of a client command (idempotency scope)."
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ids_are_distinct_types() {
        let a = AgentId::new();
        let p = PersonId::new();
        assert_ne!(a.as_uuid(), p.as_uuid());
        let parsed: AgentId = a.to_string().parse().unwrap();
        assert_eq!(a, parsed);
    }
}
