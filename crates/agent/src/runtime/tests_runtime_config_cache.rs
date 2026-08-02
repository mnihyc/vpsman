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
        display_name: "edge-a".to_string(),
        ..AgentRuntimeConfig::default()
    };
    cache.store(&first).await.unwrap();
    assert_eq!(cache.load().await.unwrap().unwrap().version, 10);

    let second = AgentRuntimeConfig {
        version: 11,
        display_name: "edge-b".to_string(),
        ..AgentRuntimeConfig::default()
    };
    cache.store(&second).await.unwrap();
    let loaded = cache.load().await.unwrap().unwrap();
    assert_eq!(loaded.version, 11);
    assert_eq!(loaded.display_name, "edge-b");

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
        "display_name": "edge-a",
        "future_runtime_section": { "enabled": true }
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
    assert_eq!(loaded.display_name, "edge-a");
    let _ = tokio::fs::remove_dir_all(root).await;
}
