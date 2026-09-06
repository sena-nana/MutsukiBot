//! Explicit ServiceHost assembly for host-neutral standard plugins.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::doc_markdown)]

use mutsuki_plugin_image_render::{ImageRenderConfig, ImageRenderRunner};
use mutsuki_plugin_io_browser_chromium::{BrowserSnapshotRunner, ChromiumConfig};
use mutsuki_plugin_io_http_client::{HttpClientConfig, HttpEffectHandler, SecureHttpGateway};
use mutsuki_service_runtime::{
    ConfiguredPluginCatalog, ConfiguredPluginFactory, ServiceRuntimeBuilder, ServiceRuntimeResult,
};
use mutsuki_std_plugins::std_plugin_catalog;
use serde_json::Value;

pub struct MemoryResourcePluginFactory;

impl ConfiguredPluginFactory for MemoryResourcePluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_resource_memory::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        if !config.is_null() && config.as_object().is_none_or(|object| !object.is_empty()) {
            return Err("memory resource provider does not accept product configuration".into());
        }
        let manifest = std_plugin_catalog().memory_resource_manifest();
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, || {
                Ok::<_, String>(mutsuki_plugin_resource_memory::loaded_plugin())
            }),
        )
    }
}

pub struct SqliteResourcePluginFactory;

impl ConfiguredPluginFactory for SqliteResourcePluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_resource_sqlite::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: mutsuki_plugin_resource_sqlite::SqliteResourceConfig =
            serde_json::from_value(config.clone())
                .map_err(|error| format!("invalid sqlite resource provider config: {error}"))?;
        config.validate()?;
        let database_path = config.database_path.clone();
        let manifest = std_plugin_catalog().sqlite_resource_manifest(&config)?;
        Ok(
            builder.register_builtin_loaded_plugin_factory(manifest, move || {
                let provider = mutsuki_plugin_resource_sqlite::SqliteResourceProvider::open(
                    std::path::Path::new(&database_path),
                )
                .map_err(|error| format!("{}: {}", error.error().code, error.error().route))?;
                Ok::<_, String>(mutsuki_plugin_resource_sqlite::loaded_plugin_with_provider(
                    provider,
                ))
            }),
        )
    }
}

pub struct ChromiumPluginFactory;

impl ConfiguredPluginFactory for ChromiumPluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_io_browser_chromium::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: ChromiumConfig = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid Chromium plugin config: {error}"))?;
        let manifest = std_plugin_catalog().chromium_manifest(&config)?;
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                BrowserSnapshotRunner::launch(config.clone(), resources)
                    .map(|runner| Box::new(runner) as Box<dyn mutsuki_runtime_core::Runner>)
            }))
    }
}

pub struct ImageRenderPluginFactory;

impl ConfiguredPluginFactory for ImageRenderPluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_image_render::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: ImageRenderConfig = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid image renderer config: {error}"))?;
        let manifest = std_plugin_catalog().image_render_manifest(&config)?;
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_fallible_runtime_services_runner(move |_client, resources| {
                ImageRenderRunner::launch(config.clone(), resources)
                    .map(|runner| Box::new(runner) as Box<dyn mutsuki_runtime_core::Runner>)
            }))
    }
}

pub struct HttpClientPluginFactory;

impl ConfiguredPluginFactory for HttpClientPluginFactory {
    fn plugin_id(&self) -> &str {
        mutsuki_plugin_io_http_client::PLUGIN_ID
    }

    fn prepare(
        &self,
        config: &Value,
        builder: ServiceRuntimeBuilder,
    ) -> Result<ServiceRuntimeBuilder, String> {
        let config: HttpClientConfig = serde_json::from_value(config.clone())
            .map_err(|error| format!("invalid HTTP client plugin config: {error}"))?;
        let manifest = std_plugin_catalog().http_client_manifest(&config)?;
        let response_provider_id = config.response_provider_id.clone();
        Ok(builder
            .register_builtin_plugin(manifest)
            .register_runtime_client_runner(mutsuki_plugin_io_http_client::facade_runner)
            .register_fallible_runtime_services_async_handler(move |_client, resources| {
                let gateway = SecureHttpGateway::new(config.clone())?;
                Ok::<std::sync::Arc<dyn mutsuki_runtime_core::AsyncBatchHandler>, String>(
                    std::sync::Arc::new(HttpEffectHandler::new(
                        std::sync::Arc::new(gateway),
                        resources,
                        response_provider_id.clone(),
                    )),
                )
            }))
    }
}

/// Builds the ServiceHost configured-factory catalog for standard plugins.
///
/// # Errors
///
/// Returns an error if two factories claim the same plugin identity.
pub fn configured_std_plugin_catalog() -> ServiceRuntimeResult<ConfiguredPluginCatalog> {
    let mut catalog = ConfiguredPluginCatalog::new();
    catalog.register(MemoryResourcePluginFactory)?;
    catalog.register(SqliteResourcePluginFactory)?;
    catalog.register(ChromiumPluginFactory)?;
    catalog.register(HttpClientPluginFactory)?;
    catalog.register(ImageRenderPluginFactory)?;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_catalog_contains_each_host_integration_factory() {
        configured_std_plugin_catalog().unwrap();
    }
}
