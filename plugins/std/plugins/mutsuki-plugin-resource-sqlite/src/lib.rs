// Pedantic lints below are inherited from the workspace and still fire in this
// package. They are listed explicitly so the remaining debt stays auditable and
// every other pedantic lint keeps failing the build.
#![allow(
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    clippy::similar_names,
    clippy::unnecessary_wraps
)]

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use mutsuki_runtime_contracts::resource::experimental::{CommandBatch, SagaPlan};
use mutsuki_runtime_contracts::{
    CommandPlan, ERR_RESOURCE_GENERATION_MISMATCH, ERR_RESOURCE_NOT_FOUND,
    ERR_RESOURCE_UNSUPPORTED, ExportPlan, PlanReceipt, PluginManifest, ReadPlan, RefId,
    ResourceAccess, ResourceId, ResourceLifetime, ResourceProviderCompatibility,
    ResourceProviderReloadPolicy, ResourceRef, ResourceSealState, ResourceSemantic,
    ResourceTypeDescriptor, RuntimeError, ScalarValue, SnapshotDescriptor, StreamPlan, WritePlan,
};
use mutsuki_runtime_core::{RuntimeFailure, RuntimeResult};
use mutsuki_runtime_sdk::{
    LoadedPlugin, PluginBuilder, ResourcePlanGateway, ResourceProviderGateway,
};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{Value, json};

pub const PLUGIN_ID: &str = "mutsuki.std.resource.sqlite";
pub const PROVIDER_ID: &str = "mutsuki.std.resource.sqlite";

const BLOB_KIND_ID: &str = "mutsuki.resource.sqlite.blob";
const SNAPSHOT_KIND_ID: &str = "mutsuki.resource.sqlite.snapshot";
const CAPABILITY_KIND_ID: &str = "mutsuki.resource.sqlite.capability";

/// Plugin configuration accepted through the ServiceHost configured-plugin
/// document. `database_path` must point to a writable SQLite database file;
/// the parent directory is created on open.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqliteResourceConfig {
    pub database_path: String,
}

impl SqliteResourceConfig {
    /// # Errors
    ///
    /// Returns an error when the database path is empty.
    pub fn validate(&self) -> Result<(), String> {
        if self.database_path.trim().is_empty() {
            return Err("database_path is required".into());
        }
        Ok(())
    }
}

#[derive(Debug)]
struct SqliteResourceState {
    connection: Connection,
    next_slot: u64,
}

#[derive(Debug)]
pub struct SqliteResourceProvider {
    state: Mutex<SqliteResourceState>,
}

