//! SQLx/PostgreSQL implementation of the application `AgentStore` port.
//!
//! All mutating methods execute inside the caller's transaction.
//! Row<->domain mapping lives here only — domain types never see SQL.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use iaop_agent_service_application::error::AppError;
use iaop_agent_service_application::ports::{
    AgentStore, AuditRecord, IntegrationEvent, LifecycleHistoryRecord,
};
use iaop_agent_service_domain::{
    Agent, AgentAppointment, AgentCode, AgentId, AgentIdentityLink, AgentStatus, AppointmentId,
    AppointmentStatus, IdentityLinkId, IdentityLinkStatus, LifecycleTransition, PersonId,
};

pub struct PgAgentStore;

impl PgAgentStore {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PgAgentStore {
    fn default() -> Self {
        Self::new()
    }
}

fn db_err(e: sqlx::Error) -> AppError {
    AppError::infra(e)
}

fn agent_from_row(row: &sqlx::postgres::PgRow) -> Result<Agent, AppError> {
    let status_str: String = row.try_get("lifecycle_status").map_err(db_err)?;
    let status = AgentStatus::parse(&status_str)
        .ok_or_else(|| AppError::infra_msg(format!("unknown lifecycle_status '{status_str}'")))?;
    let code_str: String = row.try_get("agent_code").map_err(db_err)?;
    let code = AgentCode::new(&code_str).map_err(AppError::Domain)?;
    Ok(Agent::restore(
        AgentId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db_err)?),
        PersonId::from_uuid(row.try_get::<Uuid, _>("person_id").map_err(db_err)?),
        code,
        status,
        row.try_get::<i64, _>("version").map_err(db_err)? as u64,
        row.try_get::<DateTime<Utc>, _>("created_at")
            .map_err(db_err)?,
        row.try_get::<Option<String>, _>("created_by")
            .map_err(db_err)?,
    ))
}

fn appointment_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentAppointment, AppError> {
    let status_str: String = row.try_get("status").map_err(db_err)?;
    let status = AppointmentStatus::parse(&status_str)
        .ok_or_else(|| AppError::infra_msg(format!("unknown appointment status '{status_str}'")))?;
    Ok(AgentAppointment::restore(
        AppointmentId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db_err)?),
        AgentId::from_uuid(row.try_get::<Uuid, _>("agent_id").map_err(db_err)?),
        row.try_get::<Option<String>, _>("appointment_reference")
            .map_err(db_err)?,
        row.try_get::<Option<String>, _>("company_reference")
            .map_err(db_err)?,
        row.try_get::<String, _>("appointment_type")
            .map_err(db_err)?,
        row.try_get::<NaiveDate, _>("effective_from")
            .map_err(db_err)?,
        row.try_get::<Option<NaiveDate>, _>("effective_to")
            .map_err(db_err)?,
        status,
        row.try_get::<DateTime<Utc>, _>("created_at")
            .map_err(db_err)?,
    ))
}

fn link_from_row(row: &sqlx::postgres::PgRow) -> Result<AgentIdentityLink, AppError> {
    let status_str: String = row.try_get("status").map_err(db_err)?;
    let status = IdentityLinkStatus::parse(&status_str)
        .ok_or_else(|| AppError::infra_msg(format!("unknown link status '{status_str}'")))?;
    Ok(AgentIdentityLink::restore(
        IdentityLinkId::from_uuid(row.try_get::<Uuid, _>("id").map_err(db_err)?),
        AgentId::from_uuid(row.try_get::<Uuid, _>("agent_id").map_err(db_err)?),
        row.try_get::<String, _>("identity_provider")
            .map_err(db_err)?,
        row.try_get::<String, _>("issuer").map_err(db_err)?,
        row.try_get::<String, _>("subject").map_err(db_err)?,
        status,
        row.try_get::<DateTime<Utc>, _>("linked_at")
            .map_err(db_err)?,
        row.try_get::<Option<DateTime<Utc>>, _>("unlinked_at")
            .map_err(db_err)?,
    ))
}

impl AgentStore for PgAgentStore {
    // ---------------------------------------------------------- commands

