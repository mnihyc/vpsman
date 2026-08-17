use super::*;

#[tokio::test]
async fn stores_and_replaces_last_accepted_runtime_config() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-agent-runtime-config-cache-{}",
        Uuid::new_v4()
    ));
    let cache = RuntimeConfigCache::open_at(root.clone()).await.unwrap();
    assert!(cache.load().await.unwrap().is_none());

    let first = AgentRuntimeConfig {
        version: 10,
        ..AgentRuntimeConfig::default()
    };
    cache.store(&first).await.unwrap();
    let first_loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(first_loaded.config.version, 10);
    assert!(!first_loaded.requires_authoritative_runtime_config_sync);

    let second = AgentRuntimeConfig {
        version: 11,
        telemetry_interval_secs: 30,
        ..AgentRuntimeConfig::default()
    };
    cache.store(&second).await.unwrap();
    let loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(loaded.config, second);
    assert!(!loaded.requires_authoritative_runtime_config_sync);

    let record: RuntimeConfigCacheRecord =
        serde_json::from_slice(&tokio::fs::read(cache.cache_path()).await.unwrap()).unwrap();
    let stored: serde_json::Value = serde_json::from_str(&record.config_json).unwrap();
    assert!(stored.get("display_name").is_none());
    assert!(stored.get("tags").is_none());

    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn rejects_a_corrupted_runtime_config_cache() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-agent-runtime-config-cache-corrupt-{}",
        Uuid::new_v4()
    ));
    let cache = RuntimeConfigCache::open_at(root.clone()).await.unwrap();
    let config = AgentRuntimeConfig {
        version: 10,
        ..AgentRuntimeConfig::default()
    };
    cache.store(&config).await.unwrap();

    let path = cache.cache_path();
    let mut value: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&path).await.unwrap()).unwrap();
    value["content_hash"] = serde_json::Value::String("00".repeat(32));
    tokio::fs::write(&path, serde_json::to_vec(&value).unwrap())
        .await
        .unwrap();

    assert!(cache
        .load()
        .await
        .unwrap_err()
        .to_string()
        .contains("hash mismatch"));
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn accepts_intact_cached_json_with_additive_future_fields() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-agent-runtime-config-cache-future-{}",
        Uuid::new_v4()
    ));
    let cache = RuntimeConfigCache::open_at(root.clone()).await.unwrap();
    let config_json = serde_json::json!({
        "version": 42,
        "future_runtime_section": {
            "enabled": true,
            "display_name": "nested-future-value"
        }
    })
    .to_string();
    let record = RuntimeConfigCacheRecord {
        schema_version: CACHE_SCHEMA_VERSION,
        content_hash: payload_hash(config_json.as_bytes()),
        config_json,
    };
    tokio::fs::write(cache.cache_path(), serde_json::to_vec(&record).unwrap())
        .await
        .unwrap();

    let loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(loaded.config.version, 42);
    assert!(!loaded.requires_authoritative_runtime_config_sync);
    let _ = tokio::fs::remove_dir_all(root).await;
}

#[tokio::test]
async fn legacy_identity_keys_preserve_runtime_state_and_force_sync_without_rewriting_cache() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-agent-runtime-config-cache-legacy-{}",
        Uuid::new_v4()
    ));
    let cache = RuntimeConfigCache::open_at(root.clone()).await.unwrap();
    let mut expected = AgentRuntimeConfig {
        version: 41,
        telemetry_interval_secs: 75,
        ..AgentRuntimeConfig::default()
    };
    expected.network.apply_enabled = true;
    expected.network.runtime_reconcile_enabled = true;
    expected.network.root_dir = "/legacy-runtime-root".to_string();

    let mut raw_config = serde_json::to_value(&expected).unwrap();
    raw_config["display_name"] = serde_json::json!("legacy-edge");
    raw_config["tags"] = serde_json::json!(["legacy", "prod"]);
    let config_json = serde_json::to_string(&raw_config).unwrap();
    let record = RuntimeConfigCacheRecord {
        schema_version: CACHE_SCHEMA_VERSION,
        content_hash: payload_hash(config_json.as_bytes()),
        config_json,
    };
    let original_bytes = serde_json::to_vec(&record).unwrap();
    tokio::fs::write(cache.cache_path(), &original_bytes)
        .await
        .unwrap();

    let loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(loaded.config, expected);
    assert!(loaded.requires_authoritative_runtime_config_sync);
    assert_eq!(
        tokio::fs::read(cache.cache_path()).await.unwrap(),
        original_bytes,
        "legacy cache must not be rewritten until a runtime sync is accepted"
    );

    cache.store(&loaded.config).await.unwrap();
    let canonical = cache.load().await.unwrap().unwrap();
    assert_eq!(canonical.config, expected);
    assert!(!canonical.requires_authoritative_runtime_config_sync);

    let _ = tokio::fs::remove_dir_all(root).await;
}