impl SqliteResourceProvider {
    /// Opens (or creates) the persistent resource database at `path`.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the database cannot be opened or the
    /// schema cannot be prepared.
    pub fn open(path: &Path) -> RuntimeResult<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
            }
        }
        let connection = Connection::open(path)
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Self::prepare(connection)
    }

    /// Opens a throwaway in-database provider; mainly for tests and default
    /// plugin construction where no product database path is configured.
    ///
    /// # Errors
    ///
    /// Returns a structured failure when the schema cannot be prepared.
    pub fn open_in_memory() -> RuntimeResult<Self> {
        let connection = Connection::open_in_memory()
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Self::prepare(connection)
    }

    fn prepare(connection: Connection) -> RuntimeResult<Self> {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS resources (
                    ref_id TEXT PRIMARY KEY,
                    slot INTEGER NOT NULL UNIQUE,
                    kind_id TEXT NOT NULL,
                    semantic TEXT NOT NULL,
                    schema TEXT NOT NULL,
                    version INTEGER NOT NULL,
                    bytes BLOB NOT NULL
                );",
            )
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        let next_slot = connection
            .query_row("SELECT COALESCE(MAX(slot), 0) FROM resources", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|slot| slot as u64)
            .map_err(|error| storage_failure("resource.sqlite.open", &error.to_string()))?;
        Ok(Self {
            state: Mutex::new(SqliteResourceState {
                connection,
                next_slot,
            }),
        })
    }

    fn lock_state(&self, route: &str) -> RuntimeResult<MutexGuard<'_, SqliteResourceState>> {
        self.state
            .lock()
            .map_err(|_| storage_failure(route, "sqlite provider mutex poisoned"))
    }

    fn create_resource(
        &self,
        kind_id: &str,
        semantic: ResourceSemantic,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        let mut state = self.lock_state("resource.sqlite.create")?;
        state.next_slot += 1;
        let slot = state.next_slot;
        let ref_id = RefId::from(format!("sqlite-resource-{slot}"));
        state
            .connection
            .execute(
                "INSERT INTO resources (ref_id, slot, kind_id, semantic, schema, version, bytes)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                rusqlite::params![
                    ref_id.as_str(),
                    slot as i64,
                    kind_id,
                    semantic_key(&semantic),
                    schema,
                    bytes
                ],
            )
            .map_err(|error| storage_failure("resource.sqlite.create", &error.to_string()))?;
        Ok(resource_ref(
            ref_id.as_str(),
            kind_id,
            semantic,
            schema,
            1,
            Some(bytes.len() as u64),
        ))
    }

    fn with_entry<T>(
        &self,
        resource: &ResourceRef,
        route: &str,
        read: impl FnOnce(&ResourceRef, &[u8]) -> RuntimeResult<T>,
    ) -> RuntimeResult<T> {
        ensure_provider(resource, route)?;
        let state = self.lock_state(route)?;
        let entry = state
            .connection
            .query_row(
                "SELECT kind_id, semantic, schema, version, bytes
                 FROM resources WHERE ref_id = ?1",
                [resource.ref_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? as u64,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                },
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => runtime_failure(
                    ERR_RESOURCE_NOT_FOUND,
                    format!("{route}.{}", resource.ref_id),
                ),
                cause => storage_failure(route, &cause.to_string()),
            })?;
        let (kind_id, semantic, schema, version, bytes) = entry;
        let current = resource_ref(
            resource.ref_id.as_str(),
            &kind_id,
            semantic_from_key(&semantic, route)?,
            &schema,
            version,
            Some(bytes.len() as u64),
        );
        ensure_descriptor_current(resource, &current, route)?;
        read(&current, &bytes)
    }
}

impl ResourcePlanGateway for SqliteResourceProvider {
    fn collect_read_plan(&self, plan: &ReadPlan) -> RuntimeResult<Vec<u8>> {
        match plan.operation.as_str() {
            "collect" | "get" => self.with_entry(
                &plan.resource,
                "resource.sqlite.read",
                |_descriptor, bytes| Ok(bytes.to_vec()),
            ),
            operation => Err(unsupported("resource.sqlite.read", operation)),
        }
    }

