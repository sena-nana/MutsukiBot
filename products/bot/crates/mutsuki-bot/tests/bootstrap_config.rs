// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(clippy::manual_let_else)]

use mutsuki_bot::{
    CONSOLE_AUTH_TOKEN_KEY, PRODUCT_CONFIG_PROVIDER_ID, load_single_instance_product_for_test,
};
use mutsuki_config_service::{
    ConfigApplyRequest, ConfigCompareAndSetRequest, ConfigContext, ConfigDocumentKey, ConfigValue,
    capability,
};
use mutsuki_plugin_bot_adapter_qqbot::QQBOT_ADAPTER_PLUGIN_ID;

const TEST_ADMIN_PASSPHRASE: &str = "test-admin-passphrase";

#[tokio::test]
async fn empty_single_instance_is_seeded_once_and_restored() {
    let root = tempfile::tempdir().unwrap();
    let first = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    let first_product = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    assert_eq!(first_product.revision.0, 1);
    assert_eq!(first_product.schema_version, 3);
    assert_eq!(first_product.value_version, 3);
    assert_eq!(
        first_product.value.to_json()["workspace_enabled"],
        serde_json::Value::Bool(true)
    );
    for id in [
        "mutsuki.agent.connections",
        "mutsuki.bot.router.flow",
        "mutsuki.bot.sandbox",
    ] {
        assert!(
            first
                .service
                .plugins
                .configured
                .iter()
                .any(|selection| selection.id == id && selection.enabled)
        );
    }
    assert_eq!(
        first.console.extensions,
        vec!["config", "qq", "agent", "bot-flow-editor", "sandbox"]
    );
    assert_eq!(
        first.service.secret(CONSOLE_AUTH_TOKEN_KEY).as_deref(),
        Some(TEST_ADMIN_PASSPHRASE)
    );

    drop(first);
    let second = load_single_instance_product_for_test(root.path(), "must-not-replace")
        .await
        .unwrap();
    let second_product = second
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    assert_eq!(second_product.revision, first_product.revision);
    assert_eq!(
        second.service.secret(CONSOLE_AUTH_TOKEN_KEY).as_deref(),
        Some(TEST_ADMIN_PASSPHRASE)
    );
}

#[tokio::test]
async fn legacy_product_document_version_is_rejected_without_migration() {
    let root = tempfile::tempdir().unwrap();
    let first = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let mut write = first
        .config
        .repository()
        .prepare_compare_and_set(ConfigCompareAndSetRequest {
            key: ConfigDocumentKey::new(PRODUCT_CONFIG_PROVIDER_ID, ConfigContext::global()),
            expected_revision: snapshot.revision,
            value: snapshot.value,
            schema_version: 2,
            value_version: 2,
        })
        .unwrap();
    write.commit().unwrap();
    write.finish().unwrap();
    drop(first);

    let error =
        match load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE).await {
            Ok(_) => panic!("legacy product document unexpectedly loaded"),
            Err(error) => error,
        };
    assert!(error.contains("product.config.version_unsupported"));
}

#[tokio::test]
async fn builtin_platform_plugins_boot_with_disabled_owner_documents() {
    let root = tempfile::tempdir().unwrap();
    let product = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    for id in [
        "mutsuki.bot.bilibili",
        "mutsuki.bot.bilibili.workshop",
        "mutsuki.bot.mihuashi",
    ] {
        let snapshot = product
            .config
            .read(
                id,
                ConfigContext::global(),
                &[capability::VALUE_READ.into()],
            )
            .await
            .unwrap();
        assert_eq!(
            snapshot.value.to_json()["enabled"],
            serde_json::Value::Bool(false),
            "{id} must boot disabled"
        );
        assert!(
            product
                .service
                .plugins
                .configured
                .iter()
                .any(|selection| selection.id == id && !selection.enabled),
            "{id} must ship as a disabled configured selection"
        );
    }
}

#[tokio::test]
async fn owner_plugin_ids_are_rejected_from_runtime_plugins() {
    let root = tempfile::tempdir().unwrap();
    let first = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    let snapshot = first
        .config
        .read(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let ConfigValue::Object(mut candidate) = snapshot.value else {
        panic!("product config must be an object");
    };
    candidate
        .get_mut("runtime_plugins")
        .and_then(ConfigValue::as_object_mut)
        .unwrap()
        .insert(
            "mutsuki.bot.adapter.qqbot".into(),
            ConfigValue::from_json(&serde_json::json!({"enabled": true, "config": {}})),
        );
    first
        .config
        .apply(
            PRODUCT_CONFIG_PROVIDER_ID,
            ConfigApplyRequest {
                candidate: ConfigValue::Object(candidate),
                expected_revision: snapshot.revision,
                dry_run: false,
            },
            ConfigContext::global(),
            &[capability::VALUE_WRITE.into(), capability::APPLY.into()],
        )
        .await
        .unwrap();
    drop(first);

    let error =
        match load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE).await {
            Ok(_) => panic!("reserved owner plugin unexpectedly loaded"),
            Err(error) => error,
        };
    assert!(error.contains("不得配置 owner 插件"));
}

#[tokio::test]
async fn legacy_bootstrap_files_are_not_imported() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("local.toml"),
        "[host]\ninstance_id = \"legacy\"\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("local.secret.toml"),
        "[secrets]\nlegacy = \"ignored\"\n",
    )
    .unwrap();

    let product = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    assert_eq!(product.service.service.instance_id, "mutsuki-bot");
    assert_eq!(product.service.service.home_dir, root.path());
    assert!(root.path().join("config.sqlite3").is_file());
    assert_eq!(
        product.service.secret(CONSOLE_AUTH_TOKEN_KEY).as_deref(),
        Some(TEST_ADMIN_PASSPHRASE)
    );
}

#[tokio::test]
async fn seeded_qq_login_document_uses_receive_switches() {
    let root = tempfile::tempdir().unwrap();
    let product = load_single_instance_product_for_test(root.path(), TEST_ADMIN_PASSPHRASE)
        .await
        .unwrap();
    let snapshot = product
        .config
        .read(
            QQBOT_ADAPTER_PLUGIN_ID,
            ConfigContext::global(),
            &[capability::VALUE_READ.into()],
        )
        .await
        .unwrap();
    let value = snapshot.value.to_json();
    assert_eq!(value["enabled"], false);
    assert!(value.get("receive_private_and_group").is_some());
    assert!(value.get("receive_guild").is_some());
    assert!(value.get("gateway_intents").is_none());
    assert!(value.get("shard_index").is_none());
}
