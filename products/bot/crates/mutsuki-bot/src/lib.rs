// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::collapsible_if,
    clippy::default_trait_access,
    clippy::doc_markdown,
    clippy::field_reassign_with_default,
    clippy::items_after_test_module,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_pass_by_value,
    clippy::single_match_else,
    clippy::struct_excessive_bools,
    clippy::too_many_lines
)]

use std::sync::Arc;

use mutsuki_agent_service_host_integration::{
    AgentConnectionRegistry, LocalAgentRuntimeExtension,
    configured_standard_agent_plugin_catalog_with_extensions,
};
use mutsuki_bot_flow::{BotFlowRegistry, BotNodeCatalog};
use mutsuki_bot_service_host_integration::configured_bot_plugin_catalog_with_agent_and_flow;
use mutsuki_bot_service_host_integration::qq_full_business_flow;
use mutsuki_plugin_bot_flow_agent_tool::{
    flow_tool_descriptors, flow_tool_manifest, flow_tool_runner,
};
use mutsuki_service_config::{ExecutionClassName, ExecutionDomainSection, ServiceConfig};
use mutsuki_service_runtime::{ServiceRuntimeBuilder, ServiceRuntimeResult};
use mutsuki_std_service_host_integration::configured_std_plugin_catalog;

mod bootstrap;
mod distribution;
mod lifecycle;
mod product_config;
#[cfg(feature = "web-console")]
mod product_runtime;
#[cfg(feature = "web-console")]
mod web_console;
pub use bootstrap::*;
pub use distribution::*;
pub use lifecycle::*;
pub use product_config::*;
#[cfg(feature = "web-console")]
pub use product_runtime::*;
#[cfg(feature = "web-console")]
pub use web_console::*;

/// Assemble a neutral ServiceRuntime from owner-provided plugin factories.
/// Configuration selects every platform, route, business plugin and provider.
pub fn apply_product_runtime_profile(service: &mut ServiceConfig) {
    if service.service.profile != "bot" || !service.core.execution_domains.is_empty() {
        return;
    }
    service.core.execution_domains = vec![
        execution_domain("bot-control", vec![ExecutionClassName::Orchestration], 2),
        execution_domain("network-io", vec![ExecutionClassName::Io], 4),
        execution_domain("blocking-adapters", vec![ExecutionClassName::Blocking], 2),
        execution_domain(
            "agent-compute",
            vec![ExecutionClassName::Cpu, ExecutionClassName::Script],
            2,
        ),
    ];
}

fn execution_domain(
    id: &str,
    execution_classes: Vec<ExecutionClassName>,
    threads: usize,
) -> ExecutionDomainSection {
    ExecutionDomainSection {
        id: id.into(),
        execution_classes,
        threads,
        ..ExecutionDomainSection::default()
    }
}

pub fn assemble_service(
    service: ServiceConfig,
    config: Arc<mutsuki_config_service::ConfigService>,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    assemble_service_with_connections(service, config, AgentConnectionRegistry::new())
}

pub fn assemble_service_with_connections(
    service: ServiceConfig,
    config: Arc<mutsuki_config_service::ConfigService>,
    agent_connections: AgentConnectionRegistry,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    assemble_service_with_flow_registry(
        service,
        config,
        agent_connections,
        Arc::new(BotFlowRegistry::new(BotNodeCatalog::default())),
    )
}

/// Assembly variant that pins the shared `BotFlowRegistry`: the flow router,
/// the console bridge and the co-located Agent flow tools must observe one
/// active graph.
pub fn assemble_service_with_flow_registry(
    mut service: ServiceConfig,
    config: Arc<mutsuki_config_service::ConfigService>,
    agent_connections: AgentConnectionRegistry,
    flow_registry: Arc<BotFlowRegistry>,
) -> ServiceRuntimeResult<ServiceRuntimeBuilder> {
    apply_product_runtime_profile(&mut service);
    let mut catalog = configured_std_plugin_catalog()?;
    catalog.merge(configured_standard_agent_plugin_catalog_with_extensions(
        agent_connections.clone(),
        config.clone(),
        vec![flow_tool_extension(config.clone(), flow_registry.clone())],
    )?)?;
    catalog.merge(configured_bot_plugin_catalog_with_agent_and_flow(
        config,
        agent_connections,
        flow_registry,
        Some(qq_full_business_flow()),
        MEDIA_RESOURCE_PROVIDER_ID.to_string(),
    )?)?;
    Ok(ServiceRuntimeBuilder::new(service).with_configured_plugin_catalog(catalog))
}

/// Co-located Bot Flow tool extension installed into the local in-process
/// Agent engine. Assembly only wires the shared ConfigService and flow
/// registry into the target runner owned by `mutsuki-plugin-bot-flow-agent-tool`.
fn flow_tool_extension(
    config: Arc<mutsuki_config_service::ConfigService>,
    flow_registry: Arc<BotFlowRegistry>,
) -> LocalAgentRuntimeExtension {
    LocalAgentRuntimeExtension {
        manifests: vec![flow_tool_manifest()],
        runners: vec![Arc::new(move |client| {
            flow_tool_runner(client, config.clone(), flow_registry.clone())
        })],
        tools: flow_tool_descriptors(),
    }
}