    fn snapshot_read_plan(
        &self,
        plan: &ReadPlan,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<SnapshotDescriptor> {
        let (source_ref, source_version, bytes) = self.with_entry(
            &plan.resource,
            "resource.sqlite.snapshot",
            |descriptor, bytes| Ok((descriptor.clone(), descriptor.version, bytes.to_vec())),
        )?;
        let kind_id = if kind_id.is_empty() {
            SNAPSHOT_KIND_ID
        } else {
            kind_id
        };
        let snapshot_ref =
            self.create_resource(kind_id, ResourceSemantic::VersionedSnapshot, schema, bytes)?;
        Ok(SnapshotDescriptor {
            snapshot_ref,
            source_ref,
            source_version,
            snapshot_version: 1,
            is_stale: false,
            is_latest: true,
        })
    }

    fn open_stream_plan(&self, plan: &ReadPlan) -> RuntimeResult<StreamPlan> {
        Err(unsupported("resource.sqlite.stream", &plan.operation))
    }

    fn execute_export_plan(&self, plan: &ExportPlan) -> RuntimeResult<PlanReceipt> {
        if plan.target != "inline_utf8" {
            return Err(unsupported("resource.sqlite.export", &plan.target));
        }
        let (resource_ref, text) = self.with_entry(
            &plan.resource,
            "resource.sqlite.export",
            |descriptor, bytes| {
                let text = std::str::from_utf8(bytes).map_err(|error| {
                    let mut runtime_error = RuntimeError::new(
                        ERR_RESOURCE_UNSUPPORTED,
                        "runtime.resource_provider.sqlite",
                        format!("resource.sqlite.export.{}", plan.resource.ref_id),
                    );
                    runtime_error
                        .evidence
                        .insert("detail".into(), ScalarValue::String(error.to_string()));
                    RuntimeFailure::new(runtime_error)
                })?;
                Ok((descriptor.clone(), text.to_string()))
            },
        )?;
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "exported".into(),
            resource_ref: Some(resource_ref),
            snapshot: None,
            descriptor_updates: Vec::new(),
            new_version: None,
            output: json!(text),
        })
    }

    fn commit_write_plan(&self, plan: &WritePlan, bytes: Vec<u8>) -> RuntimeResult<PlanReceipt> {
        ensure_provider(&plan.resource, "resource.sqlite.write")?;
        let state = self.lock_state("resource.sqlite.write")?;
        let current_version = state
            .connection
            .query_row(
                "SELECT semantic, version FROM resources WHERE ref_id = ?1",
                [plan.resource.ref_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .map_err(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => runtime_failure(
                    ERR_RESOURCE_NOT_FOUND,
                    format!("resource.sqlite.write.{}", plan.resource.ref_id),
                ),
                cause => storage_failure("resource.sqlite.write", &cause.to_string()),
            })?;
        let semantic = semantic_from_key(&current_version.0, "resource.sqlite.write")?;
        if plan.resource.semantic != ResourceSemantic::CowVersionedState
            || semantic != ResourceSemantic::CowVersionedState
            || plan.base_version != current_version.1
            || plan.patch.base_version != current_version.1
        {
            return Err(runtime_failure(
                ERR_RESOURCE_GENERATION_MISMATCH,
                format!("resource.sqlite.write.{}", plan.resource.ref_id),
            ));
        }

        let new_version = current_version.1 + 1;
        state
            .connection
            .execute(
                "UPDATE resources SET version = ?2, bytes = ?3 WHERE ref_id = ?1",
                rusqlite::params![plan.resource.ref_id.as_str(), new_version as i64, bytes],
            )
            .map_err(|error| storage_failure("resource.sqlite.write", &error.to_string()))?;
        let descriptor = resource_ref(
            plan.resource.ref_id.as_str(),
            &plan.resource.resource_kind,
            ResourceSemantic::CowVersionedState,
            &plan.resource.schema,
            new_version,
            Some(bytes.len() as u64),
        );
        Ok(PlanReceipt {
            plan_id: plan.plan_id.clone(),
            status: "committed".into(),
            resource_ref: Some(descriptor.clone()),
            snapshot: None,
            descriptor_updates: vec![descriptor],
            new_version: Some(new_version),
            output: Value::Null,
        })
    }

    fn execute_command_plan(&self, plan: &CommandPlan) -> RuntimeResult<PlanReceipt> {
        let capability = self.with_entry(
            &plan.capability,
            "resource.sqlite.command",
            |descriptor, _| {
                if descriptor.semantic != ResourceSemantic::CapabilityResource {
                    return Err(unsupported(
                        "resource.sqlite.command",
                        "non_capability_resource",
                    ));
                }
                Ok(descriptor.clone())
            },
        )?;
        match plan.operation.as_str() {
            "query" => Ok(PlanReceipt {
                plan_id: plan.plan_id.clone(),
                status: "commanded".into(),
                resource_ref: Some(capability),
                snapshot: None,
                descriptor_updates: Vec::new(),
                new_version: None,
                output: json!({
                    "provider_id": PROVIDER_ID,
                    "operation": plan.operation.clone(),
                    "args": plan.args.clone(),
                    "idempotency_key": plan.idempotency_key.clone(),
                }),
            }),
            "delete" => {
                let target_ref_id = plan
                    .args
                    .get("ref_id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| unsupported("resource.sqlite.command.delete", "missing ref_id"))?
                    .to_string();
                let state = self.lock_state("resource.sqlite.command.delete")?;
                let deleted = state
                    .connection
                    .execute(
                        "DELETE FROM resources WHERE ref_id = ?1",
                        [target_ref_id.as_str()],
                    )
                    .map_err(|error| {
                        storage_failure("resource.sqlite.command.delete", &error.to_string())
                    })?;
                if deleted == 0 {
                    return Err(runtime_failure(
                        ERR_RESOURCE_NOT_FOUND,
                        format!("resource.sqlite.command.delete.{target_ref_id}"),
                    ));
                }
                Ok(PlanReceipt {
                    plan_id: plan.plan_id.clone(),
                    status: "deleted".into(),
                    resource_ref: Some(capability),
                    snapshot: None,
                    descriptor_updates: Vec::new(),
                    new_version: None,
                    output: json!({ "deleted_ref_id": target_ref_id }),
                })
            }
            operation => Err(unsupported("resource.sqlite.command", operation)),
        }
    }

    fn execute_command_batch(&self, batch: &CommandBatch) -> RuntimeResult<Vec<PlanReceipt>> {
        if batch.rollback_guarantee {
            return Err(unsupported(
                "resource.sqlite.command_batch",
                "rollback_guarantee",
            ));
        }
        batch
            .commands
            .iter()
            .map(|command| self.execute_command_plan(command))
            .collect()
    }

    fn execute_saga_plan(&self, saga: &SagaPlan) -> RuntimeResult<Vec<PlanReceipt>> {
        let mut receipts = Vec::new();
        for command in &saga.steps {
            match self.execute_command_plan(command) {
                Ok(receipt) => receipts.push(receipt),
                Err(cause) => {
                    for compensation in saga.compensations.iter().rev() {
                        let _ = self.execute_command_plan(compensation);
                    }
                    let mut runtime_error = RuntimeError::new(
                        "resource.saga_failed",
                        "runtime.resource_provider.sqlite",
                        format!("resource.sqlite.saga.{}", saga.saga_id),
                    );
                    runtime_error.cause = Some(Box::new(cause.error().clone()));
                    return Err(RuntimeFailure::new(runtime_error));
                }
            }
        }
        Ok(receipts)
    }
}

impl ResourceProviderGateway for SqliteResourceProvider {
    fn create_blob_resource(&self, schema: &str, bytes: Vec<u8>) -> RuntimeResult<ResourceRef> {
        self.create_resource(BLOB_KIND_ID, ResourceSemantic::FrozenValue, schema, bytes)
    }

    fn create_cow_state_resource(
        &self,
        kind_id: &str,
        schema: &str,
        bytes: Vec<u8>,
    ) -> RuntimeResult<ResourceRef> {
        self.create_resource(kind_id, ResourceSemantic::CowVersionedState, schema, bytes)
    }

    fn create_capability_resource(
        &self,
        kind_id: &str,
        schema: &str,
    ) -> RuntimeResult<ResourceRef> {
        let kind_id = if kind_id.is_empty() {
            CAPABILITY_KIND_ID
        } else {
            kind_id
        };
        self.create_resource(
            kind_id,
            ResourceSemantic::CapabilityResource,
            schema,
            Vec::new(),
        )
    }
}

/// Manifest-only identity; the ServiceHost factory supplies the file-backed
/// provider through [`loaded_plugin_with_provider`].
pub fn manifest() -> PluginManifest {
    base_builder().build().manifest
}

pub fn loaded_plugin_with_provider(provider: SqliteResourceProvider) -> LoadedPlugin {
    base_builder()
        .resource_provider_gateway(PROVIDER_ID, Arc::new(provider))
        .build()
}

fn base_builder() -> PluginBuilder {
    PluginBuilder::new(PLUGIN_ID)
        .resource_provider(PROVIDER_ID)
        .resource_type_descriptor(resource_type(
            BLOB_KIND_ID,
            ResourceSemantic::FrozenValue,
            "mutsuki.resource.sqlite.blob.v1",
            &["collect", "get", "snapshot", "export"],
        ))
        .resource_type_descriptor(resource_type(
            SNAPSHOT_KIND_ID,
            ResourceSemantic::VersionedSnapshot,
            "mutsuki.resource.sqlite.snapshot.v1",
            &["collect", "get", "export"],
        ))
        .resource_type_descriptor(resource_type(
            CAPABILITY_KIND_ID,
            ResourceSemantic::CapabilityResource,
            "mutsuki.resource.sqlite.capability.v1",
            &["query", "delete"],
        ))
}

fn resource_type(
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    operations: &[&str],
) -> ResourceTypeDescriptor {
    ResourceTypeDescriptor {
        kind_id: kind_id.into(),
        semantic,
        schema: schema.into(),
        provider_id: PROVIDER_ID.into(),
        operations: operations
            .iter()
            .map(|operation| (*operation).into())
            .collect(),
        reload_policy: ResourceProviderReloadPolicy::CompatibleWithoutLeases,
        compatibility: ResourceProviderCompatibility {
            schema_version: "1.0.0".into(),
            required_operations: operations
                .iter()
                .map(|operation| (*operation).into())
                .collect(),
            preserves_resource_type_id: true,
            accepts_older_generations: false,
            lease_drain_required: false,
        },
    }
}

fn resource_ref(
    ref_id: &str,
    kind_id: &str,
    semantic: ResourceSemantic,
    schema: &str,
    version: u64,
    size_hint: Option<u64>,
) -> ResourceRef {
    ResourceRef {
        ref_id: ref_id.into(),
        resource_id: ResourceId {
            kind_id: kind_id.into(),
            slot_id: ref_id.into(),
            generation: 1,
            version,
        },
        semantic,
        provider_id: PROVIDER_ID.into(),
        resource_kind: kind_id.into(),
        schema: schema.into(),
        version,
        generation: 1,
        access: ResourceAccess::ProviderRpc {
            provider_id: PROVIDER_ID.into(),
            method: "sqlite".into(),
        },
        size_hint,
        content_hash: None,
        lifetime: ResourceLifetime::Persistent,
        lease: None,
        seal_state: ResourceSealState::Sealed,
    }
}

fn semantic_key(semantic: &ResourceSemantic) -> &'static str {
    match semantic {
        ResourceSemantic::FrozenValue => "frozen_value",
        ResourceSemantic::CowVersionedState => "cow_versioned_state",
        ResourceSemantic::VersionedSnapshot => "versioned_snapshot",
        ResourceSemantic::CapabilityResource => "capability_resource",
        ResourceSemantic::ReadOnlyFact => "read_only_fact",
        ResourceSemantic::StreamResource => "stream_resource",
        ResourceSemantic::TransactionResource => "transaction_resource",
    }
}

