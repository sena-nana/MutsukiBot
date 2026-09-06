use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mutsuki_agent_service_host_integration::AgentConnectionRegistry;
use mutsuki_config_service::{
    ConfigApplyMode, ConfigConstraints, ConfigContext, ConfigDescriptor, ConfigKey,
    ConfigMutability, ConfigNode, ConfigPresentation, ConfigProviderId, ConfigScope, ConfigService,
    ConfigValue, ConfigValueType, LocalizedText, MemoryConfigProvider, RestartPolicy, capability,
};
use mutsuki_plugin_config_sqlite::SqliteConfigRepository;
use mutsuki_service_config::{
    ConfiguredPluginSelection, ServiceConfig, recover_host_secret_transaction,
};

use crate::{
    PRODUCT_CONFIG_PROVIDER_ID, ProductConfigOptions, configured_product_owner_selections,
    configured_product_selections, product_config_service_with_options, product_seed_defaults,
    register_configured_product_providers,
};

pub const SERVICE_CONFIG_PROVIDER_ID: &str = "mutsuki.service.runtime";
pub const CONSOLE_AUTH_TOKEN_KEY: &str = "mutsuki.web.console.token";
pub const CONSOLE_AUTH_TOKEN_ENV: &str = "MUTSUKI_SECRET_MUTSUKI_WEB_CONSOLE_TOKEN";
pub const CONSOLE_LISTEN_ENV: &str = "MUTSUKI_CONSOLE_LISTEN";
const INSTANCE_DIR: &str = ".mutsuki-bot";
const CONFIG_NAMESPACE: &str = "mutsuki-bot";

pub struct SingleInstanceProduct {
    pub service: ServiceConfig,
    pub config: Arc<ConfigService>,
    pub console: LocalConsoleConfig,
    pub root: PathBuf,
    pub agent_connections: AgentConnectionRegistry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalConsoleConfig {
    pub enabled: bool,
    pub listen: String,
    pub auth_token_key: Option<String>,
    pub extensions: Vec<String>,
    pub release_set: Option<String>,
}

pub async fn load_single_instance_product() -> Result<SingleInstanceProduct, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("single_instance.executable_unavailable: {error}"))?;
    let root = single_instance_root(&executable)?;
    load_single_instance_product_at(&root, interactive_admin_passphrase).await
}

/// Test harness entry for isolating the otherwise fixed single-instance directory.
/// Production callers must use [`load_single_instance_product`].
#[doc(hidden)]
pub async fn load_single_instance_product_for_test(
    root: &Path,
    admin_passphrase: &str,
) -> Result<SingleInstanceProduct, String> {
    let admin_passphrase = admin_passphrase.to_owned();
    load_single_instance_product_at(root, move || Ok(admin_passphrase.clone())).await
}

fn single_instance_root(executable: &Path) -> Result<PathBuf, String> {
    executable
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join(INSTANCE_DIR))
        .ok_or_else(|| "single_instance.executable_parent_missing".to_string())
}

