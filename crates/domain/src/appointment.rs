//! AgentAppointment — an agent's appointment with an insurer/company.
//!
//! Appointments are a separate consistency boundary from the Agent
//! aggregate: they have their own lifecycle and are queried/updated
//! independently. Phase 1 models them as an entity owned by the service;
//! overlap policy: appointments for the same agent MAY overlap in time
//! only if they reference different companies — same-company overlaps are
//! rejected at the application layer (documented in DATABASE_DESIGN).

use chrono::{DateTime, NaiveDate, Utc};

use crate::error::AgentError;
use crate::ids::{AgentId, AppointmentId};

/// Appointment lifecycle status (local to this service).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppointmentStatus {
    Draft,
    Active,
    Ended,
    Cancelled,
}

impl AppointmentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Draft => "DRAFT",
            Self::Active => "ACTIVE",
            Self::Ended => "ENDED",
            Self::Cancelled => "CANCELLED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "DRAFT" => Some(Self::Draft),
            "ACTIVE" => Some(Self::Active),
            "ENDED" => Some(Self::Ended),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// An appointment record. References to insurer/company are opaque
/// strings — Agent Service does not own company master data.
#[derive(Debug, Clone)]
pub struct AgentAppointment {
    id: AppointmentId,
    agent_id: AgentId,
    appointment_reference: Option<String>,
    company_reference: Option<String>,
    appointment_type: String,
    effective_from: NaiveDate,
    effective_to: Option<NaiveDate>,
    status: AppointmentStatus,
    created_at: DateTime<Utc>,
}

impl AgentAppointment {
    /// Creates a new appointment in `DRAFT` status.
    ///
    /// `appointment_type` is a free-form classification supplied by the
    /// caller (e.g. `APPOINTMENT`, `BROKER_OF_RECORD`) — the service does
    /// not invent legal categories.
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        agent_id: AgentId,
        appointment_reference: Option<String>,
        company_reference: Option<String>,
        appointment_type: String,
        effective_from: NaiveDate,
        effective_to: Option<NaiveDate>,
        now: DateTime<Utc>,
    ) -> Result<Self, AgentError> {
        if appointment_type.trim().is_empty() || appointment_type.len() > 40 {
            return Err(AgentError::Validation(
                "appointment_type must be 1-40 chars".into(),
            ));
        }
        if let Some(to) = effective_to {
            if to < effective_from {
                return Err(AgentError::InvalidEffectiveDate(format!(
                    "effective_to {to} precedes effective_from {effective_from}"
                )));
            }
        }
        Ok(Self {
            id: AppointmentId::new(),
            agent_id,
            appointment_reference,
            company_reference,
            appointment_type,
            effective_from,
            effective_to,
            status: AppointmentStatus::Draft,
            created_at: now,
        })
    }

    /// Rehydrates from persistence.
    #[allow(clippy::too_many_arguments)]
    pub fn restore(
        id: AppointmentId,
        agent_id: AgentId,
        appointment_reference: Option<String>,
        company_reference: Option<String>,
        appointment_type: String,
        effective_from: NaiveDate,
        effective_to: Option<NaiveDate>,
        status: AppointmentStatus,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            agent_id,
            appointment_reference,
            company_reference,
            appointment_type,
            effective_from,
            effective_to,
            status,
            created_at,
        }
    }

    /// Whether the appointment covers `date` (inclusive bounds; open-ended
    /// `effective_to` means ongoing).
    pub fn covers(&self, date: NaiveDate) -> bool {
        date >= self.effective_from && self.effective_to.is_none_or(|to| date <= to)
    }

    /// Whether this appointment may currently authorize activation:
    /// status ACTIVE and covering today (UTC).
    pub fn is_currently_valid(&self, today: NaiveDate) -> bool {
        self.status == AppointmentStatus::Active && self.covers(today)
    }

    pub fn id(&self) -> AppointmentId {
        self.id
    }
    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }
    pub fn status(&self) -> AppointmentStatus {
        self.status
    }
    pub fn effective_from(&self) -> NaiveDate {
        self.effective_from
    }
    pub fn effective_to(&self) -> Option<NaiveDate> {
        self.effective_to
    }
    pub fn appointment_reference(&self) -> Option<&str> {
        self.appointment_reference.as_deref()
    }
    pub fn company_reference(&self) -> Option<&str> {
        self.company_reference.as_deref()
    }
    pub fn appointment_type(&self) -> &str {
        &self.appointment_type
    }
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_window_validation() {
        let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert!(
            AgentAppointment::create(
                AgentId::new(),
                None,
                None,
                "APPOINTMENT".into(),
                from,
                Some(to),
                Utc::now(),
            )
            .is_ok()
        );
        assert!(
            AgentAppointment::create(
                AgentId::new(),
                None,
                None,
                "APPOINTMENT".into(),
                to,
                Some(from),
                Utc::now(),
            )
            .is_err()
        );
    }

    #[test]
    fn covers_respects_bounds() {
        let from = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let to = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        let a = AgentAppointment::create(
            AgentId::new(),
            None,
            None,
            "APPOINTMENT".into(),
            from,
            Some(to),
            Utc::now(),
        )
        .unwrap();
        assert!(a.covers(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()));
        assert!(!a.covers(NaiveDate::from_ymd_opt(2027, 1, 1).unwrap()));
    }
}