fn semantic_from_key(key: &str, route: &str) -> RuntimeResult<ResourceSemantic> {
    match key {
        "frozen_value" => Ok(ResourceSemantic::FrozenValue),
        "cow_versioned_state" => Ok(ResourceSemantic::CowVersionedState),
        "versioned_snapshot" => Ok(ResourceSemantic::VersionedSnapshot),
        "capability_resource" => Ok(ResourceSemantic::CapabilityResource),
        "read_only_fact" => Ok(ResourceSemantic::ReadOnlyFact),
        "stream_resource" => Ok(ResourceSemantic::StreamResource),
        "transaction_resource" => Ok(ResourceSemantic::TransactionResource),
        other => Err(storage_failure(
            route,
            &format!("unknown resource semantic: {other}"),
        )),
    }
}

fn ensure_provider(resource: &ResourceRef, route: &str) -> RuntimeResult<()> {
    if resource.provider_id != PROVIDER_ID {
        return Err(unsupported(route, &resource.provider_id));
    }
    Ok(())
}

fn ensure_descriptor_current(
    requested: &ResourceRef,
    current: &ResourceRef,
    route: &str,
) -> RuntimeResult<()> {
    if requested.generation != current.generation
        || requested.resource_id.generation != requested.generation
        || requested.version != current.version
        || requested.resource_id.version != requested.version
    {
        return Err(runtime_failure(
            ERR_RESOURCE_GENERATION_MISMATCH,
            format!("{route}.{}", requested.ref_id),
        ));
    }
    Ok(())
}