async fn load_single_instance_product_at<F>(
    root: &Path,
    prompt: F,
) -> Result<SingleInstanceProduct, String>
where
    F: FnMut() -> Result<String, String>,
{
    ensure_single_instance_directories(root)?;
    let secret_path = root.join("secrets.toml");
    recover_host_secret_transaction(&secret_path).map_err(|error| error.to_string())?;
    ensure_local_auth_secret(&secret_path, prompt)?;
    let repository_path = root.join("config.sqlite3");
    let repository = Arc::new(
        SqliteConfigRepository::open(repository_path, CONFIG_NAMESPACE)
            .map_err(|error| error.to_string())?,
    );
    let config = product_config_service_with_options(ProductConfigOptions::new(repository))
        .map_err(|error| error.to_string())?;
    config
        .create_if_absent(
            PRODUCT_CONFIG_PROVIDER_ID,
            product_seed_defaults(),
            ConfigContext::global(),
        )
        .await
        .map_err(|error| error.to_string())?;
    let seed = service_seed(root);
    config
        .registry()
        .register(Arc::new(MemoryConfigProvider::new(
            service_descriptor(),
            ConfigValue::from_json(
                &serde_json::to_value(&seed).map_err(|error| error.to_string())?,
            ),
            ConfigApplyMode::RequireRestart,
        )))
        .map_err(|error| error.to_string())?;
    config
        .create_if_absent(
            SERVICE_CONFIG_PROVIDER_ID,
            ConfigValue::from_json(
                &serde_json::to_value(&seed).map_err(|error| error.to_string())?,
            ),
            ConfigContext::global(),
        )
        .await
        .map_err(|error| error.to_string())?;

    let snapshot = config
        .read(
            SERVICE_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .map_err(|error| error.to_string())?;
    let mut service: ServiceConfig = serde_json::from_value(snapshot.value.to_json())
        .map_err(|error| format!("stored ServiceConfig is invalid: {error}"))?;
    apply_single_instance_boundaries(&mut service, root);

    let product_snapshot = config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .map_err(|error| error.to_string())?;
    if product_snapshot.schema_version != 3 || product_snapshot.value_version != 3 {
        return Err(format!(
            "product.config.version_unsupported: stored mutsuki.product schema/value version {}/{} is unsupported; recreate the product config repository for version 3",
            product_snapshot.schema_version, product_snapshot.value_version
        ));
    }
    let product = product_snapshot.value.to_json();
    let mut console = console_config(&product);
    if let Ok(listen) = std::env::var(CONSOLE_LISTEN_ENV)
        && !listen.is_empty()
    {
        console.listen = listen;
    }
    let boundary = root.join("instance.boundary");
    let mut service = service
        .finalize_bootstrap(&boundary, None)
        .map_err(|error| error.to_string())?;
    register_configured_product_providers(&config, service.host_secret_store())
        .await
        .map_err(|error| error.to_string())?;
    let owner_selections = configured_product_owner_selections(&config)
        .await
        .map_err(|error| error.to_string())?;
    service.plugins.configured = configured_product_selections(&product, owner_selections)
        .map_err(|error| error.to_string())?;
    apply_lilia_image_render_fonts(&mut service, root)?;
    ensure_sqlite_resource_plugin(&mut service, root)?;
    let agent_connections = AgentConnectionRegistry::new();
    Ok(SingleInstanceProduct {
        service,
        config,
        console,
        root: root.to_path_buf(),
        agent_connections,
    })
}

fn ensure_single_instance_directories(root: &Path) -> Result<(), String> {
    for path in [
        root.to_path_buf(),
        root.join("data"),
        root.join("logs"),
        root.join("run"),
        root.join("plugins/installed"),
        root.join("plugins/disabled"),
    ] {
        std::fs::create_dir_all(&path).map_err(|error| {
            format!(
                "single_instance.directory_unavailable: failed to create {}: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn service_seed(root: &Path) -> ServiceConfig {
    let mut service = ServiceConfig::default();
    service.service.profile = "bot".into();
    service.plugins.configured.clear();
    apply_single_instance_boundaries(&mut service, root);
    service
}

const IMAGE_RENDER_PLUGIN_ID: &str = "mutsuki.std.image.render";
const LILIA_FONT_FILES: &[&str] = &[
    "noto-sans-sc-chinese-simplified-400-normal.woff2",
    "noto-sans-sc-chinese-simplified-500-normal.woff2",
    "noto-sans-sc-chinese-simplified-600-normal.woff2",
];

/// Seeds the persistent media resource provider every product plugin binds to.
/// An existing selection keeps its configured database path; only a missing
/// path falls back to the instance data directory.
fn ensure_sqlite_resource_plugin(service: &mut ServiceConfig, root: &Path) -> Result<(), String> {
    let config = serde_json::json!({
        "database_path": root.join("data").join("resources.sqlite").to_string_lossy(),
    });
    if let Some(plugin) = service
        .plugins
        .configured
        .iter_mut()
        .find(|plugin| plugin.id == mutsuki_plugin_resource_sqlite::PLUGIN_ID)
    {
        plugin.enabled = true;
        if plugin
            .config
            .get("database_path")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|path| path.trim().is_empty())
        {
            plugin.config = config;
        }
        return Ok(());
    }
    service.plugins.configured.push(ConfiguredPluginSelection {
        id: mutsuki_plugin_resource_sqlite::PLUGIN_ID.into(),
        enabled: true,
        config,
    });
    Ok(())
}

fn apply_lilia_image_render_fonts(service: &mut ServiceConfig, root: &Path) -> Result<(), String> {
    let Some(plugin) = service
        .plugins
        .configured
        .iter_mut()
        .find(|plugin| plugin.id == IMAGE_RENDER_PLUGIN_ID)
    else {
        return Ok(());
    };
    let Some(config) = plugin.config.as_object_mut() else {
        return Ok(());
    };
    if config.get("output_provider_id").is_none() {
        return Ok(());
    }
    let has_fonts = config
        .get("font_files")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|files| !files.is_empty());
    if has_fonts {
        return Ok(());
    }
    let fonts = install_lilia_fonts(root)?;
    config.insert(
        "font_files".into(),
        serde_json::Value::Array(
            fonts
                .into_iter()
                .map(|path| serde_json::Value::String(path.to_string_lossy().into_owned()))
                .collect(),
        ),
    );
    Ok(())
}

fn bundled_lilia_font_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts")
}

fn install_lilia_fonts(root: &Path) -> Result<Vec<PathBuf>, String> {
    let dest = root.join("fonts");
    std::fs::create_dir_all(&dest).map_err(|error| {
        format!(
            "single_instance.lilia_fonts_unavailable: failed to create {}: {error}",
            dest.display()
        )
    })?;
    let source = bundled_lilia_font_dir();
    let mut files = Vec::with_capacity(LILIA_FONT_FILES.len());
    for name in LILIA_FONT_FILES {
        let target = dest.join(name);
        if !target.is_file() {
            let from = source.join(name);
            if !from.is_file() {
                return Err(format!(
                    "single_instance.lilia_font_missing: {}",
                    from.display()
                ));
            }
            std::fs::copy(&from, &target).map_err(|error| {
                format!(
                    "single_instance.lilia_font_copy_failed: {} -> {}: {error}",
                    from.display(),
                    target.display()
                )
            })?;
        }
        files.push(target);
    }
    Ok(files)
}

fn apply_single_instance_boundaries(service: &mut ServiceConfig, root: &Path) {
    // This local product is administered through the loopback Web Console; it does not expose
    // the optional Unix-domain control socket.
    service.ipc.enabled = false;
    service.service.instance_id = "mutsuki-bot".into();
    service.service.home_dir = root.to_path_buf();
    service.service.data_dir = root.join("data");
    service.service.log_dir = root.join("logs");
    service.service.run_dir = root.join("run");
    service.service.plugin_dir = root.join("plugins/installed");
    service.security.secret_file = Some(root.join("secrets.toml"));
    service.plugins.dynamic_dirs = vec![root.join("plugins/installed")];
    service.plugins.disabled_dir = root.join("plugins/disabled");
}

fn console_config(product: &serde_json::Value) -> LocalConsoleConfig {
    let mut extensions = vec!["config".to_string()];
    if product
        .get("workspace_enabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        extensions.extend([
            "qq".into(),
            "agent".into(),
            "bot-flow-editor".into(),
            "sandbox".into(),
        ]);
    }
    LocalConsoleConfig {
        enabled: true,
        listen: "127.0.0.1:8787".into(),
        auth_token_key: Some(CONSOLE_AUTH_TOKEN_KEY.into()),
        extensions,
        release_set: None,
    }
}