    async fn insert_agent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        agent: &Agent,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO agents (id, person_id, agent_code, lifecycle_status, version, \
             created_at, updated_at, created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,now(),$7)",
        )
        .bind(agent.id().as_uuid())
        .bind(agent.person_id().as_uuid())
        .bind(agent.agent_code().as_str())
        .bind(agent.status().as_str())
        .bind(agent.version() as i64)
        .bind(agent.created_at())
        .bind(agent.created_by().map(str::to_string))
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn load_agent_for_update(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: AgentId,
    ) -> Result<Option<Agent>, AppError> {
        let row = sqlx::query(
            "SELECT id, person_id, agent_code, lifecycle_status, version, created_at, created_by              FROM agents WHERE id = $1 FOR UPDATE",
        )
            .bind(id.as_uuid())
            .fetch_optional(&mut **tx)
            .await
            .map_err(db_err)?;
        row.map(|r| agent_from_row(&r)).transpose()
    }

    async fn update_agent(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        agent: &Agent,
        updated_by: &str,
    ) -> Result<bool, AppError> {
        // Optimistic check: stored version must equal version-1 (the
        // aggregate increments by exactly 1 per transition).
        let expected = agent.version() as i64 - 1;
        let res = sqlx::query(
            "UPDATE agents SET lifecycle_status=$1, version=$2, updated_at=now(), updated_by=$3 \
             WHERE id=$4 AND version=$5",
        )
        .bind(agent.status().as_str())
        .bind(agent.version() as i64)
        .bind(updated_by)
        .bind(agent.id().as_uuid())
        .bind(expected)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected() == 1)
    }

    async fn insert_lifecycle_history(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &LifecycleTransition,
        actor: &str,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO agent_lifecycle_history \
             (id, agent_id, from_status, to_status, reason_code, reason_text, \
              effective_at, performed_at, performed_by, correlation_id, command_id, aggregate_version) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,now(),$8,$9,$10,$11)",
        )
        .bind(Uuid::now_v7())
        .bind(record.agent_id.as_uuid())
        .bind(record.from_status.map(|s| s.as_str()))
        .bind(record.to_status.as_str())
        .bind(&record.reason_code)
        .bind(&record.reason_text)
        .bind(record.effective_at)
        .bind(actor)
        .bind(record.correlation_id)
        .bind(record.command_id.as_uuid())
        .bind(record.aggregate_version as i64)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn insert_audit(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        record: &AuditRecord,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO agent_audit_events \
             (id, agent_id, actor, action, before_state, after_state, reason, source, \
              occurred_at, correlation_id, command_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(record.id)
        .bind(record.agent_id.as_uuid())
        .bind(&record.actor)
        .bind(&record.action)
        .bind(&record.before_state)
        .bind(&record.after_state)
        .bind(&record.reason)
        .bind(&record.source)
        .bind(record.occurred_at)
        .bind(record.correlation_id)
        .bind(record.command_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn insert_outbox(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        event: &IntegrationEvent,
    ) -> Result<(), AppError> {
        // `payload` stores the event payload; envelope metadata maps to
        // dedicated columns added in migration 20261114000002.
        sqlx::query(
            "INSERT INTO outbox_events \
             (event_id, aggregate_id, aggregate_type, aggregate_version, event_type, \
              event_version, payload, occurred_at, source, correlation_id, causation_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(event.event_id)
        .bind(event.aggregate_id)
        .bind("Agent")
        .bind(event.aggregate_version.map(|v| v as i64))
        .bind(&event.event_type)
        .bind(event.event_version as i32)
        .bind(&event.payload)
        .bind(event.occurred_at)
        .bind(&event.source)
        .bind(event.correlation_id)
        .bind(event.causation_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn insert_appointment(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        appointment: &AgentAppointment,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO agent_appointments \
             (id, agent_id, appointment_reference, company_reference, appointment_type, \
              effective_from, effective_to, status, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,now())",
        )
        .bind(appointment.id().as_uuid())
        .bind(appointment.agent_id().as_uuid())
        .bind(appointment.appointment_reference().map(str::to_string))
        .bind(appointment.company_reference().map(str::to_string))
        .bind(appointment.appointment_type())
        .bind(appointment.effective_from())
        .bind(appointment.effective_to())
        .bind(appointment.status().as_str())
        .bind(appointment.created_at())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn insert_identity_link(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        link: &AgentIdentityLink,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT INTO agent_identity_links \
             (id, agent_id, identity_provider, issuer, subject, status, linked_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(link.id().as_uuid())
        .bind(link.agent_id().as_uuid())
        .bind(link.identity_provider())
        .bind(link.issuer())
        .bind(link.subject())
        .bind(link.status().as_str())
        .bind(link.linked_at())
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    async fn update_identity_link(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        link: &AgentIdentityLink,
    ) -> Result<(), AppError> {
        sqlx::query("UPDATE agent_identity_links SET status=$1, unlinked_at=$2 WHERE id=$3")
            .bind(link.status().as_str())
            .bind(link.unlinked_at())
            .bind(link.id().as_uuid())
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    // ------------------------------------------------------------ queries

    async fn find_agent_by_id(
        &self,
        pool: &PgPool,
        id: AgentId,
    ) -> Result<Option<Agent>, AppError> {
        let row = sqlx::query(
            "SELECT id, person_id, agent_code, lifecycle_status, version, created_at, created_by              FROM agents WHERE id = $1",
        )
            .bind(id.as_uuid())
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;
        row.map(|r| agent_from_row(&r)).transpose()
    }

    async fn find_agent_by_code(
        &self,
        pool: &PgPool,
        code: &AgentCode,
    ) -> Result<Option<Agent>, AppError> {
        let row = sqlx::query(
            "SELECT id, person_id, agent_code, lifecycle_status, version, created_at, created_by              FROM agents WHERE agent_code = $1",
        )
            .bind(code.as_str())
            .fetch_optional(pool)
            .await
            .map_err(db_err)?;
        row.map(|r| agent_from_row(&r)).transpose()
    }

    async fn list_agents(
        &self,
        pool: &PgPool,
        after: Option<AgentId>,
        limit: i64,
    ) -> Result<Vec<Agent>, AppError> {
        // UUIDv7 ids are time-ordered; keyset pagination on id is stable.
        let rows = sqlx::query(
            "SELECT id, person_id, agent_code, lifecycle_status, version, created_at, created_by \
             FROM agents WHERE ($1::uuid IS NULL OR id > $1) \
             ORDER BY id LIMIT $2",
        )
        .bind(after.map(|a| a.as_uuid()))
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(agent_from_row).collect()
    }

    async fn find_appointment(
        &self,
        pool: &PgPool,
        id: AppointmentId,
    ) -> Result<Option<AgentAppointment>, AppError> {
        let row = sqlx::query(
            "SELECT id, agent_id, appointment_reference, company_reference, appointment_type, \
             effective_from, effective_to, status, created_at \
             FROM agent_appointments WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
        row.map(|r| appointment_from_row(&r)).transpose()
    }

    async fn list_appointments(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
    ) -> Result<Vec<AgentAppointment>, AppError> {
        let rows = sqlx::query(
            "SELECT id, agent_id, appointment_reference, company_reference, appointment_type, \
             effective_from, effective_to, status, created_at \
             FROM agent_appointments WHERE agent_id = $1 ORDER BY effective_from, id",
        )
        .bind(agent_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(appointment_from_row).collect()
    }

    async fn find_identity_link(
        &self,
        pool: &PgPool,
        id: IdentityLinkId,
    ) -> Result<Option<AgentIdentityLink>, AppError> {
        let row = sqlx::query(
            "SELECT id, agent_id, identity_provider, issuer, subject, status, linked_at, unlinked_at \
             FROM agent_identity_links WHERE id = $1",
        )
        .bind(id.as_uuid())
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
        row.map(|r| link_from_row(&r)).transpose()
    }

    async fn list_identity_links(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
    ) -> Result<Vec<AgentIdentityLink>, AppError> {
        let rows = sqlx::query(
            "SELECT id, agent_id, identity_provider, issuer, subject, status, linked_at, unlinked_at \
             FROM agent_identity_links WHERE agent_id = $1 ORDER BY linked_at, id",
        )
        .bind(agent_id.as_uuid())
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        rows.iter().map(link_from_row).collect()
    }

    async fn find_identity_link_by_subject(
        &self,
        pool: &PgPool,
        issuer: &str,
        subject: &str,
    ) -> Result<Option<AgentIdentityLink>, AppError> {
        let row = sqlx::query(
            "SELECT id, agent_id, identity_provider, issuer, subject, status, linked_at, unlinked_at \
             FROM agent_identity_links WHERE issuer = $1 AND subject = $2 AND status = 'ACTIVE'",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
        row.map(|r| link_from_row(&r)).transpose()
    }

    async fn list_lifecycle_history(
        &self,
        pool: &PgPool,
        agent_id: AgentId,
        limit: i64,
    ) -> Result<Vec<LifecycleHistoryRecord>, AppError> {
        let rows = sqlx::query(
            "SELECT id, agent_id, from_status, to_status, reason_code, reason_text, \
             effective_at, performed_at, performed_by, correlation_id, command_id, aggregate_version \
             FROM agent_lifecycle_history WHERE agent_id = $1 \
             ORDER BY effective_at DESC, id DESC LIMIT $2",
        )
        .bind(agent_id.as_uuid())
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        rows.iter()
            .map(|r| {
                Ok(LifecycleHistoryRecord {
                    id: r.try_get::<Uuid, _>("id").map_err(db_err)?,
                    agent_id: AgentId::from_uuid(r.try_get::<Uuid, _>("agent_id").map_err(db_err)?),
                    from_status: r
                        .try_get::<Option<String>, _>("from_status")
                        .map_err(db_err)?,
                    to_status: r.try_get::<String, _>("to_status").map_err(db_err)?,
                    reason_code: r
                        .try_get::<Option<String>, _>("reason_code")
                        .map_err(db_err)?,
                    reason_text: r
                        .try_get::<Option<String>, _>("reason_text")
                        .map_err(db_err)?,
                    effective_at: r
                        .try_get::<DateTime<Utc>, _>("effective_at")
                        .map_err(db_err)?,
                    performed_at: r
                        .try_get::<DateTime<Utc>, _>("performed_at")
                        .map_err(db_err)?,
                    performed_by: r
                        .try_get::<Option<String>, _>("performed_by")
                        .map_err(db_err)?,
                    correlation_id: r
                        .try_get::<Option<Uuid>, _>("correlation_id")
                        .map_err(db_err)?,
                    command_id: r.try_get::<Uuid, _>("command_id").map_err(db_err)?,
                    aggregate_version: r.try_get::<i64, _>("aggregate_version").map_err(db_err)?,
                })
            })
            .collect()
    }
}
