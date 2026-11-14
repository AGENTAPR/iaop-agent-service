use utoipa::OpenApi;

use crate::dto::*;

/// OpenAPI 3.1 document for the Agent Registry API.
///
/// Served at `/api/docs/openapi.json`. Every operation documents its
/// authorization requirement; problem responses use the shared
/// `ProblemDetails` schema.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "IAOP Agent Registry & Identity Service",
        version = "1.0.0",
        description = "Agent Lifecycle bounded context: agent registry, appointments, identity links, lifecycle state machine. Production-equivalent reference implementation.",
        license(name = "Proprietary")
    ),
    paths(
        crate::routes::live,
        crate::routes::ready,
        crate::routes::system_info,
        crate::agents::create_agent,
        crate::agents::list_agents,
        crate::agents::get_agent,
        crate::agents::get_agent_by_code,
        crate::agents::submit_for_activation,
        crate::agents::activate_agent,
        crate::agents::suspend_agent,
        crate::agents::reinstate_agent,
        crate::agents::terminate_agent,
        crate::agents::lifecycle_history,
        crate::agents::agent_appointments,
        crate::agents::agent_identity_links,
        crate::agents::link_identity,
        crate::agents::unlink_identity,
        crate::agents::resolve_identity,
    ),
    components(schemas(
        HealthResponse,
        SystemInfoResponse,
        ProblemDetails,
        AgentViewDto,
        AgentListResponse,
        CreateAgentRequest,
        LifecycleCommandRequest,
        AppointmentDto,
        IdentityLinkDto,
        LinkIdentityRequest,
        LifecycleHistoryItemDto,
    )),
    tags(
        (name = "system", description = "Health and service metadata"),
        (name = "agents", description = "Agent registry and lifecycle"),
    ),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_openapi_spec() {
        let json = ApiDoc::openapi().to_pretty_json().unwrap();
        std::fs::write("../../target/agent_service_openapi.json", json).unwrap();
    }
}