fn ensure_local_auth_secret<F>(secret_path: &Path, mut prompt: F) -> Result<(), String>
where
    F: FnMut() -> Result<String, String>,
{
    let environment = match std::env::var(CONSOLE_AUTH_TOKEN_ENV) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(format!(
                "single_instance.console_auth_invalid: {CONSOLE_AUTH_TOKEN_ENV}"
            ));
        }
        Err(std::env::VarError::NotPresent) => None,
    };
    ensure_local_auth_secret_with_environment(secret_path, &mut prompt, environment.as_deref())
}

fn ensure_local_auth_secret_with_environment<F>(
    secret_path: &Path,
    prompt: &mut F,
    environment: Option<&str>,
) -> Result<(), String>
where
    F: FnMut() -> Result<String, String>,
{
    let mut document = if secret_path.exists() {
        let content = std::fs::read_to_string(&secret_path).map_err(|error| error.to_string())?;
        toml::from_str::<toml::Value>(&content).map_err(|error| error.to_string())?
    } else {
        toml::Value::Table(Default::default())
    };
    let root = document
        .as_table_mut()
        .ok_or_else(|| "single_instance.secret_document_invalid".to_string())?;
    let secrets = root
        .entry("secrets")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| "single_instance.secret_table_invalid".to_string())?;
    if secrets
        .get(CONSOLE_AUTH_TOKEN_KEY)
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        ensure_private_permissions(secret_path)?;
        return Ok(());
    }
    match environment {
        Some("") => {
            return Err(format!(
                "single_instance.console_auth_empty: {CONSOLE_AUTH_TOKEN_ENV}"
            ));
        }
        Some(_) => return write_private_toml(secret_path, &document),
        None => {}
    }
    let passphrase = prompt()?;
    if passphrase.is_empty() {
        return Err("single_instance.console_auth_empty".into());
    }
    secrets.insert(
        CONSOLE_AUTH_TOKEN_KEY.into(),
        toml::Value::String(passphrase),
    );
    write_private_toml(secret_path, &document)
}

