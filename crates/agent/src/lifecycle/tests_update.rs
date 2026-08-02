use std::{fs, os::unix::fs::PermissionsExt};

use super::{
    agent_asset_name, candidate_version_status, check_and_stage_update, normalize_sha256,
    parse_artifact_url, sha256_hex, stage_update_artifact, update_http_url_allowed,
    update_redirect_url_allowed, CandidateVersionStatus, CheckStageInput, UpdateStageInput,
};
use reqwest::Url;

#[test]
fn parses_artifact_urls_and_rejects_remote_http() {
    assert!(parse_artifact_url("https://updates.example/vpsman-agent").is_ok());
    assert!(parse_artifact_url("http://127.0.0.1:8080/vpsman-agent").is_ok());
    assert!(parse_artifact_url("file:///tmp/vpsman-agent").is_ok());
    assert!(parse_artifact_url("http://updates.example/vpsman-agent").is_err());
}

#[test]
fn normalizes_sha256_hex() {
    assert_eq!(normalize_sha256(&"AA".repeat(32)).unwrap(), "aa".repeat(32));
    assert!(normalize_sha256("not-a-hash").is_err());
}

#[test]
fn classifies_candidate_update_versions_conservatively() {
    assert_eq!(
        candidate_version_status("1.2.3", "1.2.4"),
        CandidateVersionStatus::Newer
    );
    assert_eq!(
        candidate_version_status("1.2.3", "1.2.3"),
        CandidateVersionStatus::Current
    );
    assert_eq!(
        candidate_version_status("1.2.3", "1.2.2"),
        CandidateVersionStatus::DowngradeBlocked
    );
    assert_eq!(
        candidate_version_status("1.2.3", "dev-build"),
        CandidateVersionStatus::NotOrderable
    );
    assert_eq!(
        candidate_version_status("dev-build", "1.2.4"),
        CandidateVersionStatus::NotOrderable
    );
}

