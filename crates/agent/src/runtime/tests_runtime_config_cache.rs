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
    assert_eq!(first_loaded.version, 10);

    let second = AgentRuntimeConfig {
        version: 11,
        telemetry_interval_secs: 30,
        ..AgentRuntimeConfig::default()
    };
    cache.store(&second).await.unwrap();
    let loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(loaded, second);

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
async fn rejects_a_runtime_config_cache_from_another_schema() {
    let root = std::env::temp_dir().join(format!(
        "vpsman-agent-runtime-config-cache-schema-{}",
        Uuid::new_v4()
    ));
    let cache = RuntimeConfigCache::open_at(root.clone()).await.unwrap();
    let config_json = serde_json::to_string(&AgentRuntimeConfig::default()).unwrap();
    let record = RuntimeConfigCacheRecord {
        schema_version: CACHE_SCHEMA_VERSION - 1,
        content_hash: payload_hash(config_json.as_bytes()),
        config_json,
    };
    tokio::fs::write(cache.cache_path(), serde_json::to_vec(&record).unwrap())
        .await
        .unwrap();

    assert!(cache
        .load()
        .await
        .unwrap_err()
        .to_string()
        .contains("unsupported runtime config cache schema"));
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
    assert_eq!(loaded.version, 42);
    let _ = tokio::fs::remove_dir_all(root).await;
}
