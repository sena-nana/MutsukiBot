//! Host-neutral catalog for standard Mutsuki plugins.
//!
//! This crate owns plugin identities and manifest construction only. ServiceHost
//! assembly belongs to `mutsuki-std-service-host-integration`.
// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::doc_markdown)]

use mutsuki_plugin_image_render::ImageRenderConfig;
use mutsuki_plugin_io_browser_chromium::ChromiumConfig;
use mutsuki_plugin_io_http_client::HttpClientConfig;
use mutsuki_runtime_contracts::{ContractSurfaceKind, PluginManifest, SurfaceRequirement};

pub const STD_PLUGIN_IDS: [&str; 5] = [
    mutsuki_plugin_resource_memory::PLUGIN_ID,
    mutsuki_plugin_resource_sqlite::PLUGIN_ID,
    mutsuki_plugin_io_browser_chromium::PLUGIN_ID,
    mutsuki_plugin_io_http_client::PLUGIN_ID,
    mutsuki_plugin_image_render::PLUGIN_ID,
];

#[derive(Clone, Copy, Debug, Default)]
pub struct StdPluginCatalog;

impl StdPluginCatalog {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    #[must_use]
    pub const fn plugin_ids(self) -> [&'static str; 5] {
        STD_PLUGIN_IDS
    }

    #[must_use]
    pub fn memory_resource_manifest(self) -> PluginManifest {
        mutsuki_plugin_resource_memory::loaded_plugin().manifest
    }

    /// Builds the SQLite resource plugin manifest after validating deployment config.
    ///
    /// # Errors
    ///
    /// Returns an error when the database path is invalid.
    pub fn sqlite_resource_manifest(
        self,
        config: &mutsuki_plugin_resource_sqlite::SqliteResourceConfig,
    ) -> Result<PluginManifest, String> {
        config.validate()?;
        Ok(mutsuki_plugin_resource_sqlite::manifest())
    }

    /// Builds the Chromium plugin manifest after validating deployment config.
    ///
    /// # Errors
    ///
    /// Returns an error when the Chromium executable, allowlist, or limits are invalid.
    pub fn chromium_manifest(self, config: &ChromiumConfig) -> Result<PluginManifest, String> {
        config.validate()?;
        Ok(mutsuki_plugin_io_browser_chromium::manifest())
    }

    /// Builds the HTTP plugin manifest with its configured response resource dependency.
    ///
    /// # Errors
    ///
    /// Returns an error when the response provider, domain allowlist, or limits are invalid.
    pub fn http_client_manifest(self, config: &HttpClientConfig) -> Result<PluginManifest, String> {
        config.validate()?;
        let mut manifest = mutsuki_plugin_io_http_client::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            config.response_provider_id.clone(),
        ));
        Ok(manifest)
    }

    /// Builds the image renderer manifest with its configured output resource dependency.
    ///
    /// # Errors
    ///
    /// Returns an error when the output provider or deployment font files are invalid.
    pub fn image_render_manifest(
        self,
        config: &ImageRenderConfig,
    ) -> Result<PluginManifest, String> {
        config.validate()?;
        let mut manifest = mutsuki_plugin_image_render::manifest();
        manifest.requires.push(SurfaceRequirement::new(
            ContractSurfaceKind::ResourceProvider,
            config.output_provider_id.clone(),
        ));
        Ok(manifest)
    }
}

#[must_use]
pub const fn std_plugin_catalog() -> StdPluginCatalog {
    StdPluginCatalog::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_host_neutral_and_lists_each_owned_plugin_once() {
        let catalog = std_plugin_catalog();
        let ids = catalog.plugin_ids();
        assert_eq!(ids.len(), STD_PLUGIN_IDS.len());
        assert!(ids.contains(&mutsuki_plugin_resource_memory::PLUGIN_ID));
        assert!(ids.contains(&mutsuki_plugin_resource_sqlite::PLUGIN_ID));
        assert!(ids.contains(&mutsuki_plugin_image_render::PLUGIN_ID));
        assert_eq!(catalog.memory_resource_manifest().plugin_id, ids[0]);
    }
}
