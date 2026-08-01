use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{payload_hash, RuntimeTunnelCommand};

use crate::{
    model::{
        AgentView, AuthContext, ConfigurationOverrideAction, ConfigurationPresetOverrideRecord,
        ConfigurationPresetPreviewView, ConfigurationPresetView, ConfigurationReadinessView,
        ConfigurationRuntimeSyncView, ConfigurationSourceChangeView,
        ConfigurationSourceOverridePreviewView, ConfigurationSourceView,
        CreateConfigurationPresetRequest, NetworkAdapterDefinitionView,
        PreviewConfigurationPresetRequest, PreviewConfigurationSourceOverrideRequest,
        ResolvedOspfCommandSource, UpsertNetworkAdapterDefinitionRequest, CONFIGURATION_BEHAVIORS,
    },
    repository::Repository,
    repository_key_lifecycle::lock_postgres_agent_identity_lifecycle,
    unix_now,
};

const MAX_ARGV_ITEMS: usize = 32;
const MAX_ARG_BYTES: usize = 4096;

struct SystemConfigurationPreset {
    id: &'static str,
    behavior: &'static str,
    name: &'static str,
    is_default: bool,
    description: &'static str,
    definition: Value,
}

fn system_configuration_presets() -> Vec<SystemConfigurationPreset> {
    vec![
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000001",
            behavior: "host_metrics",
            name: "Linux host metrics",
            is_default: true,
            description: "Collect host metrics from the standard Linux procfs and sysfs paths.",
            definition: serde_json::json!({
                "source": "linux_procfs",
                "proc_root": "/proc",
                "sys_class_net_dir": "/sys/class/net",
                "hostname_file": "/etc/hostname",
                "os_release_file": "/etc/os-release"
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000011",
            behavior: "host_metrics",
            name: "Host-mounted Linux metrics",
            is_default: false,
            description: "Collect metrics from host files mounted beneath /host.",
            definition: serde_json::json!({
                "source": "linux_procfs",
                "proc_root": "/host/proc",
                "sys_class_net_dir": "/host/sys/class/net",
                "hostname_file": "/host/etc/hostname",
                "os_release_file": "/host/etc/os-release"
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000002",
            behavior: "tunnel_traffic",
            name: "Interface traffic counters",
            is_default: true,
            description: "Use Linux interface counters for tunnel traffic accounting.",
            definition: serde_json::json!({"source": "interface_counters"}),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000021",
            behavior: "tunnel_traffic",
            name: "vnStat traffic counters",
            is_default: false,
            description: "Use the JSON output from /usr/bin/vnstat.",
            definition: serde_json::json!({
                "source": "vnstat",
                "vnstat_argv": ["/usr/bin/vnstat"]
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000003",
            behavior: "latency_probe",
            name: "Linux latency probe",
            is_default: true,
            description: "Use the agent's bounded Linux ping command candidates.",
            definition: serde_json::json!({"source": "linux_ping_preset"}),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000031",
            behavior: "latency_probe",
            name: "/usr/bin/ping latency probe",
            is_default: false,
            description: "Use the explicitly pinned /usr/bin/ping executable.",
            definition: serde_json::json!({
                "source": "configured_ping_argv",
                "probe_ping_argv": ["/usr/bin/ping"]
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000004",
            behavior: "ospf_update_command",
            name: "Unconfigured OSPF updater",
            is_default: true,
            description:
                "Do not run OSPF status or update commands unless an operator assigns a configured preset or a tunnel plan overrides it.",
            definition: serde_json::json!({
                "contract_version": 1,
                "status_command": null,
                "update_command": null
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000005",
            behavior: "process_inventory",
            name: "Linux process inventory",
            is_default: true,
            description: "Read process inventory from /proc.",
            definition: serde_json::json!({"source": "linux_procfs", "proc_root": "/proc"}),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000051",
            behavior: "process_inventory",
            name: "Host-mounted process inventory",
            is_default: false,
            description: "Read process inventory from /host/proc.",
            definition: serde_json::json!({"source": "linux_procfs", "proc_root": "/host/proc"}),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000006",
            behavior: "user_sessions",
            name: "Linux user sessions",
            is_default: true,
            description: "Use the agent's bounded Linux w/who command candidates.",
            definition: serde_json::json!({"source": "linux_w_who_preset"}),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000061",
            behavior: "user_sessions",
            name: "/usr/bin/w user sessions",
            is_default: false,
            description: "Read sessions with the explicitly pinned /usr/bin/w executable.",
            definition: serde_json::json!({
                "source": "custom_command",
                "user_sessions_command": {
                    "argv": ["/usr/bin/w", "-h"],
                    "max_timeout_secs": 5,
                    "max_output_bytes": 16384
                }
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000062",
            behavior: "user_sessions",
            name: "/usr/bin/who user sessions",
            is_default: false,
            description: "Read sessions with the explicitly pinned /usr/bin/who executable.",
            definition: serde_json::json!({
                "source": "custom_command",
                "user_sessions_command": {
                    "argv": ["/usr/bin/who"],
                    "max_timeout_secs": 5,
                    "max_output_bytes": 16384
                }
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000007",
            behavior: "command_execution",
            name: "Standard command execution",
            is_default: true,
            description:
                "Run shell scripts with the standard Linux shell and inherited environment.",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/sh", "-lc"],
                "working_directory": null,
                "environment_policy": "inherit",
                "environment_keep": [],
                "environment_set": {},
                "pty_policy": "native_pty",
                "process_cleanup": "process_group"
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000071",
            behavior: "command_execution",
            name: "BusyBox command execution",
            is_default: false,
            description: "Run shell scripts with BusyBox ash and a minimal environment.",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/ash", "-lc"],
                "working_directory": null,
                "environment_policy": "minimal_path",
                "environment_keep": ["TERM"],
                "environment_set": {},
                "pty_policy": "native_pty",
                "process_cleanup": "process_group"
            }),
        },
        SystemConfigurationPreset {
            id: "00000000-0000-4000-8000-000000000072",
            behavior: "command_execution",
            name: "Clean batch execution",
            is_default: false,
            description: "Run non-interactive shell scripts with a clean environment.",
            definition: serde_json::json!({
                "shell_script_argv": ["/bin/sh", "-lc"],
                "working_directory": null,
                "environment_policy": "clean",
                "environment_keep": ["PATH", "HOME", "LANG", "LC_ALL"],
                "environment_set": {},
                "pty_policy": "disabled",
                "process_cleanup": "process_group"
            }),
        },
    ]
}

impl Repository {
    pub(crate) async fn initialize_system_configuration_presets(&self) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut seeded = memory.configuration_presets_seeded.write().await;
                if *seeded {
                    return Ok(());
                }
                let now = unix_now().to_string();
                let mut presets = memory.configuration_presets.write().await;
                for system in system_configuration_presets() {
                    let id = Uuid::parse_str(system.id)?;
                    validate_configuration_preset_definition(system.behavior, &system.definition)?;
                    if presets.iter().any(|preset| preset.id == id) {
                        continue;
                    }
                    presets.push(ConfigurationPresetView {
                        id,
                        behavior: system.behavior.to_string(),
                        name: system.name.to_string(),
                        kind: "system".to_string(),
                        is_default: system.is_default,
                        description: Some(system.description.to_string()),
                        definition: system.definition,
                        effective_vps_count: 0,
                        override_vps_count: 0,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    });
                }
                *seeded = true;
                Ok(())
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for system in system_configuration_presets()
                    .into_iter()
                    .filter(|preset| preset.is_default)
                {
                    sqlx::query(
                        r#"
                        UPDATE configuration_presets
                        SET is_default = FALSE, updated_at = now()
                        WHERE behavior = $1
                          AND is_default
                          AND id <> $2
                        "#,
                    )
                    .bind(system.behavior)
                    .bind(Uuid::parse_str(system.id)?)
                    .execute(&mut *tx)
                    .await?;
                }
                for system in system_configuration_presets() {
                    validate_configuration_preset_definition(system.behavior, &system.definition)?;
                    sqlx::query(
                        r#"
                        INSERT INTO configuration_presets (
                            id, behavior, name, kind, is_default, description, definition
                        )
                        VALUES ($1, $2, $3, 'system', $4, $5, $6)
                        ON CONFLICT (id) DO UPDATE SET
                            behavior = EXCLUDED.behavior,
                            name = EXCLUDED.name,
                            kind = 'system',
                            is_default = EXCLUDED.is_default,
                            description = EXCLUDED.description,
                            definition = EXCLUDED.definition,
                            updated_at = now()
                        WHERE (
                            configuration_presets.behavior,
                            configuration_presets.name,
                            configuration_presets.kind,
                            configuration_presets.is_default,
                            configuration_presets.description,
                            configuration_presets.definition
                        ) IS DISTINCT FROM (
                            EXCLUDED.behavior,
                            EXCLUDED.name,
                            'system',
                            EXCLUDED.is_default,
                            EXCLUDED.description,
                            EXCLUDED.definition
                        )
                        "#,
                    )
                    .bind(Uuid::parse_str(system.id)?)
                    .bind(system.behavior)
                    .bind(system.name)
                    .bind(system.is_default)
                    .bind(system.description)
                    .bind(sqlx::types::Json(system.definition))
                    .execute(&mut *tx)
                    .await?;
                }
                let defaults: i64 = sqlx::query_scalar(
                    "SELECT count(*)::bigint FROM configuration_presets WHERE is_default",
                )
                .fetch_one(&mut *tx)
                .await?;
                anyhow::ensure!(
                    defaults == CONFIGURATION_BEHAVIORS.len() as i64,
                    "configuration_preset_default_catalog_incomplete"
                );
                tx.commit().await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_configuration_presets(
        &self,
        behavior: Option<&str>,
    ) -> Result<Vec<ConfigurationPresetView>> {
        if matches!(self, Self::Memory(_)) {
            self.initialize_system_configuration_presets().await?;
        }
        let mut presets = match self {
            Self::Memory(memory) => {
                let agents = self.list_agents().await?;
                let overrides = memory.configuration_preset_overrides.read().await.clone();
                let mut rows = memory
                    .configuration_presets
                    .read()
                    .await
                    .iter()
                    .filter(|preset| behavior.is_none_or(|value| preset.behavior == value))
                    .cloned()
                    .collect::<Vec<_>>();
                for preset in &mut rows {
                    preset.override_vps_count = overrides
                        .iter()
                        .filter(|entry| entry.preset_id == preset.id)
                        .count() as i64;
                    preset.effective_vps_count = preset.override_vps_count;
                    if preset.is_default {
                        preset.effective_vps_count += agents
                            .iter()
                            .filter(|agent| {
                                !overrides.iter().any(|entry| {
                                    entry.client_id == agent.id && entry.behavior == preset.behavior
                                })
                            })
                            .count() as i64;
                    }
                }
                rows
            }
            Self::Postgres(pool) => sqlx::query(
                r#"
                WITH visible_clients AS (
                    SELECT id
                    FROM clients
                    WHERE hidden_at IS NULL
                      AND status NOT IN ('deleted', 'revoked')
                )
                SELECT
                    preset.id,
                    preset.behavior,
                    preset.name,
                    preset.kind,
                    preset.is_default,
                    preset.description,
                    preset.definition,
                    preset.created_at::text AS created_at,
                    preset.updated_at::text AS updated_at,
                    (
                        SELECT count(*)::bigint
                        FROM client_configuration_preset_overrides selected
                        JOIN visible_clients client ON client.id = selected.client_id
                        WHERE selected.preset_id = preset.id
                    ) AS override_vps_count,
                    (
                        SELECT count(*)::bigint
                        FROM client_configuration_preset_overrides selected
                        JOIN visible_clients client ON client.id = selected.client_id
                        WHERE selected.preset_id = preset.id
                    ) + CASE WHEN preset.is_default THEN (
                        SELECT count(*)::bigint
                        FROM visible_clients client
                        WHERE NOT EXISTS (
                            SELECT 1
                            FROM client_configuration_preset_overrides selected
                            WHERE selected.client_id = client.id
                              AND selected.behavior = preset.behavior
                        )
                    ) ELSE 0 END AS effective_vps_count
                FROM configuration_presets preset
                WHERE $1::text IS NULL OR preset.behavior = $1
                ORDER BY preset.behavior, preset.is_default DESC, preset.kind, preset.name
                "#,
            )
            .bind(behavior)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(configuration_preset_from_row)
            .collect::<Result<Vec<_>>>()?,
        };
        presets.sort_by(|left, right| {
            left.behavior
                .cmp(&right.behavior)
                .then_with(|| right.is_default.cmp(&left.is_default))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(presets)
    }

    async fn configuration_preset_catalog(
        &self,
        behavior: Option<&str>,
    ) -> Result<Vec<ConfigurationPresetView>> {
        if matches!(self, Self::Memory(_)) {
            self.initialize_system_configuration_presets().await?;
        }
        let mut presets = match self {
            Self::Memory(memory) => memory
                .configuration_presets
                .read()
                .await
                .iter()
                .filter(|preset| behavior.is_none_or(|value| preset.behavior == value))
                .cloned()
                .collect::<Vec<_>>(),
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT
                    id,
                    behavior,
                    name,
                    kind,
                    is_default,
                    description,
                    definition,
                    created_at::text AS created_at,
                    updated_at::text AS updated_at,
                    0::bigint AS effective_vps_count,
                    0::bigint AS override_vps_count
                FROM configuration_presets
                WHERE $1::text IS NULL OR behavior = $1
                ORDER BY behavior, is_default DESC, kind, name
                "#,
            )
            .bind(behavior)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(configuration_preset_from_row)
            .collect::<Result<Vec<_>>>()?,
        };
        presets.sort_by(|left, right| {
            left.behavior
                .cmp(&right.behavior)
                .then_with(|| right.is_default.cmp(&left.is_default))
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(presets)
    }

    pub(crate) async fn configuration_preset_by_id(
        &self,
        preset_id: Uuid,
    ) -> Result<Option<ConfigurationPresetView>> {
        Ok(self
            .list_configuration_presets(None)
            .await?
            .into_iter()
            .find(|preset| preset.id == preset_id))
    }

    pub(crate) async fn create_configuration_preset(
        &self,
        request: &CreateConfigurationPresetRequest,
        operator: &AuthContext,
    ) -> Result<ConfigurationPresetView> {
        validate_configuration_preset_request(
            &request.behavior,
            &request.name,
            request.description.as_deref(),
            &request.definition,
        )?;
        let now = unix_now().to_string();
        let preset = match self {
            Self::Memory(memory) => {
                self.initialize_system_configuration_presets().await?;
                let mut presets = memory.configuration_presets.write().await;
                anyhow::ensure!(
                    !presets.iter().any(|preset| {
                        preset.behavior == request.behavior
                            && preset.name.eq_ignore_ascii_case(request.name.trim())
                    }),
                    "configuration_preset_duplicate"
                );
                let preset = ConfigurationPresetView {
                    id: Uuid::new_v4(),
                    behavior: request.behavior.clone(),
                    name: request.name.trim().to_string(),
                    kind: "custom".to_string(),
                    is_default: false,
                    description: normalized_description(request.description.as_deref()),
                    definition: request.definition.clone(),
                    effective_vps_count: 0,
                    override_vps_count: 0,
                    created_at: now.clone(),
                    updated_at: now,
                };
                presets.push(preset.clone());
                preset
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO configuration_presets (
                        id, behavior, name, kind, is_default, description, definition
                    )
                    VALUES ($1, $2, $3, 'custom', FALSE, $4, $5)
                    RETURNING
                        id, behavior, name, kind, is_default, description, definition,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        0::bigint AS effective_vps_count,
                        0::bigint AS override_vps_count
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(&request.behavior)
                .bind(request.name.trim())
                .bind(normalized_description(request.description.as_deref()))
                .bind(sqlx::types::Json(&request.definition))
                .fetch_one(&mut *tx)
                .await
                .map_err(configuration_preset_database_error)?;
                let preset = configuration_preset_from_row(row)?;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "configuration_preset.created",
                    &format!("configuration_preset:{}", preset.id),
                    configuration_preset_audit_metadata(&preset, &[]),
                    operator,
                )
                .await?;
                tx.commit().await?;
                return Ok(preset);
            }
        };
        self.record_configuration_preset_audit(
            "configuration_preset.created",
            &preset,
            &[],
            operator,
        )
        .await?;
        Ok(preset)
    }

    pub(crate) async fn clone_configuration_preset(
        &self,
        source_id: Uuid,
        name: &str,
        description: Option<&str>,
        operator: &AuthContext,
    ) -> Result<ConfigurationPresetView> {
        let source = self
            .configuration_preset_by_id(source_id)
            .await?
            .with_context(|| format!("configuration_preset_not_found:{source_id}"))?;
        self.create_configuration_preset(
            &CreateConfigurationPresetRequest {
                behavior: source.behavior,
                name: name.to_string(),
                description: normalized_description(description),
                definition: source.definition,
            },
            operator,
        )
        .await
    }

    pub(crate) async fn preview_configuration_preset_update(
        &self,
        preset_id: Uuid,
        request: &PreviewConfigurationPresetRequest,
    ) -> Result<ConfigurationPresetPreviewView> {
        let preset = self
            .configuration_preset_by_id(preset_id)
            .await?
            .with_context(|| format!("configuration_preset_not_found:{preset_id}"))?;
        anyhow::ensure!(
            preset.kind == "custom",
            "configuration_preset_system_immutable"
        );
        validate_configuration_preset_request(
            &preset.behavior,
            &preset.name,
            request.description.as_deref(),
            &request.definition,
        )?;
        let affected_client_ids = if preset.definition != request.definition {
            self.configuration_preset_override_client_ids(preset.id)
                .await?
        } else {
            Vec::new()
        };
        let rendered =
            render_configuration_preset_definition(&preset.behavior, &request.definition)?;
        let candidate_description = normalized_description(request.description.as_deref());
        let mut changed_keys = changed_definition_keys(&preset.definition, &request.definition);
        if normalized_description(preset.description.as_deref()) != candidate_description {
            changed_keys.push("description".to_string());
            changed_keys.sort();
        }
        let hash_payload = serde_json::json!({
            "action": "configuration_preset.update",
            "preset_id": preset.id,
            "behavior": preset.behavior,
            "current_updated_at": preset.updated_at,
            "current_description": preset.description,
            "current_definition": preset.definition,
            "description": candidate_description,
            "definition": request.definition,
            "affected_client_ids": affected_client_ids,
        });
        let preview_hash = payload_hash(&serde_json::to_vec(&hash_payload)?);
        Ok(ConfigurationPresetPreviewView {
            preset_id: preset.id,
            behavior: preset.behavior,
            name: preset.name,
            current_description: preset.description,
            current_updated_at: preset.updated_at,
            candidate_description,
            current_definition: preset.definition,
            candidate_definition: request.definition.clone(),
            changed_keys,
            affected_client_count: affected_client_ids.len() as i64,
            affected_client_ids,
            sections: rendered.sections,
            toml: rendered.toml,
            preview_hash,
        })
    }

    pub(crate) async fn update_configuration_preset(
        &self,
        preset_id: Uuid,
        preview: &ConfigurationPresetPreviewView,
        operator: &AuthContext,
    ) -> Result<ConfigurationPresetView> {
        let now = unix_now().to_string();
        let mut preset = match self {
            Self::Memory(memory) => {
                let mut presets = memory.configuration_presets.write().await;
                let preset = presets
                    .iter_mut()
                    .find(|preset| preset.id == preset_id)
                    .context("configuration_preset_not_found")?;
                anyhow::ensure!(
                    preset.kind == "custom",
                    "configuration_preset_system_immutable"
                );
                anyhow::ensure!(
                    preset.updated_at == preview.current_updated_at
                        && preset.description == preview.current_description
                        && preset.definition == preview.current_definition,
                    "configuration_preset_preview_stale"
                );
                if preview.current_definition != preview.candidate_definition {
                    let mut current_clients = memory
                        .configuration_preset_overrides
                        .read()
                        .await
                        .iter()
                        .filter(|entry| entry.preset_id == preset_id)
                        .map(|entry| entry.client_id.clone())
                        .collect::<Vec<_>>();
                    current_clients.sort();
                    current_clients.dedup();
                    anyhow::ensure!(
                        current_clients == preview.affected_client_ids,
                        "configuration_preset_preview_stale"
                    );
                }
                preset.description = preview.candidate_description.clone();
                preset.definition = preview.candidate_definition.clone();
                preset.updated_at = now;
                preset.clone()
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current = sqlx::query(
                    r#"
                    SELECT kind, description, definition, updated_at::text AS updated_at
                    FROM configuration_presets
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(preset_id)
                .fetch_optional(&mut *tx)
                .await?
                .context("configuration_preset_not_found")?;
                let current_kind: String = current.try_get("kind")?;
                let current_description: Option<String> = current.try_get("description")?;
                let current_definition = current
                    .try_get::<sqlx::types::Json<Value>, _>("definition")?
                    .0;
                let current_updated_at: String = current.try_get("updated_at")?;
                anyhow::ensure!(
                    current_kind == "custom",
                    "configuration_preset_system_immutable"
                );
                anyhow::ensure!(
                    current_updated_at == preview.current_updated_at
                        && current_description == preview.current_description
                        && current_definition == preview.current_definition,
                    "configuration_preset_preview_stale"
                );
                if preview.current_definition != preview.candidate_definition {
                    let current_clients = sqlx::query_scalar::<_, String>(
                        r#"
                        SELECT client_id
                        FROM client_configuration_preset_overrides
                        WHERE preset_id = $1
                        ORDER BY client_id
                        FOR UPDATE
                        "#,
                    )
                    .bind(preset_id)
                    .fetch_all(&mut *tx)
                    .await?;
                    anyhow::ensure!(
                        current_clients == preview.affected_client_ids,
                        "configuration_preset_preview_stale"
                    );
                }
                let row = sqlx::query(
                    r#"
                    UPDATE configuration_presets
                    SET description = $2, definition = $3, updated_at = now()
                    WHERE id = $1 AND kind = 'custom'
                    RETURNING
                        id, behavior, name, kind, is_default, description, definition,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        0::bigint AS effective_vps_count,
                        (
                            SELECT count(*)::bigint
                            FROM client_configuration_preset_overrides selected
                            WHERE selected.preset_id = configuration_presets.id
                        ) AS override_vps_count
                    "#,
                )
                .bind(preset_id)
                .bind(&preview.candidate_description)
                .bind(sqlx::types::Json(&preview.candidate_definition))
                .fetch_optional(&mut *tx)
                .await?
                .context("configuration_preset_not_found_or_immutable")?;
                let mut preset = configuration_preset_from_row(row)?;
                preset.effective_vps_count = preset.override_vps_count;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "configuration_preset.updated",
                    &format!("configuration_preset:{}", preset.id),
                    configuration_preset_audit_metadata(&preset, &preview.affected_client_ids),
                    operator,
                )
                .await?;
                tx.commit().await?;
                return Ok(preset);
            }
        };
        preset.effective_vps_count = preset.override_vps_count;
        self.record_configuration_preset_audit(
            "configuration_preset.updated",
            &preset,
            &preview.affected_client_ids,
            operator,
        )
        .await?;
        Ok(preset)
    }

    pub(crate) async fn delete_configuration_preset(
        &self,
        preset_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        let preset = self
            .configuration_preset_by_id(preset_id)
            .await?
            .with_context(|| format!("configuration_preset_not_found:{preset_id}"))?;
        anyhow::ensure!(
            preset.kind == "custom",
            "configuration_preset_system_immutable"
        );
        match self {
            Self::Memory(memory) => {
                let mut presets = memory.configuration_presets.write().await;
                let current = presets
                    .iter()
                    .find(|candidate| candidate.id == preset_id)
                    .context("configuration_preset_not_found")?;
                anyhow::ensure!(
                    current.kind == "custom",
                    "configuration_preset_system_immutable"
                );
                anyhow::ensure!(
                    !memory
                        .configuration_preset_overrides
                        .read()
                        .await
                        .iter()
                        .any(|entry| entry.preset_id == preset_id),
                    "configuration_preset_in_use"
                );
                presets.retain(|candidate| candidate.id != preset_id);
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let kind: Option<String> = sqlx::query_scalar(
                    "SELECT kind FROM configuration_presets WHERE id = $1 FOR UPDATE",
                )
                .bind(preset_id)
                .fetch_optional(&mut *tx)
                .await?;
                let kind = kind.context("configuration_preset_not_found")?;
                anyhow::ensure!(kind == "custom", "configuration_preset_system_immutable");
                let in_use: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM client_configuration_preset_overrides WHERE preset_id = $1)",
                )
                .bind(preset_id)
                .fetch_one(&mut *tx)
                .await?;
                anyhow::ensure!(!in_use, "configuration_preset_in_use");
                sqlx::query("DELETE FROM configuration_presets WHERE id = $1")
                    .bind(preset_id)
                    .execute(&mut *tx)
                    .await?;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "configuration_preset.deleted",
                    &format!("configuration_preset:{}", preset.id),
                    serde_json::json!({
                        "preset_id": preset.id,
                        "behavior": preset.behavior,
                        "name": preset.name,
                        "kind": preset.kind,
                        "target_clients": [],
                    }),
                    operator,
                )
                .await?;
                tx.commit().await?;
                return Ok(());
            }
        }
        self.record_configuration_preset_audit(
            "configuration_preset.deleted",
            &preset,
            &[],
            operator,
        )
        .await
    }

    pub(crate) async fn list_configuration_sources(
        &self,
        client_id: Option<&str>,
        behavior: Option<&str>,
    ) -> Result<Vec<ConfigurationSourceView>> {
        let requested_client_ids = client_id.map(|client_id| vec![client_id.to_string()]);
        let agents = match requested_client_ids.as_deref() {
            Some(client_ids) => self.list_agents_for_client_ids(client_ids).await?,
            None => self.list_agents().await?,
        };
        self.configuration_sources_for_agents(&agents, behavior)
            .await
    }

    async fn configuration_sources_for_agents(
        &self,
        agents: &[AgentView],
        behavior: Option<&str>,
    ) -> Result<Vec<ConfigurationSourceView>> {
        let presets = self.configuration_preset_catalog(behavior).await?;
        let defaults = presets
            .iter()
            .filter(|preset| preset.is_default)
            .map(|preset| (preset.behavior.as_str(), preset))
            .collect::<HashMap<_, _>>();
        let presets_by_id = presets
            .iter()
            .map(|preset| (preset.id, preset))
            .collect::<HashMap<_, _>>();
        let client_ids = agents
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>();
        let overrides = self
            .list_configuration_preset_overrides(Some(&client_ids), behavior)
            .await?;
        let overrides = overrides
            .iter()
            .map(|entry| ((entry.client_id.as_str(), entry.behavior.as_str()), entry))
            .collect::<HashMap<_, _>>();
        let mut rows = Vec::new();
        for agent in agents {
            for selected_behavior in CONFIGURATION_BEHAVIORS
                .iter()
                .copied()
                .filter(|candidate| behavior.is_none_or(|value| *candidate == value))
            {
                let override_record = overrides.get(&(agent.id.as_str(), selected_behavior));
                let preset = if let Some(entry) = override_record {
                    presets_by_id
                        .get(&entry.preset_id)
                        .copied()
                        .with_context(|| {
                            format!(
                                "configuration_preset_override_missing:{}:{}",
                                entry.client_id, entry.preset_id
                            )
                        })?
                } else {
                    defaults.get(selected_behavior).copied().with_context(|| {
                        format!("configuration_preset_default_missing:{selected_behavior}")
                    })?
                };
                let ospf_command_configured = if selected_behavior == "ospf_update_command" {
                    parse_ospf_update_commands(&preset.definition)?.is_some()
                } else {
                    true
                };
                let (readiness_state, readiness_reason) = if !ospf_command_configured {
                    (
                        "unconfigured",
                        "This VPS's effective OSPF updater preset is unconfigured; assign a configured preset or use a tunnel-plan endpoint override",
                    )
                } else if matches!(agent.status.as_str(), "online" | "connected") {
                    (
                        "unverified",
                        "No agent evidence verifies the selected paths or executable yet",
                    )
                } else {
                    (
                        "unavailable",
                        "VPS is not online; the desired selection is retained",
                    )
                };
                rows.push(ConfigurationSourceView {
                    client_id: agent.id.clone(),
                    behavior: selected_behavior.to_string(),
                    effective_preset_id: preset.id,
                    effective_preset_name: preset.name.clone(),
                    effective_preset_kind: preset.kind.clone(),
                    selection_origin: if override_record.is_some() {
                        "explicit_override"
                    } else {
                        "system_default"
                    }
                    .to_string(),
                    override_updated_at: override_record.map(|entry| entry.updated_at.clone()),
                    runtime_sync: ConfigurationRuntimeSyncView {
                        state: "unknown".to_string(),
                        reason: "Runtime apply state has not been compared yet".to_string(),
                    },
                    readiness: ConfigurationReadinessView {
                        state: readiness_state.to_string(),
                        reason: readiness_reason.to_string(),
                        evidence: serde_json::json!({
                            "client_status": agent.status,
                            "command_configured": ospf_command_configured
                        }),
                    },
                });
            }
        }
        rows.sort_by(|left, right| {
            left.client_id
                .cmp(&right.client_id)
                .then_with(|| behavior_order(&left.behavior).cmp(&behavior_order(&right.behavior)))
        });
        Ok(rows)
    }

    pub(crate) async fn preview_configuration_source_override(
        &self,
        request: &PreviewConfigurationSourceOverrideRequest,
    ) -> Result<ConfigurationSourceOverridePreviewView> {
        validate_configuration_behavior(&request.behavior)?;
        let mut target_client_ids = request.target_client_ids.clone();
        target_client_ids.sort();
        target_client_ids.dedup();
        anyhow::ensure!(
            !target_client_ids.is_empty(),
            "configuration_source_override_targets_required"
        );
        let before_rows = self
            .list_configuration_sources_for_clients(&target_client_ids, &request.behavior)
            .await?;
        anyhow::ensure!(
            before_rows.len() == target_client_ids.len(),
            "configuration_source_override_targets_not_found"
        );
        let preset = match request.action {
            ConfigurationOverrideAction::Set => {
                let preset_id = request
                    .preset_id
                    .context("configuration_source_override_preset_required")?;
                let preset = self
                    .configuration_preset_by_id(preset_id)
                    .await?
                    .context("configuration_preset_not_found")?;
                anyhow::ensure!(
                    preset.behavior == request.behavior,
                    "configuration_source_override_behavior_mismatch"
                );
                anyhow::ensure!(
                    !preset.is_default,
                    "configuration_source_override_default_requires_reset"
                );
                Some(preset)
            }
            ConfigurationOverrideAction::Reset => {
                anyhow::ensure!(
                    request.preset_id.is_none(),
                    "configuration_source_override_reset_preset_forbidden"
                );
                None
            }
        };
        let default = self
            .list_configuration_presets(Some(&request.behavior))
            .await?
            .into_iter()
            .find(|preset| preset.is_default)
            .context("configuration_preset_default_missing")?;
        let mut targets = Vec::with_capacity(before_rows.len());
        for before in before_rows {
            let after = preset.as_ref().unwrap_or(&default);
            targets.push(ConfigurationSourceChangeView {
                client_id: before.client_id,
                before_preset_id: before.effective_preset_id,
                before_preset_name: before.effective_preset_name,
                before_origin: before.selection_origin,
                after_preset_id: after.id,
                after_preset_name: after.name.clone(),
                after_origin: if preset.is_some() {
                    "explicit_override"
                } else {
                    "system_default"
                }
                .to_string(),
            });
        }
        targets.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        let selector_expression = request.selector_expression.trim().to_string();
        let hash_payload = serde_json::json!({
            "action": request.action,
            "behavior": request.behavior,
            "preset": preset.as_ref().map(|preset| serde_json::json!({
                "id": preset.id,
                "updated_at": preset.updated_at,
                "definition": preset.definition,
            })),
            "selector_expression": selector_expression,
            "targets": targets.iter().map(|target| serde_json::json!({
                "client_id": target.client_id,
                "before_preset_id": target.before_preset_id,
                "before_origin": target.before_origin,
                "after_preset_id": target.after_preset_id,
                "after_origin": target.after_origin,
            })).collect::<Vec<_>>(),
        });
        let preview_hash = payload_hash(&serde_json::to_vec(&hash_payload)?);
        Ok(ConfigurationSourceOverridePreviewView {
            action: request.action,
            behavior: request.behavior.clone(),
            preset,
            selector_expression,
            target_count: targets.len(),
            targets,
            preview_hash,
        })
    }

    pub(crate) async fn apply_configuration_source_override(
        &self,
        preview: &ConfigurationSourceOverridePreviewView,
        operator: &AuthContext,
    ) -> Result<()> {
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let _agent_lifecycle_guard = memory.agent_key_lifecycle.lock().await;
                let target_ids = preview
                    .targets
                    .iter()
                    .map(|target| target.client_id.as_str())
                    .collect::<BTreeSet<_>>();
                let hidden = memory.hidden_clients.read().await;
                let agents = memory.agents.read().await;
                let visible_target_count = agents
                    .iter()
                    .filter(|agent| {
                        target_ids.contains(agent.id.as_str()) && !hidden.contains(&agent.id)
                    })
                    .count();
                anyhow::ensure!(
                    visible_target_count == target_ids.len(),
                    "configuration_source_override_preview_stale"
                );
                drop(agents);
                drop(hidden);
                let presets = memory.configuration_presets.read().await;
                let mut overrides = memory.configuration_preset_overrides.write().await;
                if let Some(reviewed) = preview.preset.as_ref() {
                    let current = presets
                        .iter()
                        .find(|preset| preset.id == reviewed.id)
                        .context("configuration_source_override_preview_stale")?;
                    anyhow::ensure!(
                        current.updated_at == reviewed.updated_at
                            && current.definition == reviewed.definition,
                        "configuration_source_override_preview_stale"
                    );
                }
                let default = presets
                    .iter()
                    .find(|preset| preset.behavior == preview.behavior && preset.is_default)
                    .context("configuration_preset_default_missing")?;
                for target in &preview.targets {
                    let current = overrides.iter().find(|entry| {
                        entry.client_id == target.client_id && entry.behavior == preview.behavior
                    });
                    let current_id = current.map_or(default.id, |entry| entry.preset_id);
                    let current_origin = if current.is_some() {
                        "explicit_override"
                    } else {
                        "system_default"
                    };
                    anyhow::ensure!(
                        current_id == target.before_preset_id
                            && current_origin == target.before_origin,
                        "configuration_source_override_preview_stale"
                    );
                }
                drop(presets);
                for target in &preview.targets {
                    overrides.retain(|entry| {
                        entry.client_id != target.client_id || entry.behavior != preview.behavior
                    });
                    if matches!(preview.action, ConfigurationOverrideAction::Set) {
                        overrides.push(ConfigurationPresetOverrideRecord {
                            client_id: target.client_id.clone(),
                            behavior: preview.behavior.clone(),
                            preset_id: target.after_preset_id,
                            updated_at: now.clone(),
                        });
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_agent_identity_lifecycle(&mut tx).await?;
                let target_ids = preview
                    .targets
                    .iter()
                    .map(|target| target.client_id.as_str())
                    .collect::<Vec<_>>();
                let locked_target_count = sqlx::query(
                    r#"
                    SELECT id
                    FROM clients
                    WHERE id = ANY($1::text[])
                      AND hidden_at IS NULL
                      AND status <> 'deleted'
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(&target_ids)
                .fetch_all(&mut *tx)
                .await?
                .len();
                anyhow::ensure!(
                    locked_target_count == target_ids.len(),
                    "configuration_source_override_preview_stale"
                );
                if let Some(reviewed) = preview.preset.as_ref() {
                    let current = sqlx::query(
                        r#"
                        SELECT definition, updated_at::text AS updated_at
                        FROM configuration_presets
                        WHERE id = $1
                        FOR SHARE
                        "#,
                    )
                    .bind(reviewed.id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .context("configuration_source_override_preview_stale")?;
                    let definition = current
                        .try_get::<sqlx::types::Json<Value>, _>("definition")?
                        .0;
                    let updated_at: String = current.try_get("updated_at")?;
                    anyhow::ensure!(
                        updated_at == reviewed.updated_at && definition == reviewed.definition,
                        "configuration_source_override_preview_stale"
                    );
                }
                let current_rows = sqlx::query(
                    r#"
                    SELECT
                        client.id AS client_id,
                        COALESCE(selected.preset_id, fallback.id) AS preset_id,
                        CASE
                            WHEN selected.preset_id IS NULL THEN 'system_default'
                            ELSE 'explicit_override'
                        END AS selection_origin
                    FROM clients client
                    LEFT JOIN client_configuration_preset_overrides selected
                      ON selected.client_id = client.id
                     AND selected.behavior = $2
                    JOIN configuration_presets fallback
                      ON fallback.behavior = $2
                     AND fallback.is_default
                    WHERE client.id = ANY($1::text[])
                      AND client.hidden_at IS NULL
                      AND client.status <> 'deleted'
                    ORDER BY client.id
                    "#,
                )
                .bind(&target_ids)
                .bind(&preview.behavior)
                .fetch_all(&mut *tx)
                .await?;
                let expected = preview
                    .targets
                    .iter()
                    .map(|target| (target.client_id.as_str(), target))
                    .collect::<HashMap<_, _>>();
                anyhow::ensure!(
                    current_rows.len() == expected.len(),
                    "configuration_source_override_preview_stale"
                );
                for row in current_rows {
                    let client_id: String = row.try_get("client_id")?;
                    let preset_id: Uuid = row.try_get("preset_id")?;
                    let selection_origin: String = row.try_get("selection_origin")?;
                    let reviewed = expected
                        .get(client_id.as_str())
                        .context("configuration_source_override_preview_stale")?;
                    anyhow::ensure!(
                        preset_id == reviewed.before_preset_id
                            && selection_origin == reviewed.before_origin,
                        "configuration_source_override_preview_stale"
                    );
                }
                for target in &preview.targets {
                    match preview.action {
                        ConfigurationOverrideAction::Set => {
                            sqlx::query(
                                r#"
                                INSERT INTO client_configuration_preset_overrides (
                                    client_id, behavior, preset_id, updated_by, updated_at
                                )
                                VALUES ($1, $2, $3, $4, now())
                                ON CONFLICT (client_id, behavior) DO UPDATE SET
                                    preset_id = EXCLUDED.preset_id,
                                    updated_by = EXCLUDED.updated_by,
                                    updated_at = now()
                                "#,
                            )
                            .bind(&target.client_id)
                            .bind(&preview.behavior)
                            .bind(target.after_preset_id)
                            .bind(operator.operator.id)
                            .execute(&mut *tx)
                            .await?;
                        }
                        ConfigurationOverrideAction::Reset => {
                            sqlx::query(
                                r#"
                                DELETE FROM client_configuration_preset_overrides
                                WHERE client_id = $1 AND behavior = $2
                                "#,
                            )
                            .bind(&target.client_id)
                            .bind(&preview.behavior)
                            .execute(&mut *tx)
                            .await?;
                        }
                    }
                }
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "configuration_source_override.applied",
                    &format!("configuration_behavior:{}", preview.behavior),
                    serde_json::json!({
                        "action": preview.action,
                        "behavior": preview.behavior,
                        "preset_id": preview.preset.as_ref().map(|preset| preset.id),
                        "selector_expression": preview.selector_expression,
                        "target_clients": preview.targets.iter().map(|target| target.client_id.as_str()).collect::<Vec<_>>(),
                        "preview_hash": preview.preview_hash,
                    }),
                    operator,
                )
                .await?;
                tx.commit().await?;
                return Ok(());
            }
        }
        self.record_configuration_audit(
            "configuration_source_override.applied",
            &format!("configuration_behavior:{}", preview.behavior),
            serde_json::json!({
                "action": preview.action,
                "behavior": preview.behavior,
                "preset_id": preview.preset.as_ref().map(|preset| preset.id),
                "selector_expression": preview.selector_expression,
                "target_clients": preview.targets.iter().map(|target| target.client_id.as_str()).collect::<Vec<_>>(),
                "preview_hash": preview.preview_hash,
            }),
            operator,
        )
        .await
    }

    pub(crate) async fn render_configuration_preset_patch_toml(
        &self,
        client_id: &str,
    ) -> Result<String> {
        self.render_configuration_preset_patches_for_clients(&[client_id.to_string()])
            .await?
            .remove(client_id)
            .context("effective_agent_config_client_not_found")
    }

    pub(crate) async fn render_configuration_preset_patches_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<BTreeMap<String, String>> {
        let requested_client_ids = client_ids.iter().collect::<BTreeSet<_>>();
        let agents = self.list_agents_for_client_ids(client_ids).await?;
        anyhow::ensure!(
            agents.len() == requested_client_ids.len(),
            "effective_agent_config_client_not_found"
        );
        let sources = self.configuration_sources_for_agents(&agents, None).await?;
        let presets = self.configuration_preset_catalog(None).await?;
        let by_id = presets
            .iter()
            .map(|preset| (preset.id, preset))
            .collect::<HashMap<_, _>>();
        let mut sections_by_client = agents
            .iter()
            .map(|agent| (agent.id.clone(), Value::Object(Map::new())))
            .collect::<BTreeMap<_, _>>();
        let mut source_count_by_client = HashMap::<&str, usize>::new();
        for source in &sources {
            let preset = by_id.get(&source.effective_preset_id).with_context(|| {
                format!(
                    "effective_configuration_preset_not_found:{}",
                    source.effective_preset_id
                )
            })?;
            let rendered =
                render_configuration_preset_definition(&preset.behavior, &preset.definition)?;
            let sections = sections_by_client
                .get_mut(&source.client_id)
                .context("effective_agent_config_client_not_found")?;
            merge_json_object(sections, rendered.sections)?;
            *source_count_by_client
                .entry(source.client_id.as_str())
                .or_default() += 1;
        }
        sections_by_client
            .into_iter()
            .map(|(client_id, sections)| {
                anyhow::ensure!(
                    source_count_by_client.get(client_id.as_str()).copied()
                        == Some(CONFIGURATION_BEHAVIORS.len()),
                    "effective_agent_config_client_not_found"
                );
                Ok((
                    client_id,
                    toml::to_string_pretty(&sections)
                        .context("effective_configuration_toml_failed")?,
                ))
            })
            .collect()
    }

    pub(crate) async fn effective_ospf_command_sources_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<BTreeMap<String, Option<ResolvedOspfCommandSource>>> {
        let requested = client_ids.iter().cloned().collect::<BTreeSet<_>>();
        let agents = self.list_agents_for_client_ids(client_ids).await?;
        anyhow::ensure!(
            agents.len() == requested.len(),
            "ospf_update_command_client_not_found"
        );
        let sources = self
            .configuration_sources_for_agents(&agents, Some("ospf_update_command"))
            .await?;
        let presets = self
            .configuration_preset_catalog(Some("ospf_update_command"))
            .await?;
        let by_id = presets
            .into_iter()
            .map(|preset| (preset.id, preset))
            .collect::<HashMap<_, _>>();
        let mut resolved = BTreeMap::new();
        for source in sources {
            let preset = by_id
                .get(&source.effective_preset_id)
                .context("effective_ospf_update_command_preset_not_found")?;
            let commands = parse_ospf_update_commands(&preset.definition)?;
            let command_source = if let Some((status, update)) = commands {
                Some(ResolvedOspfCommandSource {
                    origin: "configuration_preset".to_string(),
                    id: preset.id,
                    name: preset.name.clone(),
                    definition_hash: payload_hash(&serde_json::to_vec(&preset.definition)?),
                    status,
                    update,
                })
            } else {
                None
            };
            resolved.insert(source.client_id, command_source);
        }
        anyhow::ensure!(
            resolved.keys().cloned().collect::<BTreeSet<_>>() == requested,
            "ospf_update_command_client_not_found"
        );
        Ok(resolved)
    }

    pub(crate) async fn list_network_adapter_definitions(
        &self,
        adapter_kind: Option<&str>,
    ) -> Result<Vec<NetworkAdapterDefinitionView>> {
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .network_adapter_definitions
                    .read()
                    .await
                    .iter()
                    .filter(|adapter| {
                        adapter_kind.is_none_or(|value| adapter.adapter_kind == value)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    left.adapter_kind
                        .cmp(&right.adapter_kind)
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(rows)
            }
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                SELECT id, adapter_kind, name, description, definition,
                       created_at::text AS created_at, updated_at::text AS updated_at
                FROM network_adapter_definitions
                WHERE $1::text IS NULL OR adapter_kind = $1
                ORDER BY adapter_kind, name
                "#,
            )
            .bind(adapter_kind)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(network_adapter_definition_from_row)
            .collect::<Result<Vec<_>>>()?),
        }
    }

    pub(crate) async fn network_adapter_definition_by_id(
        &self,
        id: Uuid,
        adapter_kind: Option<&str>,
    ) -> Result<Option<NetworkAdapterDefinitionView>> {
        Ok(self
            .list_network_adapter_definitions(adapter_kind)
            .await?
            .into_iter()
            .find(|definition| definition.id == id))
    }

    pub(crate) async fn create_network_adapter_definition(
        &self,
        request: &UpsertNetworkAdapterDefinitionRequest,
        operator: &AuthContext,
    ) -> Result<NetworkAdapterDefinitionView> {
        validate_network_adapter_definition(request)?;
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut definitions = memory.network_adapter_definitions.write().await;
                anyhow::ensure!(
                    !definitions.iter().any(|definition| {
                        definition.adapter_kind == request.adapter_kind
                            && definition.name.eq_ignore_ascii_case(request.name.trim())
                    }),
                    "network_adapter_definition_duplicate"
                );
                let definition = NetworkAdapterDefinitionView {
                    id: Uuid::new_v4(),
                    adapter_kind: request.adapter_kind.clone(),
                    name: request.name.trim().to_string(),
                    description: normalized_description(request.description.as_deref()),
                    definition: request.definition.clone(),
                    created_at: now.clone(),
                    updated_at: now,
                };
                definitions.push(definition.clone());
                drop(definitions);
                self.record_configuration_audit(
                    "network_adapter_definition.created",
                    &format!("network_adapter_definition:{}", definition.id),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
                Ok(definition)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO network_adapter_definitions (
                        id, adapter_kind, name, description, definition
                    )
                    VALUES ($1, $2, $3, $4, $5)
                    RETURNING id, adapter_kind, name, description, definition,
                              created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(&request.adapter_kind)
                .bind(request.name.trim())
                .bind(normalized_description(request.description.as_deref()))
                .bind(sqlx::types::Json(&request.definition))
                .fetch_one(&mut *tx)
                .await
                .map_err(network_adapter_database_error)?;
                let definition = network_adapter_definition_from_row(row)?;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "network_adapter_definition.created",
                    &format!("network_adapter_definition:{}", definition.id),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
                tx.commit().await?;
                Ok(definition)
            }
        }
    }

    pub(crate) async fn update_network_adapter_definition(
        &self,
        id: Uuid,
        request: &UpsertNetworkAdapterDefinitionRequest,
        operator: &AuthContext,
    ) -> Result<NetworkAdapterDefinitionView> {
        validate_network_adapter_definition(request)?;
        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                anyhow::ensure!(
                    !plans
                        .iter()
                        .any(|plan| tunnel_plan_references_adapter(plan, id)),
                    "network_adapter_definition_in_use"
                );
                let mut definitions = memory.network_adapter_definitions.write().await;
                let index = definitions
                    .iter()
                    .position(|definition| definition.id == id)
                    .context("network_adapter_definition_not_found")?;
                anyhow::ensure!(
                    definitions[index].adapter_kind == request.adapter_kind,
                    "network_adapter_definition_kind_immutable"
                );
                anyhow::ensure!(
                    !definitions.iter().any(|definition| {
                        definition.id != id
                            && definition.adapter_kind == request.adapter_kind
                            && definition.name.eq_ignore_ascii_case(request.name.trim())
                    }),
                    "network_adapter_definition_duplicate"
                );
                let definition = &mut definitions[index];
                definition.name = request.name.trim().to_string();
                definition.description = normalized_description(request.description.as_deref());
                definition.definition = request.definition.clone();
                definition.updated_at = unix_now().to_string();
                let definition = definition.clone();
                drop(definitions);
                drop(plans);
                self.record_configuration_audit(
                    "network_adapter_definition.updated",
                    &format!("network_adapter_definition:{id}"),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
                Ok(definition)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("LOCK TABLE tunnel_plans IN SHARE MODE")
                    .execute(&mut *tx)
                    .await?;
                let current_kind = sqlx::query_scalar::<_, String>(
                    r#"
                    SELECT adapter_kind
                    FROM network_adapter_definitions
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .context("network_adapter_definition_not_found")?;
                anyhow::ensure!(
                    current_kind == request.adapter_kind,
                    "network_adapter_definition_kind_immutable"
                );
                let in_use = postgres_tunnel_plan_references_adapter(&mut tx, id).await?;
                anyhow::ensure!(!in_use, "network_adapter_definition_in_use");
                let row = sqlx::query(
                    r#"
                    UPDATE network_adapter_definitions
                    SET name = $2, description = $3,
                        definition = $4, updated_at = now()
                    WHERE id = $1
                    RETURNING id, adapter_kind, name, description, definition,
                              created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(id)
                .bind(request.name.trim())
                .bind(normalized_description(request.description.as_deref()))
                .bind(sqlx::types::Json(&request.definition))
                .fetch_optional(&mut *tx)
                .await
                .map_err(network_adapter_database_error)?
                .context("network_adapter_definition_not_found")?;
                let definition = network_adapter_definition_from_row(row)?;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "network_adapter_definition.updated",
                    &format!("network_adapter_definition:{id}"),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
                tx.commit().await?;
                Ok(definition)
            }
        }
    }

    pub(crate) async fn delete_network_adapter_definition(
        &self,
        id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                anyhow::ensure!(
                    !plans
                        .iter()
                        .any(|plan| tunnel_plan_references_adapter(plan, id)),
                    "network_adapter_definition_in_use"
                );
                let mut definitions = memory.network_adapter_definitions.write().await;
                let definition = definitions
                    .iter()
                    .find(|definition| definition.id == id)
                    .cloned()
                    .context("network_adapter_definition_not_found")?;
                let before = definitions.len();
                definitions.retain(|definition| definition.id != id);
                anyhow::ensure!(
                    before != definitions.len(),
                    "network_adapter_definition_not_found"
                );
                drop(definitions);
                drop(plans);
                self.record_configuration_audit(
                    "network_adapter_definition.deleted",
                    &format!("network_adapter_definition:{id}"),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("LOCK TABLE tunnel_plans IN SHARE MODE")
                    .execute(&mut *tx)
                    .await?;
                let in_use = postgres_tunnel_plan_references_adapter(&mut tx, id).await?;
                anyhow::ensure!(!in_use, "network_adapter_definition_in_use");
                let row = sqlx::query(
                    r#"
                    DELETE FROM network_adapter_definitions
                    WHERE id = $1
                    RETURNING id, adapter_kind, name, description, definition,
                              created_at::text AS created_at, updated_at::text AS updated_at
                    "#,
                )
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?
                .context("network_adapter_definition_not_found")?;
                let definition = network_adapter_definition_from_row(row)?;
                insert_configuration_audit_in_tx(
                    &mut tx,
                    "network_adapter_definition.deleted",
                    &format!("network_adapter_definition:{id}"),
                    network_adapter_audit_metadata(&definition),
                    operator,
                )
                .await?;
                tx.commit().await?;
            }
        }
        Ok(())
    }

    async fn list_configuration_preset_overrides(
        &self,
        client_ids: Option<&[String]>,
        behavior: Option<&str>,
    ) -> Result<Vec<ConfigurationPresetOverrideRecord>> {
        if client_ids.is_some_and(<[String]>::is_empty) {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let selected = client_ids.map(|client_ids| {
                    client_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                });
                Ok(memory
                    .configuration_preset_overrides
                    .read()
                    .await
                    .iter()
                    .filter(|entry| {
                        selected
                            .as_ref()
                            .is_none_or(|selected| selected.contains(entry.client_id.as_str()))
                    })
                    .filter(|entry| behavior.is_none_or(|value| entry.behavior == value))
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => Ok(sqlx::query(
                r#"
                SELECT client_id, behavior, preset_id,
                       updated_at::text AS updated_at
                FROM client_configuration_preset_overrides
                WHERE ($1::text[] IS NULL OR client_id = ANY($1))
                  AND ($2::text IS NULL OR behavior = $2)
                ORDER BY client_id, behavior
                "#,
            )
            .bind(client_ids.map(<[String]>::to_vec))
            .bind(behavior)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row| {
                Ok(ConfigurationPresetOverrideRecord {
                    client_id: row.try_get("client_id")?,
                    behavior: row.try_get("behavior")?,
                    preset_id: row.try_get("preset_id")?,
                    updated_at: row.try_get("updated_at")?,
                })
            })
            .collect::<Result<Vec<_>>>()?),
        }
    }

    async fn list_configuration_sources_for_clients(
        &self,
        client_ids: &[String],
        behavior: &str,
    ) -> Result<Vec<ConfigurationSourceView>> {
        let agents = self.list_agents_for_client_ids(client_ids).await?;
        self.configuration_sources_for_agents(&agents, Some(behavior))
            .await
    }

    async fn configuration_preset_override_client_ids(
        &self,
        preset_id: Uuid,
    ) -> Result<Vec<String>> {
        let mut clients = self
            .list_configuration_preset_overrides(None, None)
            .await?
            .into_iter()
            .filter(|entry| entry.preset_id == preset_id)
            .map(|entry| entry.client_id)
            .collect::<Vec<_>>();
        clients.sort();
        clients.dedup();
        Ok(clients)
    }

    async fn record_configuration_preset_audit(
        &self,
        action: &str,
        preset: &ConfigurationPresetView,
        client_ids: &[String],
        operator: &AuthContext,
    ) -> Result<()> {
        self.record_configuration_audit(
            action,
            &format!("configuration_preset:{}", preset.id),
            configuration_preset_audit_metadata(preset, client_ids),
            operator,
        )
        .await
    }

    async fn record_configuration_audit(
        &self,
        action: &str,
        target: &str,
        metadata: Value,
        operator: &AuthContext,
    ) -> Result<()> {
        let command_hash = payload_hash(metadata.to_string().as_bytes());
        let metadata = configuration_audit_metadata(metadata, operator);
        match self {
            Self::Memory(memory) => {
                memory
                    .audits
                    .write()
                    .await
                    .push(crate::model::AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: Some(operator.operator.id),
                        action: action.to_string(),
                        target: target.to_string(),
                        command_hash: Some(command_hash),
                        metadata,
                        created_at: unix_now().to_string(),
                    });
                Ok(())
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(action)
                .bind(target)
                .bind(command_hash)
                .bind(sqlx::types::Json(metadata))
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }
}

pub(crate) fn validate_configuration_behavior(behavior: &str) -> Result<()> {
    anyhow::ensure!(
        CONFIGURATION_BEHAVIORS.contains(&behavior),
        "configuration_behavior_invalid"
    );
    Ok(())
}

pub(crate) fn validate_configuration_preset_request(
    behavior: &str,
    name: &str,
    description: Option<&str>,
    definition: &Value,
) -> Result<()> {
    validate_configuration_behavior(behavior)?;
    validate_operator_name(name, "configuration_preset_name_invalid")?;
    anyhow::ensure!(
        description.is_none_or(|value| value.len() <= 4096),
        "configuration_preset_description_invalid"
    );
    validate_configuration_preset_definition(behavior, definition)
}

pub(crate) fn validate_configuration_preset_definition(
    behavior: &str,
    definition: &Value,
) -> Result<()> {
    let rendered = render_configuration_preset_definition(behavior, definition)?;
    anyhow::ensure!(
        rendered.sections.is_object(),
        "configuration_preset_definition_invalid"
    );
    Ok(())
}

struct RenderedConfigurationPreset {
    sections: Value,
    toml: String,
}

fn render_configuration_preset_definition(
    behavior: &str,
    definition: &Value,
) -> Result<RenderedConfigurationPreset> {
    validate_configuration_behavior(behavior)?;
    let sections = match behavior {
        "host_metrics" => render_host_metrics(definition)?,
        "tunnel_traffic" => render_tunnel_traffic(definition)?,
        "latency_probe" => render_latency_probe(definition)?,
        "ospf_update_command" => render_ospf_update_command(definition)?,
        "process_inventory" => render_process_inventory(definition)?,
        "user_sessions" => render_user_sessions(definition)?,
        "command_execution" => render_command_execution(definition)?,
        _ => unreachable!("validated configuration behavior"),
    };
    let toml =
        toml::to_string_pretty(&sections).context("configuration_preset_definition_toml_failed")?;
    Ok(RenderedConfigurationPreset { sections, toml })
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum HostMetricsDefinition {
    LinuxProcfs {
        proc_root: String,
        sys_class_net_dir: String,
        hostname_file: String,
        os_release_file: String,
    },
    CustomCommand {
        custom_metrics_command: PresetCommand,
    },
    LinuxProcfsAndCustomCommand {
        proc_root: String,
        sys_class_net_dir: String,
        hostname_file: String,
        os_release_file: String,
        custom_metrics_command: PresetCommand,
    },
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum TunnelTrafficDefinition {
    InterfaceCounters {},
    Vnstat { vnstat_argv: Vec<String> },
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum LatencyProbeDefinition {
    LinuxPingPreset {},
    ConfiguredPingArgv { probe_ping_argv: Vec<String> },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OspfUpdateCommandDefinition {
    contract_version: u16,
    status_command: Option<PresetCommand>,
    update_command: Option<PresetCommand>,
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum ProcessInventoryDefinition {
    LinuxProcfs {
        proc_root: String,
    },
    CustomCommand {
        process_inventory_command: PresetCommand,
    },
}

#[derive(Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum UserSessionsDefinition {
    LinuxWWhoPreset {},
    CustomCommand {
        user_sessions_command: PresetCommand,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandExecutionDefinition {
    shell_script_argv: Vec<String>,
    working_directory: Value,
    environment_policy: String,
    environment_keep: Vec<String>,
    environment_set: BTreeMap<String, String>,
    pty_policy: String,
    process_cleanup: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PresetCommand {
    argv: Vec<String>,
    max_timeout_secs: u64,
    max_output_bytes: u32,
}

fn render_host_metrics(definition: &Value) -> Result<Value> {
    let parsed: HostMetricsDefinition =
        serde_json::from_value(definition.clone()).context("host_metrics_definition_invalid")?;
    let telemetry = match parsed {
        HostMetricsDefinition::LinuxProcfs {
            proc_root,
            sys_class_net_dir,
            hostname_file,
            os_release_file,
        } => {
            validate_absolute_path(&proc_root, "proc_root")?;
            validate_absolute_path(&sys_class_net_dir, "sys_class_net_dir")?;
            validate_absolute_path(&hostname_file, "hostname_file")?;
            validate_absolute_path(&os_release_file, "os_release_file")?;
            serde_json::json!({
                "source": "linux_procfs",
                "proc_root": proc_root,
                "sys_class_net_dir": sys_class_net_dir,
                "hostname_file": hostname_file,
                "os_release_file": os_release_file
            })
        }
        HostMetricsDefinition::CustomCommand {
            custom_metrics_command,
        } => {
            validate_preset_command(&custom_metrics_command, "custom_metrics_command")?;
            serde_json::json!({
                "source": "custom_command",
                "custom_metrics_command": command_value(custom_metrics_command)
            })
        }
        HostMetricsDefinition::LinuxProcfsAndCustomCommand {
            proc_root,
            sys_class_net_dir,
            hostname_file,
            os_release_file,
            custom_metrics_command,
        } => {
            validate_absolute_path(&proc_root, "proc_root")?;
            validate_absolute_path(&sys_class_net_dir, "sys_class_net_dir")?;
            validate_absolute_path(&hostname_file, "hostname_file")?;
            validate_absolute_path(&os_release_file, "os_release_file")?;
            validate_preset_command(&custom_metrics_command, "custom_metrics_command")?;
            serde_json::json!({
                "source": "linux_procfs_and_custom_command",
                "proc_root": proc_root,
                "sys_class_net_dir": sys_class_net_dir,
                "hostname_file": hostname_file,
                "os_release_file": os_release_file,
                "custom_metrics_command": command_value(custom_metrics_command)
            })
        }
    };
    Ok(serde_json::json!({"telemetry": telemetry}))
}

fn render_tunnel_traffic(definition: &Value) -> Result<Value> {
    let parsed: TunnelTrafficDefinition =
        serde_json::from_value(definition.clone()).context("tunnel_traffic_definition_invalid")?;
    let argv = match parsed {
        TunnelTrafficDefinition::InterfaceCounters {} => Vec::new(),
        TunnelTrafficDefinition::Vnstat { vnstat_argv } => {
            validate_argv(&vnstat_argv, "vnstat_argv")?;
            vnstat_argv
        }
    };
    Ok(serde_json::json!({"network": {"runtime_vnstat_argv": argv}}))
}

fn render_latency_probe(definition: &Value) -> Result<Value> {
    let parsed: LatencyProbeDefinition =
        serde_json::from_value(definition.clone()).context("latency_probe_definition_invalid")?;
    let argv = match parsed {
        LatencyProbeDefinition::LinuxPingPreset {} => Vec::new(),
        LatencyProbeDefinition::ConfiguredPingArgv { probe_ping_argv } => {
            validate_argv(&probe_ping_argv, "probe_ping_argv")?;
            probe_ping_argv
        }
    };
    Ok(serde_json::json!({"network": {"probe_ping_argv": argv}}))
}

fn render_ospf_update_command(definition: &Value) -> Result<Value> {
    let commands = parse_ospf_update_commands(definition)?;
    let Some((status, update)) = commands else {
        return Ok(serde_json::json!({"network": {}}));
    };
    Ok(serde_json::json!({
        "network": {
            "ospf_status_command": runtime_command_value(status),
            "ospf_update_command": runtime_command_value(update)
        }
    }))
}

fn parse_ospf_update_commands(
    definition: &Value,
) -> Result<Option<(RuntimeTunnelCommand, RuntimeTunnelCommand)>> {
    let parsed: OspfUpdateCommandDefinition = serde_json::from_value(definition.clone())
        .context("ospf_update_command_definition_invalid")?;
    anyhow::ensure!(
        parsed.contract_version == vpsman_common::ROUTING_COST_ADAPTER_CONTRACT_VERSION,
        "ospf_update_command_contract_version_invalid"
    );
    anyhow::ensure!(
        parsed.status_command.is_some() == parsed.update_command.is_some(),
        "ospf_update_commands_must_be_configured_together"
    );
    let (Some(status), Some(update)) = (parsed.status_command, parsed.update_command) else {
        return Ok(None);
    };
    validate_preset_command(&status, "ospf_status_command")?;
    validate_preset_command(&update, "ospf_update_command")?;
    Ok(Some((
        RuntimeTunnelCommand {
            argv: status.argv,
            max_timeout_secs: status.max_timeout_secs,
            max_output_bytes: status.max_output_bytes,
        },
        RuntimeTunnelCommand {
            argv: update.argv,
            max_timeout_secs: update.max_timeout_secs,
            max_output_bytes: update.max_output_bytes,
        },
    )))
}

fn render_process_inventory(definition: &Value) -> Result<Value> {
    let parsed: ProcessInventoryDefinition = serde_json::from_value(definition.clone())
        .context("process_inventory_definition_invalid")?;
    let execution = match parsed {
        ProcessInventoryDefinition::LinuxProcfs { proc_root } => {
            validate_absolute_path(&proc_root, "proc_root")?;
            serde_json::json!({
                "process_inventory_source": "linux_procfs",
                "process_proc_root": proc_root
            })
        }
        ProcessInventoryDefinition::CustomCommand {
            process_inventory_command,
        } => {
            validate_preset_command(&process_inventory_command, "process_inventory_command")?;
            serde_json::json!({
                "process_inventory_source": "custom_command",
                "process_inventory_command": command_value(process_inventory_command)
            })
        }
    };
    Ok(serde_json::json!({"execution": execution}))
}

fn render_user_sessions(definition: &Value) -> Result<Value> {
    let parsed: UserSessionsDefinition =
        serde_json::from_value(definition.clone()).context("user_sessions_definition_invalid")?;
    let execution = match parsed {
        UserSessionsDefinition::LinuxWWhoPreset {} => serde_json::json!({
            "user_sessions_source": "linux_w_who_preset"
        }),
        UserSessionsDefinition::CustomCommand {
            user_sessions_command,
        } => {
            validate_preset_command(&user_sessions_command, "user_sessions_command")?;
            serde_json::json!({
                "user_sessions_source": "custom_command",
                "user_sessions_command": command_value(user_sessions_command)
            })
        }
    };
    Ok(serde_json::json!({"execution": execution}))
}

fn render_command_execution(definition: &Value) -> Result<Value> {
    let parsed: CommandExecutionDefinition = serde_json::from_value(definition.clone())
        .context("command_execution_definition_invalid")?;
    validate_argv(&parsed.shell_script_argv, "shell_script_argv")?;
    let working_directory = match parsed.working_directory {
        Value::Null => Value::Null,
        Value::String(path) => {
            validate_absolute_path(&path, "working_directory")?;
            Value::String(path)
        }
        _ => anyhow::bail!("working_directory_must_be_absolute_string_or_null"),
    };
    anyhow::ensure!(
        matches!(
            parsed.environment_policy.as_str(),
            "inherit" | "clean" | "minimal_path"
        ),
        "environment_policy_invalid"
    );
    anyhow::ensure!(
        matches!(parsed.pty_policy.as_str(), "native_pty" | "disabled"),
        "pty_policy_invalid"
    );
    anyhow::ensure!(
        matches!(
            parsed.process_cleanup.as_str(),
            "process_group" | "direct_child"
        ),
        "process_cleanup_invalid"
    );
    anyhow::ensure!(
        parsed.environment_keep.len() <= 64 && parsed.environment_set.len() <= 64,
        "environment_entries_too_many"
    );
    for key in &parsed.environment_keep {
        validate_environment_key(key)?;
    }
    for (key, value) in &parsed.environment_set {
        validate_environment_key(key)?;
        anyhow::ensure!(
            value.len() <= 4096 && !value.as_bytes().contains(&0),
            "environment_set_value_invalid"
        );
    }
    let mut execution = serde_json::json!({
            "shell_script_argv": parsed.shell_script_argv,
            "environment_policy": parsed.environment_policy,
            "environment_keep": parsed.environment_keep,
            "environment_set": parsed.environment_set,
            "pty_policy": parsed.pty_policy,
            "process_cleanup": parsed.process_cleanup
    });
    if !working_directory.is_null() {
        execution
            .as_object_mut()
            .expect("command execution object")
            .insert("working_directory".to_string(), working_directory);
    }
    Ok(serde_json::json!({"execution": execution}))
}

fn command_value(command: PresetCommand) -> Value {
    serde_json::json!({
        "argv": command.argv,
        "max_timeout_secs": command.max_timeout_secs,
        "max_output_bytes": command.max_output_bytes
    })
}

fn runtime_command_value(command: RuntimeTunnelCommand) -> Value {
    serde_json::json!({
        "argv": command.argv,
        "max_timeout_secs": command.max_timeout_secs,
        "max_output_bytes": command.max_output_bytes
    })
}

fn validate_preset_command(command: &PresetCommand, field: &str) -> Result<()> {
    validate_argv(&command.argv, field)?;
    anyhow::ensure!(
        (1..=120).contains(&command.max_timeout_secs)
            && (1024..=64 * 1024).contains(&command.max_output_bytes),
        "{field}_budget_invalid"
    );
    Ok(())
}

fn validate_argv(argv: &[String], field: &str) -> Result<()> {
    anyhow::ensure!(
        !argv.is_empty() && argv.len() <= MAX_ARGV_ITEMS,
        "{field}_invalid"
    );
    anyhow::ensure!(
        argv[0].starts_with('/')
            && !argv[0].chars().any(char::is_control)
            && argv.iter().all(|part| {
                !part.is_empty() && part.len() <= MAX_ARG_BYTES && !part.as_bytes().contains(&0)
            }),
        "{field}_invalid"
    );
    Ok(())
}

fn validate_absolute_path(path: &str, field: &str) -> Result<()> {
    anyhow::ensure!(
        path.starts_with('/')
            && path.len() <= MAX_ARG_BYTES
            && !path.chars().any(char::is_control)
            && !path.split('/').any(|segment| matches!(segment, "." | "..")),
        "{field}_must_be_absolute"
    );
    Ok(())
}

fn validate_environment_key(key: &str) -> Result<()> {
    anyhow::ensure!(
        !key.is_empty()
            && key.len() <= 128
            && !key.as_bytes()[0].is_ascii_digit()
            && key
                .bytes()
                .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric()),
        "environment_key_invalid"
    );
    Ok(())
}

pub(crate) fn validate_network_adapter_definition(
    request: &UpsertNetworkAdapterDefinitionRequest,
) -> Result<()> {
    anyhow::ensure!(
        matches!(
            request.adapter_kind.as_str(),
            "runtime_tunnel" | "routing_cost"
        ),
        "network_adapter_kind_invalid"
    );
    validate_operator_name(&request.name, "network_adapter_name_invalid")?;
    anyhow::ensure!(
        request
            .description
            .as_deref()
            .is_none_or(|value| value.len() <= 4096),
        "network_adapter_description_invalid"
    );
    let object = request
        .definition
        .as_object()
        .context("network_adapter_definition_must_be_object")?;
    let allowed = match request.adapter_kind.as_str() {
        "runtime_tunnel" => &[
            "manager",
            "contract_version",
            "startup_command",
            "stop_command",
            "cleanup_command",
            "restart_command",
            "status_command",
            "traffic_limit_command",
        ][..],
        "routing_cost" => &["contract_version", "status_command", "update_command"][..],
        _ => unreachable!("validated adapter kind"),
    };
    anyhow::ensure!(
        object.keys().all(|key| allowed.contains(&key.as_str())),
        "network_adapter_definition_unknown_field"
    );
    anyhow::ensure!(
        object.get("contract_version").and_then(Value::as_u64) == Some(1),
        "network_adapter_contract_version_invalid"
    );
    let command = |field: &str, required: bool| -> Result<Option<PresetCommand>> {
        let Some(value) = object.get(field) else {
            anyhow::ensure!(!required, "network_adapter_{field}_required");
            return Ok(None);
        };
        let parsed: PresetCommand = serde_json::from_value(value.clone())
            .with_context(|| format!("network_adapter_{field}_invalid"))?;
        validate_preset_command(&parsed, field)?;
        Ok(Some(parsed))
    };
    if request.adapter_kind == "runtime_tunnel" {
        anyhow::ensure!(
            object.get("manager").and_then(Value::as_str) == Some("external_managed_adapter"),
            "network_adapter_manager_invalid"
        );
        command("status_command", true)?;
        let startup = command("startup_command", false)?;
        let restart = command("restart_command", false)?;
        let stop = command("stop_command", false)?;
        let cleanup = command("cleanup_command", false)?;
        command("traffic_limit_command", false)?;
        anyhow::ensure!(
            startup.is_some() || restart.is_some(),
            "network_adapter_start_command_required"
        );
        anyhow::ensure!(
            stop.is_some() || cleanup.is_some(),
            "network_adapter_remove_command_required"
        );
    } else {
        command("status_command", true)?;
        command("update_command", true)?;
    }
    Ok(())
}

pub(crate) fn validate_network_adapter_definition_view(
    definition: &NetworkAdapterDefinitionView,
) -> Result<()> {
    validate_network_adapter_definition(&UpsertNetworkAdapterDefinitionRequest {
        adapter_kind: definition.adapter_kind.clone(),
        name: definition.name.clone(),
        description: definition.description.clone(),
        definition: definition.definition.clone(),
    })
}

fn changed_definition_keys(current: &Value, candidate: &Value) -> Vec<String> {
    let current = current.as_object().cloned().unwrap_or_default();
    let candidate = candidate.as_object().cloned().unwrap_or_default();
    let keys = current
        .keys()
        .chain(candidate.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    keys.into_iter()
        .filter(|key| current.get(key) != candidate.get(key))
        .collect()
}

fn configuration_preset_audit_metadata(
    preset: &ConfigurationPresetView,
    client_ids: &[String],
) -> Value {
    serde_json::json!({
        "preset_id": preset.id,
        "behavior": preset.behavior,
        "name": preset.name,
        "kind": preset.kind,
        "target_clients": client_ids,
    })
}

fn network_adapter_audit_metadata(definition: &NetworkAdapterDefinitionView) -> Value {
    serde_json::json!({
        "definition_id": definition.id,
        "adapter_kind": definition.adapter_kind,
        "name": definition.name,
        "definition_hash": payload_hash(definition.definition.to_string().as_bytes()),
    })
}

fn configuration_audit_metadata(mut metadata: Value, operator: &AuthContext) -> Value {
    let fields = metadata
        .as_object_mut()
        .expect("configuration audit metadata must be an object");
    fields.insert("result".to_string(), serde_json::json!("succeeded"));
    fields.insert(
        "operator_id".to_string(),
        serde_json::json!(operator.operator.id),
    );
    fields.insert(
        "operator_username".to_string(),
        serde_json::json!(&operator.operator.username),
    );
    fields.insert(
        "operator_role".to_string(),
        serde_json::json!(&operator.operator.role),
    );
    fields.insert(
        "operator_session_id".to_string(),
        serde_json::json!(operator.audit_session_id()),
    );
    fields.insert(
        "origin_kind".to_string(),
        serde_json::json!("operator_request"),
    );
    fields.insert(
        "component".to_string(),
        serde_json::json!("configuration-controller"),
    );
    metadata
}

fn tunnel_plan_references_adapter(plan: &crate::model::TunnelPlanView, id: Uuid) -> bool {
    if plan.deleted_at.is_some() {
        return false;
    }
    let id = id.to_string();
    let runtime = &plan.plan.runtime_control;
    runtime.left_adapter_definition_id.as_deref() == Some(&id)
        || runtime.right_adapter_definition_id.as_deref() == Some(&id)
        || plan.plan.ospf.as_ref().is_some_and(|ospf| {
            ospf.left_adapter_definition_id.as_deref() == Some(&id)
                || ospf.right_adapter_definition_id.as_deref() == Some(&id)
        })
}

async fn postgres_tunnel_plan_references_adapter(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<bool> {
    let id = id.to_string();
    Ok(sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM tunnel_plans
            WHERE deleted_at IS NULL
              AND (
                plan->'runtime_control'->>'left_adapter_template_id' = $1
                OR plan->'runtime_control'->>'right_adapter_template_id' = $1
                OR plan->'ospf'->>'left_adapter_template_id' = $1
                OR plan->'ospf'->>'right_adapter_template_id' = $1
              )
        )
        "#,
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}

fn merge_json_object(target: &mut Value, patch: Value) -> Result<()> {
    let target = target
        .as_object_mut()
        .context("effective_configuration_target_not_object")?;
    for (key, value) in patch
        .as_object()
        .context("effective_configuration_patch_not_object")?
    {
        if let (Some(existing), Value::Object(_)) = (target.get_mut(key), value) {
            merge_json_object(existing, value.clone())?;
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn normalized_description(description: Option<&str>) -> Option<String> {
    description
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn validate_operator_name(name: &str, error: &str) -> Result<()> {
    let trimmed = name.trim();
    anyhow::ensure!(
        !trimmed.is_empty() && trimmed.len() <= 256 && !name.chars().any(char::is_control),
        "{error}"
    );
    Ok(())
}

fn configuration_preset_database_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.constraint())
        == Some("configuration_presets_name_idx")
    {
        anyhow::anyhow!("configuration_preset_duplicate")
    } else {
        error.into()
    }
}

fn network_adapter_database_error(error: sqlx::Error) -> anyhow::Error {
    if error
        .as_database_error()
        .and_then(|database_error| database_error.constraint())
        == Some("network_adapter_definitions_name_idx")
    {
        anyhow::anyhow!("network_adapter_definition_duplicate")
    } else {
        error.into()
    }
}

fn behavior_order(behavior: &str) -> usize {
    CONFIGURATION_BEHAVIORS
        .iter()
        .position(|candidate| *candidate == behavior)
        .unwrap_or(usize::MAX)
}

fn configuration_preset_from_row(row: sqlx::postgres::PgRow) -> Result<ConfigurationPresetView> {
    Ok(ConfigurationPresetView {
        id: row.try_get("id")?,
        behavior: row.try_get("behavior")?,
        name: row.try_get("name")?,
        kind: row.try_get("kind")?,
        is_default: row.try_get("is_default")?,
        description: row.try_get("description")?,
        definition: row.try_get::<sqlx::types::Json<Value>, _>("definition")?.0,
        effective_vps_count: row.try_get("effective_vps_count")?,
        override_vps_count: row.try_get("override_vps_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn network_adapter_definition_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<NetworkAdapterDefinitionView> {
    Ok(NetworkAdapterDefinitionView {
        id: row.try_get("id")?,
        adapter_kind: row.try_get("adapter_kind")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        definition: row.try_get::<sqlx::types::Json<Value>, _>("definition")?.0,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn insert_configuration_audit_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    action: &str,
    target: &str,
    metadata: Value,
    operator: &AuthContext,
) -> Result<()> {
    let command_hash = payload_hash(metadata.to_string().as_bytes());
    let metadata = configuration_audit_metadata(metadata, operator);
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(operator.operator.id)
    .bind(action)
    .bind(target)
    .bind(command_hash)
    .bind(sqlx::types::Json(metadata))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::AgentView, repository::MemoryState};

    #[test]
    fn all_default_presets_render_as_one_agent_runtime_config_without_nulls() {
        let defaults = system_configuration_presets()
            .into_iter()
            .filter(|preset| preset.is_default)
            .collect::<Vec<_>>();
        assert_eq!(defaults.len(), CONFIGURATION_BEHAVIORS.len());

        let mut sections = serde_json::json!({"version": 1});
        for preset in defaults {
            let rendered =
                render_configuration_preset_definition(preset.behavior, &preset.definition)
                    .unwrap();
            assert!(
                !contains_json_null(&rendered.sections),
                "{} rendered a JSON null",
                preset.behavior
            );
            merge_json_object(&mut sections, rendered.sections).unwrap();
        }

        let _: vpsman_common::AgentRuntimeConfig =
            serde_json::from_value(sections.clone()).unwrap();
        let toml = toml::to_string_pretty(&sections).unwrap();
        let _: vpsman_common::AgentRuntimeConfig = toml::from_str(&toml).unwrap();
    }

    #[test]
    fn operator_visible_names_reject_control_characters() {
        assert!(validate_operator_name("Daily checks", "invalid").is_ok());
        assert!(validate_operator_name("Daily\nchecks", "invalid").is_err());
        assert!(validate_operator_name("\t", "invalid").is_err());
    }

    #[test]
    fn preset_definitions_reject_missing_discriminators_and_unknown_fields() {
        assert!(
            validate_configuration_preset_definition("tunnel_traffic", &serde_json::json!({}))
                .is_err()
        );
        assert!(validate_configuration_preset_definition(
            "tunnel_traffic",
            &serde_json::json!({
                "source": "interface_counters",
                "unexpected": true
            })
        )
        .is_err());
        assert!(validate_configuration_preset_definition(
            "latency_probe",
            &serde_json::json!({
                "source": "linux_ping_preset",
                "unexpected": true
            })
        )
        .is_err());
        assert!(validate_configuration_preset_definition(
            "user_sessions",
            &serde_json::json!({
                "source": "linux_w_who_preset",
                "unexpected": true
            })
        )
        .is_err());
    }

    #[test]
    fn ospf_updater_presets_are_explicitly_unconfigured_or_fully_paired() {
        let default = system_configuration_presets()
            .into_iter()
            .find(|preset| preset.behavior == "ospf_update_command" && preset.is_default)
            .unwrap();
        assert!(parse_ospf_update_commands(&default.definition)
            .unwrap()
            .is_none());

        let configured = serde_json::json!({
            "contract_version": 1,
            "status_command": preset_command("/usr/bin/ospf-status"),
            "update_command": preset_command("/usr/bin/ospf-update")
        });
        let rendered =
            render_configuration_preset_definition("ospf_update_command", &configured).unwrap();
        let mut sections = serde_json::json!({"version": 1});
        merge_json_object(&mut sections, rendered.sections).unwrap();
        let runtime: vpsman_common::AgentRuntimeConfig = serde_json::from_value(sections).unwrap();
        assert_eq!(
            runtime.network.ospf_status_command.unwrap().argv,
            ["/usr/bin/ospf-status"]
        );
        assert_eq!(
            runtime.network.ospf_update_command.unwrap().argv,
            ["/usr/bin/ospf-update"]
        );

        assert!(validate_configuration_preset_definition(
            "ospf_update_command",
            &serde_json::json!({
                "contract_version": 1,
                "status_command": preset_command("/usr/bin/ospf-status"),
                "update_command": null
            })
        )
        .is_err());
    }

    #[test]
    fn preset_paths_reject_controls_and_unbounded_values() {
        assert!(validate_absolute_path("/proc", "proc_root").is_ok());
        assert!(validate_absolute_path("/proc\n", "proc_root").is_err());
        assert!(
            validate_absolute_path(&format!("/{}", "a".repeat(MAX_ARG_BYTES)), "proc_root")
                .is_err()
        );
    }

    #[tokio::test]
    async fn override_apply_rejects_a_changed_selection_origin() {
        let memory = MemoryState::default();
        memory.agents.write().await.push(test_agent("edge-a"));
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();
        let preset =
            create_test_traffic_preset(&repo, "vnStat A", "/usr/bin/vnstat", &operator).await;
        let preview = repo
            .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
                action: ConfigurationOverrideAction::Set,
                behavior: "tunnel_traffic".to_string(),
                preset_id: Some(preset.id),
                selector_expression: String::new(),
                target_client_ids: vec!["edge-a".to_string()],
            })
            .await
            .unwrap();
        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        memory.configuration_preset_overrides.write().await.push(
            ConfigurationPresetOverrideRecord {
                client_id: "edge-a".to_string(),
                behavior: "tunnel_traffic".to_string(),
                preset_id: preset.id,
                updated_at: unix_now().to_string(),
            },
        );

        let error = repo
            .apply_configuration_source_override(&preview, &operator)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("configuration_source_override_preview_stale"));
    }

    #[tokio::test]
    async fn override_preview_and_audit_retain_the_trimmed_selector() {
        let memory = MemoryState::default();
        memory.agents.write().await.push(test_agent("edge-a"));
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();
        let preset =
            create_test_traffic_preset(&repo, "vnStat selector", "/usr/bin/vnstat", &operator)
                .await;
        let preview = repo
            .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
                action: ConfigurationOverrideAction::Set,
                behavior: "tunnel_traffic".to_string(),
                preset_id: Some(preset.id),
                selector_expression: "  tag:edge  ".to_string(),
                target_client_ids: vec!["edge-a".to_string()],
            })
            .await
            .unwrap();
        assert_eq!(preview.selector_expression, "tag:edge");

        repo.apply_configuration_source_override(&preview, &operator)
            .await
            .unwrap();
        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        let audits = memory.audits.read().await;
        let applied = audits
            .iter()
            .find(|entry| entry.action == "configuration_source_override.applied")
            .unwrap();
        assert_eq!(applied.metadata["selector_expression"], "tag:edge");
    }

    #[tokio::test]
    async fn deleting_agent_releases_its_configuration_preset_override() {
        let memory = MemoryState::default();
        memory.agents.write().await.push(test_agent("edge-delete"));
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();
        let preset =
            create_test_traffic_preset(&repo, "Retired edge traffic", "/usr/bin/vnstat", &operator)
                .await;
        let preview = repo
            .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
                action: ConfigurationOverrideAction::Set,
                behavior: "tunnel_traffic".to_string(),
                preset_id: Some(preset.id),
                selector_expression: String::new(),
                target_client_ids: vec!["edge-delete".to_string()],
            })
            .await
            .unwrap();
        repo.apply_configuration_source_override(&preview, &operator)
            .await
            .unwrap();
        let assigned = repo
            .list_configuration_presets(Some("tunnel_traffic"))
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == preset.id)
            .unwrap();
        assert_eq!(assigned.override_vps_count, 1);

        repo.delete_agent("edge-delete", Some("retired"), &operator)
            .await
            .unwrap();

        let released = repo
            .list_configuration_presets(Some("tunnel_traffic"))
            .await
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.id == preset.id)
            .unwrap();
        assert_eq!(released.override_vps_count, 0);
        assert_eq!(released.effective_vps_count, 0);
        let stale_error = repo
            .apply_configuration_source_override(&preview, &operator)
            .await
            .unwrap_err();
        assert!(stale_error
            .to_string()
            .contains("configuration_source_override_preview_stale"));
        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        assert!(memory
            .configuration_preset_overrides
            .read()
            .await
            .is_empty());
        repo.delete_configuration_preset(preset.id, &operator)
            .await
            .unwrap();
        assert!(repo
            .configuration_preset_by_id(preset.id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn revoking_agent_key_releases_its_configuration_preset_override() {
        let memory = MemoryState::default();
        memory.agents.write().await.push(test_agent("edge-revoke"));
        memory
            .client_public_keys
            .write()
            .await
            .insert("edge-revoke".to_string(), vec![0x42; 32]);
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();
        let preset =
            create_test_traffic_preset(&repo, "Revoked edge traffic", "/usr/bin/vnstat", &operator)
                .await;
        let preview = repo
            .preview_configuration_source_override(&PreviewConfigurationSourceOverrideRequest {
                action: ConfigurationOverrideAction::Set,
                behavior: "tunnel_traffic".to_string(),
                preset_id: Some(preset.id),
                selector_expression: String::new(),
                target_client_ids: vec!["edge-revoke".to_string()],
            })
            .await
            .unwrap();
        repo.apply_configuration_source_override(&preview, &operator)
            .await
            .unwrap();

        repo.revoke_current_client_key("edge-revoke", Some("compromised"), &operator)
            .await
            .unwrap();

        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        assert!(memory
            .configuration_preset_overrides
            .read()
            .await
            .is_empty());
        let audits = memory.audits.read().await;
        let revocation = audits
            .iter()
            .find(|entry| entry.action == "client_key.revoked")
            .unwrap();
        assert_eq!(
            revocation.metadata["removed_configuration_preset_override_count"],
            1
        );
        drop(audits);
        repo.delete_configuration_preset(preset.id, &operator)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn existing_key_revocation_recovery_audits_configuration_override_cleanup() {
        let memory = MemoryState::default();
        let public_key = vec![0x43; 32];
        memory.agents.write().await.push(test_agent("edge-recover"));
        memory
            .client_public_keys
            .write()
            .await
            .insert("edge-recover".to_string(), public_key.clone());
        memory
            .client_key_revocations
            .write()
            .await
            .push(crate::model::ClientKeyRevocationView {
                id: Uuid::new_v4(),
                client_id: "edge-recover".to_string(),
                public_key_sha256_hex: crate::repository_key_lifecycle::public_key_sha256_hex(
                    &public_key,
                ),
                reason: Some("existing record".to_string()),
                revoked_by: Some(Uuid::nil()),
                created_at: unix_now().to_string(),
            });
        memory.configuration_preset_overrides.write().await.push(
            ConfigurationPresetOverrideRecord {
                client_id: "edge-recover".to_string(),
                behavior: "tunnel_traffic".to_string(),
                preset_id: Uuid::new_v4(),
                updated_at: unix_now().to_string(),
            },
        );
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();

        repo.revoke_current_client_key("edge-recover", Some("retry"), &operator)
            .await
            .unwrap();

        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        assert!(memory
            .configuration_preset_overrides
            .read()
            .await
            .is_empty());
        let audits = memory.audits.read().await;
        let recovery = audits
            .iter()
            .find(|entry| entry.action == "client_key.revoked")
            .unwrap();
        assert_eq!(recovery.metadata["recovered_existing_revocation"], true);
        assert_eq!(
            recovery.metadata["removed_configuration_preset_override_count"],
            1
        );
    }

    #[tokio::test]
    async fn targeted_source_reads_ignore_unrequested_client_overrides() {
        let memory = MemoryState::default();
        memory
            .agents
            .write()
            .await
            .extend([test_agent("edge-a"), test_agent("edge-b")]);
        memory.configuration_preset_overrides.write().await.push(
            ConfigurationPresetOverrideRecord {
                client_id: "edge-b".to_string(),
                behavior: "tunnel_traffic".to_string(),
                preset_id: Uuid::new_v4(),
                updated_at: unix_now().to_string(),
            },
        );
        let repo = Repository::Memory(memory);

        let rows = repo
            .list_configuration_sources_for_clients(&["edge-a".to_string()], "tunnel_traffic")
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].client_id, "edge-a");
        assert_eq!(rows[0].selection_origin, "system_default");
    }

    #[tokio::test]
    async fn preset_update_rejects_changed_affected_client_membership() {
        let memory = MemoryState::default();
        memory
            .agents
            .write()
            .await
            .extend([test_agent("edge-a"), test_agent("edge-b")]);
        let repo = Repository::Memory(memory);
        let operator = crate::tests::test_operator();
        let preset =
            create_test_traffic_preset(&repo, "vnStat B", "/usr/bin/vnstat", &operator).await;
        let Repository::Memory(memory) = &repo else {
            unreachable!()
        };
        memory.configuration_preset_overrides.write().await.push(
            ConfigurationPresetOverrideRecord {
                client_id: "edge-a".to_string(),
                behavior: "tunnel_traffic".to_string(),
                preset_id: preset.id,
                updated_at: unix_now().to_string(),
            },
        );
        let preview = repo
            .preview_configuration_preset_update(
                preset.id,
                &PreviewConfigurationPresetRequest {
                    description: preset.description.clone(),
                    definition: serde_json::json!({
                        "source": "vnstat",
                        "vnstat_argv": ["/opt/vnstat"]
                    }),
                },
            )
            .await
            .unwrap();
        assert_eq!(preview.affected_client_ids, vec!["edge-a".to_string()]);
        memory.configuration_preset_overrides.write().await.push(
            ConfigurationPresetOverrideRecord {
                client_id: "edge-b".to_string(),
                behavior: "tunnel_traffic".to_string(),
                preset_id: preset.id,
                updated_at: unix_now().to_string(),
            },
        );

        let error = repo
            .update_configuration_preset(preset.id, &preview, &operator)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("configuration_preset_preview_stale"));
    }

    #[tokio::test]
    async fn adapter_names_are_case_insensitive_and_kind_is_immutable() {
        let repo = Repository::Memory(MemoryState::default());
        let operator = crate::tests::test_operator();
        let created = repo
            .create_network_adapter_definition(&runtime_adapter_request("WireGuard"), &operator)
            .await
            .unwrap();

        let duplicate = repo
            .create_network_adapter_definition(&runtime_adapter_request("wireguard"), &operator)
            .await
            .unwrap_err();
        assert!(duplicate
            .to_string()
            .contains("network_adapter_definition_duplicate"));

        let kind_change = repo
            .update_network_adapter_definition(
                created.id,
                &UpsertNetworkAdapterDefinitionRequest {
                    adapter_kind: "routing_cost".to_string(),
                    name: created.name,
                    description: None,
                    definition: serde_json::json!({
                        "contract_version": 1,
                        "status_command": preset_command("/usr/bin/status"),
                        "update_command": preset_command("/usr/bin/update")
                    }),
                },
                &operator,
            )
            .await
            .unwrap_err();
        assert!(kind_change
            .to_string()
            .contains("network_adapter_definition_kind_immutable"));
    }

    #[tokio::test]
    async fn retired_tunnel_plan_releases_its_adapter_definitions() {
        let repo = Repository::Memory(MemoryState::default());
        let operator = crate::tests::test_operator();
        let left = repo
            .create_network_adapter_definition(&runtime_adapter_request("Runtime left"), &operator)
            .await
            .unwrap();
        let right = repo
            .create_network_adapter_definition(&runtime_adapter_request("Runtime right"), &operator)
            .await
            .unwrap();
        let mut input = crate::tests_network::test_plan_input(
            vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter,
            false,
        );
        input.runtime_control.left_adapter_definition_id = Some(left.id.to_string());
        input.runtime_control.right_adapter_definition_id = Some(right.id.to_string());
        let plan = vpsman_common::plan_tunnel(&input).unwrap();
        let saved = repo
            .record_tunnel_plan(&input, &plan, false, &operator)
            .await
            .unwrap();
        repo.delete_tunnel_plan(saved.id, saved.revision, &operator)
            .await
            .unwrap();

        repo.delete_network_adapter_definition(left.id, &operator)
            .await
            .unwrap();
        assert!(repo
            .network_adapter_definition_by_id(left.id, None)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn tunnel_plan_persistence_rejects_missing_adapter_definitions() {
        let repo = Repository::Memory(MemoryState::default());
        let input = crate::tests_network::test_plan_input(
            vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter,
            false,
        );
        let plan = vpsman_common::plan_tunnel(&input).unwrap();

        let error = repo
            .record_tunnel_plan(&input, &plan, false, &crate::tests::test_operator())
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("tunnel_plan_adapter_definition_unavailable"));
    }

    async fn create_test_traffic_preset(
        repo: &Repository,
        name: &str,
        executable: &str,
        operator: &AuthContext,
    ) -> ConfigurationPresetView {
        repo.create_configuration_preset(
            &CreateConfigurationPresetRequest {
                behavior: "tunnel_traffic".to_string(),
                name: name.to_string(),
                description: None,
                definition: serde_json::json!({
                    "source": "vnstat",
                    "vnstat_argv": [executable]
                }),
            },
            operator,
        )
        .await
        .unwrap()
    }

    fn runtime_adapter_request(name: &str) -> UpsertNetworkAdapterDefinitionRequest {
        UpsertNetworkAdapterDefinitionRequest {
            adapter_kind: "runtime_tunnel".to_string(),
            name: name.to_string(),
            description: None,
            definition: serde_json::json!({
                "manager": "external_managed_adapter",
                "contract_version": 1,
                "startup_command": preset_command("/usr/bin/start"),
                "cleanup_command": preset_command("/usr/bin/cleanup"),
                "status_command": preset_command("/usr/bin/status")
            }),
        }
    }

    fn preset_command(executable: &str) -> Value {
        serde_json::json!({
            "argv": [executable],
            "max_timeout_secs": 10,
            "max_output_bytes": 16384
        })
    }

    fn test_agent(client_id: &str) -> AgentView {
        AgentView {
            id: client_id.to_string(),
            display_name: client_id.to_string(),
            status: "online".to_string(),
            tags: Vec::new(),
            registration_ip: None,
            last_ip: None,
            last_seen_at: None,
            arch: None,
            internal_build_number: 1,
            process_incarnation_id: None,
            stale_since: None,
            stale_reason: None,
            capabilities: vpsman_common::AgentCapabilitySnapshot::default(),
        }
    }

    fn contains_json_null(value: &Value) -> bool {
        match value {
            Value::Null => true,
            Value::Array(values) => values.iter().any(contains_json_null),
            Value::Object(values) => values.values().any(contains_json_null),
            _ => false,
        }
    }
}