fn unsupported(route: &str, detail: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        ERR_RESOURCE_UNSUPPORTED,
        "runtime.resource_provider.sqlite",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

fn runtime_failure(code: &str, route: String) -> RuntimeFailure {
    RuntimeFailure::new(RuntimeError::new(
        code,
        "runtime.resource_provider.sqlite",
        route,
    ))
}

fn storage_failure(route: &str, detail: &str) -> RuntimeFailure {
    let mut error = RuntimeError::new(
        "resource.storage_failed",
        "runtime.resource_provider.sqlite",
        route,
    );
    error
        .evidence
        .insert("detail".into(), ScalarValue::String(detail.into()));
    RuntimeFailure::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mutsuki_runtime_contracts::PatchDescriptor;

    #[test]
    fn blob_collect_and_inline_utf8_export_work() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "read:1".into(),
            resource: blob.clone(),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&read).unwrap(), b"hello");

        let export = ExportPlan {
            plan_id: "export:1".into(),
            resource: blob,
            target: "inline_utf8".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.execute_export_plan(&export).unwrap().output,
            json!("hello")
        );
    }

    #[test]
    fn cow_commit_updates_version_and_rejects_stale_plans() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let state = provider
            .create_cow_state_resource("text_buffer", "text.state.v1", b"old".to_vec())
            .unwrap();
        let write = write_plan("write:1", state);
        let receipt = provider.commit_write_plan(&write, b"new".to_vec()).unwrap();
        assert_eq!(receipt.new_version, Some(2));

        let stale = provider
            .commit_write_plan(&write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(stale.error().code, ERR_RESOURCE_GENERATION_MISMATCH);
    }

    #[test]
    fn snapshot_returns_usable_snapshot_descriptor() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"hello".to_vec())
            .unwrap();
        let read = ReadPlan {
            plan_id: "snapshot:1".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
        let snapshot = provider
            .snapshot_read_plan(&read, "text_snapshot", "text.snapshot.v1")
            .unwrap();
        assert_eq!(
            snapshot.snapshot_ref.semantic,
            ResourceSemantic::VersionedSnapshot
        );
        let snapshot_read = ReadPlan {
            plan_id: "read:snapshot".into(),
            resource: snapshot.snapshot_ref,
            operation: "get".into(),
            args: Value::Null,
        };
        assert_eq!(
            provider.collect_read_plan(&snapshot_read).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn capability_query_batch_and_saga_paths_are_deterministic() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let command = CommandPlan {
            plan_id: "command:1".into(),
            capability: capability.clone(),
            operation: "query".into(),
            args: json!({"key": "value"}),
            idempotency_key: Some("query:1".into()),
        };
        assert_eq!(
            provider.execute_command_plan(&command).unwrap().output["provider_id"],
            PROVIDER_ID
        );
        assert_eq!(
            provider
                .execute_command_batch(&CommandBatch {
                    batch_id: "batch:1".into(),
                    commands: vec![command.clone()],
                    rollback_guarantee: false,
                })
                .unwrap()
                .len(),
            1
        );
        let rollback = provider
            .execute_command_batch(&CommandBatch {
                batch_id: "batch:rollback".into(),
                commands: vec![command.clone()],
                rollback_guarantee: true,
            })
            .unwrap_err();
        assert_eq!(rollback.error().code, ERR_RESOURCE_UNSUPPORTED);

        let mut failing = command.clone();
        failing.operation = "missing".into();
        let saga = provider.execute_saga_plan(&SagaPlan {
            saga_id: "saga:1".into(),
            steps: vec![failing],
            compensations: vec![command],
        });
        let error = saga.unwrap_err();
        assert_eq!(error.error().code, "resource.saga_failed");
        assert!(error.error().cause.is_some());
    }

    #[test]
    fn delete_command_removes_resource_and_fails_structurally_afterwards() {
        let provider = SqliteResourceProvider::open_in_memory().unwrap();
        let capability = provider
            .create_capability_resource("sqlite_query", "sqlite.query.v1")
            .unwrap();
        let blob = provider
            .create_blob_resource("text.v1", b"doomed".to_vec())
            .unwrap();
        let delete = CommandPlan {
            plan_id: "command:delete".into(),
            capability: capability.clone(),
            operation: "delete".into(),
            args: json!({"ref_id": blob.ref_id}),
            idempotency_key: Some("delete:1".into()),
        };
        let receipt = provider.execute_command_plan(&delete).unwrap();
        assert_eq!(receipt.status, "deleted");

        let read = ReadPlan {
            plan_id: "read:deleted".into(),
            resource: blob,
            operation: "collect".into(),
            args: Value::Null,
        };
        let error = provider.collect_read_plan(&read).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_NOT_FOUND);

        let mut missing = delete.clone();
        missing.args = json!({"ref_id": "sqlite-resource-9999"});
        let error = provider.execute_command_plan(&missing).unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_NOT_FOUND);
    }

    #[test]
    fn resources_persist_across_reopen() {
        let file = tempfile::tempdir()
            .unwrap()
            .into_path()
            .join("resources.db");
        let stale_write;
        {
            let provider = SqliteResourceProvider::open(&file).unwrap();
            let blob = provider
                .create_blob_resource("text.v1", b"persisted".to_vec())
                .unwrap();
            let state = provider
                .create_cow_state_resource("text_buffer", "text.state.v1", b"v1".to_vec())
                .unwrap();
            provider
                .commit_write_plan(&write_plan("write:1", state.clone()), b"v2".to_vec())
                .unwrap();
            stale_write = write_plan("write:stale", state);
            let read = ReadPlan {
                plan_id: "read:before".into(),
                resource: blob,
                operation: "collect".into(),
                args: Value::Null,
            };
            assert_eq!(provider.collect_read_plan(&read).unwrap(), b"persisted");
        }

        let provider = SqliteResourceProvider::open(&file).unwrap();
        let blob = ReadPlan {
            plan_id: "read:after".into(),
            resource: resource_ref(
                "sqlite-resource-1",
                BLOB_KIND_ID,
                ResourceSemantic::FrozenValue,
                "text.v1",
                1,
                None,
            ),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&blob).unwrap(), b"persisted");

        let committed = ReadPlan {
            plan_id: "read:after:state".into(),
            resource: resource_ref(
                "sqlite-resource-2",
                "text_buffer",
                ResourceSemantic::CowVersionedState,
                "text.state.v1",
                2,
                None,
            ),
            operation: "collect".into(),
            args: Value::Null,
        };
        assert_eq!(provider.collect_read_plan(&committed).unwrap(), b"v2");

        let error = provider
            .commit_write_plan(&stale_write, b"stale".to_vec())
            .unwrap_err();
        assert_eq!(error.error().code, ERR_RESOURCE_GENERATION_MISMATCH);

        let created = provider
            .create_blob_resource("text.v1", b"after".to_vec())
            .unwrap();
        assert_eq!(created.ref_id.as_str(), "sqlite-resource-3");
    }

    #[test]
    fn config_validation_requires_database_path() {
        let config = SqliteResourceConfig {
            database_path: "  ".into(),
        };
        assert!(config.validate().unwrap_err().contains("database_path"));
        let config = SqliteResourceConfig {
            database_path: "/tmp/resources.db".into(),
        };
        assert!(config.validate().is_ok());
    }

    fn write_plan(plan_id: &str, resource: ResourceRef) -> WritePlan {
        WritePlan {
            plan_id: plan_id.into(),
            resource: resource.clone(),
            base_version: resource.version,
            conflict_policy: "replace".into(),
            patch: PatchDescriptor {
                patch_id: format!("patch:{plan_id}"),
                target_ref: resource.clone(),
                base_version: resource.version,
                conflict_policy: "replace".into(),
                operations: json!({"replace": true}),
            },
            returning: None,
        }
    }
}