fn interactive_admin_passphrase() -> Result<String, String> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Err(format!(
            "single_instance.console_auth_required: set {CONSOLE_AUTH_TOKEN_ENV} for non-interactive startup"
        ));
    }
    confirmed_admin_passphrase(|prompt| {
        rpassword::prompt_password(prompt)
            .map_err(|error| format!("single_instance.console_auth_prompt_failed: {error}"))
    })
}

fn confirmed_admin_passphrase<F>(mut read: F) -> Result<String, String>
where
    F: FnMut(&str) -> Result<String, String>,
{
    loop {
        let first = read("设置管理台口令: ")?;
        if first.is_empty() {
            continue;
        }
        let confirmation = read("再次输入管理台口令: ")?;
        if first == confirmation {
            return Ok(first);
        }
    }
}

fn write_private_toml(path: &Path, document: &toml::Value) -> Result<(), String> {
    let content = toml::to_string_pretty(document).map_err(|error| error.to_string())?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("secrets.toml");
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        use std::io::Write as _;
        let mut file = options
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.write_all(content.as_bytes())
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        ensure_private_permissions(path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn ensure_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn ensure_private_permissions(_: &Path) -> Result<(), String> {
    Ok(())
}

fn service_descriptor() -> ConfigDescriptor {
    ConfigDescriptor {
        provider_id: ConfigProviderId::new(SERVICE_CONFIG_PROVIDER_ID),
        schema_version: 1,
        value_version: 1,
        title: LocalizedText::new("服务运行时"),
        description: None,
        scopes: vec![ConfigScope::global()],
        root: ConfigNode {
            key: ConfigKey::new("service"),
            value_type: ConfigValueType::Map {
                key_strategy: mutsuki_config_service::MapKeyStrategy::FreeString,
                value: Box::new(ConfigValueType::Object),
            },
            title: LocalizedText::new("服务运行时"),
            description: None,
            default_value: None,
            constraints: ConfigConstraints::default(),
            presentation: ConfigPresentation::default(),
            visibility: None,
            enabled_if: None,
            mutability: ConfigMutability::ReadWrite,
            restart_policy: RestartPolicy::HostRestart,
            children: Vec::new(),
        },
        groups: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetedPluginReloadLifecycle;
    use mutsuki_agent_service_host_integration::{
        AGENT_CONNECTION_MANAGEMENT_SERVICE_ID, AGENT_CONNECTIONS_PLUGIN_ID,
        AgentConnectionManager, LOCAL_AGENT_API_KEY, LOCAL_AGENT_API_KEY_FIELD,
        LOCAL_AGENT_CONFIG_PROVIDER_ID, LOCAL_AGENT_PLUGIN_ID, LocalAgentConfig,
        local_agent_config_value,
    };
    use mutsuki_bot_flow::BotFlowRegistry;
    use mutsuki_bot_service_host_integration::SANDBOX_SERVICE_ID;
    use mutsuki_config_service::{ConfigApplyRequest, SecretState, SecretValue};
    use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;
    use mutsuki_plugin_bot_event_router::BOT_FLOW_REGISTRY_SERVICE_ID;

    #[test]
    fn executable_path_selects_exactly_one_sibling_instance_directory() {
        assert_eq!(
            single_instance_root(Path::new("/opt/mutsuki/mutsuki-bot")).unwrap(),
            PathBuf::from("/opt/mutsuki/.mutsuki-bot")
        );
    }

    #[test]
    fn empty_image_render_font_files_are_filled_with_lilia_fonts() {
        let root = tempfile::tempdir().unwrap();
        let mut service = ServiceConfig::default();
        service
            .plugins
            .configured
            .push(mutsuki_service_config::ConfiguredPluginSelection {
                id: IMAGE_RENDER_PLUGIN_ID.into(),
                enabled: true,
                config: serde_json::json!({
                    "output_provider_id": "memory",
                    "font_files": []
                }),
            });
        apply_lilia_image_render_fonts(&mut service, root.path()).unwrap();
        let files = service.plugins.configured[0].config["font_files"]
            .as_array()
            .unwrap();
        assert_eq!(files.len(), 3);
        for file in files {
            let path = PathBuf::from(file.as_str().unwrap());
            assert!(path.starts_with(root.path().join("fonts")));
            assert!(path.is_file());
        }
    }

    #[test]
    fn passphrase_confirmation_retries_empty_and_mismatched_input() {
        let mut input = ["", "first", "different", "accepted", "accepted"].into_iter();
        let passphrase = confirmed_admin_passphrase(|_| {
            input
                .next()
                .map(str::to_owned)
                .ok_or_else(|| "unexpected extra prompt".to_string())
        })
        .unwrap();
        assert_eq!(passphrase, "accepted");
    }

    #[test]
    fn passphrase_prompt_error_does_not_create_secret_file() {
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("secrets.toml");
        let error = ensure_local_auth_secret_with_environment(
            &secret,
            &mut || Err("prompt cancelled".into()),
            None,
        )
        .unwrap_err();
        assert_eq!(error, "prompt cancelled");
        assert!(!secret.exists());
    }

    #[test]
    fn noninteractive_secret_requires_a_nonempty_environment_value() {
        let root = tempfile::tempdir().unwrap();
        let secret = root.path().join("secrets.toml");
        let error = ensure_local_auth_secret_with_environment(
            &secret,
            &mut || panic!("environment-backed startup must not prompt"),
            Some(""),
        )
        .unwrap_err();
        assert!(error.contains(CONSOLE_AUTH_TOKEN_ENV));
        assert!(!secret.exists());

        ensure_local_auth_secret_with_environment(
            &secret,
            &mut || panic!("environment-backed startup must not prompt"),
            Some("deployment-secret"),
        )
        .unwrap();
        assert!(secret.is_file());
        assert!(
            !std::fs::read_to_string(secret)
                .unwrap()
                .contains("deployment-secret")
        );
    }

    async fn load_test_product(root: &Path) -> SingleInstanceProduct {
        load_single_instance_product_at(root, || Ok("test-admin-passphrase".into()))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn first_run_creates_single_instance_workspace_and_private_console_secret() {
        let root = tempfile::tempdir().unwrap();
        let product = load_test_product(root.path()).await;
        for id in [
            AGENT_CONNECTIONS_PLUGIN_ID,
            "mutsuki.bot.router.flow",
            SANDBOX_SERVICE_ID,
        ] {
            assert!(
                product
                    .service
                    .plugins
                    .configured
                    .iter()
                    .any(|selection| selection.id == id && selection.enabled),
                "missing enabled workspace selection {id}: {:?}",
                product.service.plugins.configured
            );
        }
        for id in [
            QQBOT_ADAPTER_PLUGIN_ID,
            LOCAL_AGENT_PLUGIN_ID,
            "mutsuki.plugin.bot.agent",
        ] {
            assert!(
                product
                    .service
                    .plugins
                    .configured
                    .iter()
                    .any(|selection| selection.id == id && !selection.enabled),
                "missing disabled owner selection {id}: {:?}",
                product.service.plugins.configured
            );
        }
        assert!(!product.service.ipc.enabled);
        assert_eq!(product.service.service.instance_id, "mutsuki-bot");
        assert_eq!(product.service.service.home_dir, root.path());
        assert_eq!(product.service.service.data_dir, root.path().join("data"));
        assert_eq!(product.service.service.log_dir, root.path().join("logs"));
        assert_eq!(product.service.service.run_dir, root.path().join("run"));
        assert_eq!(product.console.listen, "127.0.0.1:8787");
        assert_eq!(
            product.console.extensions,
            vec!["config", "qq", "agent", "bot-flow-editor", "sandbox"]
        );
        let secret_path = root.path().join("secrets.toml");
        let content = std::fs::read_to_string(&secret_path).unwrap();
        assert!(!content.contains("mutsuki.web.console.token ="));
        let parsed: toml::Value = toml::from_str(&content).unwrap();
        assert_eq!(
            parsed["secrets"][CONSOLE_AUTH_TOKEN_KEY].as_str(),
            Some("test-admin-passphrase")
        );
        assert!(root.path().join("config.sqlite3").is_file());
        for directory in [
            "data",
            "logs",
            "run",
            "plugins/installed",
            "plugins/disabled",
        ] {
            assert!(root.path().join(directory).is_dir(), "missing {directory}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(secret_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }

        drop(product);
        let restored = load_single_instance_product_at(root.path(), || {
            Err("existing secret unexpectedly prompted".into())
        })
        .await
        .unwrap();
        assert_eq!(restored.console.extensions.len(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn owner_config_apply_persists_secret_and_preserves_unrelated_services() {
        let root = tempfile::tempdir().unwrap();
        let product = load_test_product(root.path()).await;
        let runtime = crate::assemble_service_with_connections(
            product.service.clone(),
            product.config.clone(),
            product.agent_connections.clone(),
        )
        .unwrap()
        .start()
        .await
        .unwrap();
        product
            .config
            .set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
                runtime.handle(),
            )));
        let flow_before = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let connections_before = runtime
            .host_service::<AgentConnectionManager>(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
            .unwrap();

        let snapshot = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        let mut local = LocalAgentConfig::default();
        local.endpoint = "http://127.0.0.1:43111/v1".into();
        local.model = "fixture-model".into();
        let mut candidate = local_agent_config_value(false, &local);
        candidate.as_object_mut().unwrap().insert(
            LOCAL_AGENT_API_KEY_FIELD.into(),
            ConfigValue::Secret(SecretState::Set {
                value: SecretValue::new("fixture-api-key"),
            }),
        );
        let applied = product
            .config
            .apply(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snapshot.revision,
                    dry_run: false,
                },
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(
            product
                .service
                .host_secret_store()
                .resolve(LOCAL_AGENT_API_KEY)
                .as_deref(),
            Some("fixture-api-key")
        );
        let stored = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert!(!format!("{stored:?}").contains("fixture-api-key"));
        let flow_after = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let connections_after = runtime
            .host_service::<AgentConnectionManager>(AGENT_CONNECTION_MANAGEMENT_SERVICE_ID)
            .unwrap();
        assert!(Arc::ptr_eq(&flow_before, &flow_after));
        assert!(Arc::ptr_eq(&connections_before, &connections_after));
        runtime.shutdown().await;
        drop(product);

        let restored = load_test_product(root.path()).await;
        let local = restored
            .service
            .plugins
            .configured
            .iter()
            .find(|selection| selection.id == LOCAL_AGENT_PLUGIN_ID)
            .unwrap();
        assert_eq!(local.config["endpoint"], "http://127.0.0.1:43111/v1");
        assert_eq!(
            restored.service.secret(LOCAL_AGENT_API_KEY).as_deref(),
            Some("fixture-api-key")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn failed_agent_preflight_restores_secret_document_and_runtime_generation() {
        let root = tempfile::tempdir().unwrap();
        let product = load_test_product(root.path()).await;
        let runtime = crate::assemble_service_with_connections(
            product.service.clone(),
            product.config.clone(),
            product.agent_connections.clone(),
        )
        .unwrap()
        .start()
        .await
        .unwrap();
        product
            .config
            .set_lifecycle(Arc::new(TargetedPluginReloadLifecycle::new(
                runtime.handle(),
            )));
        let flow_before = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        let snapshot = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        let mut local = LocalAgentConfig::default();
        local.endpoint = "http://127.0.0.1:9/v1".into();
        local.model = "unreachable-model".into();
        let mut candidate = local_agent_config_value(true, &local);
        candidate.as_object_mut().unwrap().insert(
            LOCAL_AGENT_API_KEY_FIELD.into(),
            ConfigValue::Secret(SecretState::Set {
                value: SecretValue::new("must-roll-back"),
            }),
        );
        let error = product
            .config
            .apply(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigApplyRequest {
                    candidate,
                    expected_revision: snapshot.revision,
                    dry_run: false,
                },
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("must-roll-back"));
        assert!(
            product
                .service
                .host_secret_store()
                .resolve(LOCAL_AGENT_API_KEY)
                .is_none()
        );
        assert!(
            !std::fs::read_to_string(root.path().join("secrets.toml"))
                .unwrap()
                .contains("must-roll-back")
        );
        let after = product
            .config
            .read(
                LOCAL_AGENT_CONFIG_PROVIDER_ID,
                ConfigContext::global(),
                &["*".into()],
            )
            .await
            .unwrap();
        assert_eq!(after.revision, snapshot.revision);
        assert_eq!(after.value.to_json()["enabled"], false);
        let flow_after = runtime
            .host_service::<BotFlowRegistry>(BOT_FLOW_REGISTRY_SERVICE_ID)
            .unwrap();
        assert!(Arc::ptr_eq(&flow_before, &flow_after));
        runtime.shutdown().await;
    }
}