#[tokio::test]
async fn stages_file_artifact_after_hash_verification() {
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let artifact = dir.join("vpsman-agent-new");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(&artifact, b"new-agent").unwrap();
    let hash = sha256_hex(b"new-agent");
    let output = stage_update_artifact(UpdateStageInput {
        job_id: uuid::Uuid::new_v4(),
        artifact_url: &format!("file://{}", artifact.display()),
        expected_sha256_hex: &hash,
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
    })
    .await
    .unwrap();

    let staged = dir.join("vpsman-agent.next");
    let rollback = dir.join("vpsman-agent.rollback");
    assert_eq!(fs::read(staged).unwrap(), b"new-agent");
    assert_eq!(fs::read(rollback).unwrap(), b"old-agent");
    assert_eq!(
        fs::metadata(dir.join("vpsman-agent.next"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let status: serde_json::Value = serde_json::from_slice(&output.data).unwrap();
    assert_eq!(status["status"], "staged");
    assert!(status.get("artifact_url").is_none());

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn update_check_uses_embedded_release_version_for_current_detection() {
    let Some(_) = agent_asset_name() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let manifest_path = dir.join("version.json");
    let current_version = crate::build_info::agent_release_version();
    fs::write(&current, b"current-agent").unwrap();
    fs::write(
        &manifest_path,
        serde_json::json!({
            "schema_version": 3,
            "project": "vpsman",
            "version": current_version,
            "tag": format!("v{current_version}"),
            "assets": [],
        })
        .to_string(),
    )
    .unwrap();

    let result = check_and_stage_update(CheckStageInput {
        job_id: uuid::Uuid::new_v4(),
        version_url: &format!("file://{}", manifest_path.display()),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
        verification_tx: None,
    })
    .await
    .unwrap();

    assert_eq!(result.staged_sha256_hex, None);
    assert!(!dir.join("vpsman-agent.next").exists());
    let status: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(status["status"], "current");
    assert_eq!(status["current_version"], current_version);
    assert_eq!(status["candidate_version"], current_version);

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn update_check_blocks_older_manifest_without_staging() {
    let Some(_) = agent_asset_name() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let manifest_path = dir.join("version.json");
    let current_version = semver::Version::parse(crate::build_info::agent_release_version())
        .expect("test release version is semver");
    let older_version = if current_version.minor > 0 {
        format!("{}.{}.0", current_version.major, current_version.minor - 1)
    } else if current_version.patch > 0 {
        format!(
            "{}.{}.{}",
            current_version.major,
            current_version.minor,
            current_version.patch - 1
        )
    } else if current_version.major > 0 {
        format!("{}.0.0", current_version.major - 1)
    } else {
        let _ = fs::remove_dir_all(dir);
        return;
    };
    fs::write(&current, b"current-agent").unwrap();
    fs::write(
        &manifest_path,
        serde_json::json!({
            "schema_version": 3,
            "project": "vpsman",
            "version": older_version,
            "tag": format!("v{older_version}"),
            "assets": [],
        })
        .to_string(),
    )
    .unwrap();

    let result = check_and_stage_update(CheckStageInput {
        job_id: uuid::Uuid::new_v4(),
        version_url: &format!("file://{}", manifest_path.display()),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
        verification_tx: None,
    })
    .await
    .unwrap();

    assert_eq!(result.staged_sha256_hex, None);
    assert!(!dir.join("vpsman-agent.next").exists());
    let status: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(status["status"], "downgrade_blocked");
    assert_eq!(status["candidate_version"], older_version);

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn update_check_rejects_non_semver_manifest_without_staging() {
    let Some(_) = agent_asset_name() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let manifest_path = dir.join("version.json");
    fs::write(&current, b"current-agent").unwrap();
    fs::write(
        &manifest_path,
        serde_json::json!({
            "schema_version": 3,
            "project": "vpsman",
            "version": "dev-build",
            "tag": "dev-build",
            "assets": [],
        })
        .to_string(),
    )
    .unwrap();

    let result = check_and_stage_update(CheckStageInput {
        job_id: uuid::Uuid::new_v4(),
        version_url: &format!("file://{}", manifest_path.display()),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
        verification_tx: None,
    })
    .await
    .unwrap();

    assert_eq!(result.staged_sha256_hex, None);
    assert!(!dir.join("vpsman-agent.next").exists());
    let status: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(status["status"], "version_not_orderable");
    assert_eq!(status["candidate_version"], "dev-build");

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn update_check_uses_explicit_manifest_download_urls() {
    let Some(asset_name) = agent_asset_name() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    let manifest_dir = dir.join("manifest");
    let asset_dir = dir.join("assets");
    fs::create_dir_all(&manifest_dir).unwrap();
    fs::create_dir_all(&asset_dir).unwrap();
    let current = dir.join("vpsman-agent");
    let artifact = asset_dir.join(asset_name);
    let manifest_path = manifest_dir.join("version.json");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(&artifact, b"new-agent").unwrap();
    let artifact_sha = sha256_hex(b"new-agent");
    let artifact_url = format!("file://{}", artifact.display());
    fs::write(
        &manifest_path,
        serde_json::json!({
            "schema_version": 3,
            "project": "vpsman",
            "version": "999.0.0",
            "tag": "v999.0.0",
            "commit": "unit-test",
            "assets": [
                {
                    "name": asset_name,
                    "download_url": artifact_url.clone(),
                }
            ]
        })
        .to_string(),
    )
    .unwrap();

    let result = check_and_stage_update(CheckStageInput {
        job_id: uuid::Uuid::new_v4(),
        version_url: &format!("file://{}", manifest_path.display()),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
        verification_tx: None,
    })
    .await
    .unwrap();

    assert_eq!(
        result.staged_sha256_hex.as_deref(),
        Some(artifact_sha.as_str())
    );
    assert_eq!(
        fs::read(dir.join("vpsman-agent.next")).unwrap(),
        b"new-agent"
    );
    let status: serde_json::Value = serde_json::from_slice(&result.outputs[0].data).unwrap();
    assert_eq!(status["status"], "staging");
    assert_eq!(status["artifact_url"], artifact_url);

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn update_check_rejects_legacy_name_only_manifest() {
    let Some(asset_name) = agent_asset_name() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let manifest_path = dir.join("version.json");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(
        &manifest_path,
        serde_json::json!({
            "schema_version": 1,
            "project": "vpsman",
            "version": "999.0.0",
            "tag": "v999.0.0",
            "assets": [asset_name],
        })
        .to_string(),
    )
    .unwrap();

    let error = match check_and_stage_update(CheckStageInput {
        job_id: uuid::Uuid::new_v4(),
        version_url: &format!("file://{}", manifest_path.display()),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
        verification_tx: None,
    })
    .await
    {
        Ok(_) => panic!("legacy update manifest unexpectedly passed"),
        Err(error) => error.to_string(),
    };

    assert!(error.contains("unsupported update manifest schema 1"));

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn rejects_hash_mismatch_before_writing_staged_artifact() {
    let dir = std::env::temp_dir().join(format!("vpsman-update-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let current = dir.join("vpsman-agent");
    let artifact = dir.join("vpsman-agent-new");
    fs::write(&current, b"old-agent").unwrap();
    fs::write(&artifact, b"new-agent").unwrap();

    assert!(stage_update_artifact(UpdateStageInput {
        job_id: uuid::Uuid::new_v4(),
        artifact_url: &format!("file://{}", artifact.display()),
        expected_sha256_hex: &"00".repeat(32),
        current_exe: &current,
        cancel_token: &crate::command_worker::CommandCancelToken::default(),
    })
    .await
    .is_err());
    assert!(!dir.join("vpsman-agent.next").exists());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn redirect_policy_accepts_https_and_local_http_only() {
    let https = Url::parse("https://updates.example/vpsman-agent").unwrap();
    let local_http = Url::parse("http://127.0.0.1:8080/vpsman-agent").unwrap();
    let remote_http = Url::parse("http://updates.example/vpsman-agent").unwrap();
    assert!(update_http_url_allowed(&https));
    assert!(update_http_url_allowed(&local_http));
    assert!(!update_http_url_allowed(&remote_http));
    assert!(update_redirect_url_allowed(Some(&https), &https));
    assert!(update_redirect_url_allowed(Some(&local_http), &local_http));
    assert!(!update_redirect_url_allowed(Some(&https), &local_http));
    assert!(!update_redirect_url_allowed(
        Some(&local_http),
        &remote_http
    ));
}
