use std::{
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::AsyncReadExt,
    sync::mpsc,
    time::{self, Duration},
};
use vpsman_common::{
    payload_hash, validate_absolute_file_path, AgentConfig, CommandOutput, OutputStream,
};
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::command_worker::{run_cancelable, CommandCancelToken};
use crate::telemetry::unix_now;

pub(crate) const BACKUP_ARTIFACT_FORMAT: &str = "vpsman.backup_artifact.v1";
pub(crate) const BACKUP_ARCHIVE_FORMAT: &str = "vpsman.backup_tar.v1";
pub(crate) const BACKUP_ARCHIVE_MANIFEST_PATH: &str = "vpsman-backup/manifest.json";
const MAX_BACKUP_PATHS: usize = 64;

struct LimitedVecWriter {
    inner: Vec<u8>,
    written: u64,
    max_bytes: u64,
}

impl LimitedVecWriter {
    fn new(max_bytes: u64) -> Self {
        Self {
            inner: Vec::new(),
            written: 0,
            max_bytes,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.inner
    }
}

impl Write for LimitedVecWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(buf.len() as u64)
            .ok_or_else(|| io::Error::other("backup archive size overflow"))?;
        if next > self.max_bytes {
            return Err(io::Error::other(
                "backup archive exceeds configured plaintext byte limit",
            ));
        }
        self.inner.extend_from_slice(buf);
        self.written = next;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct BackupArchive {
    pub(crate) format: String,
    pub(crate) client_id: String,
    pub(crate) created_unix: u64,
    pub(crate) files: Vec<BackupFileEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct BackupFileEntry {
    pub(crate) path: String,
    pub(crate) source: BackupFileSource,
    pub(crate) tar_path: String,
    pub(crate) mode: u32,
    pub(crate) size_bytes: u64,
    pub(crate) sha256_hex: String,
    pub(crate) mtime_unix: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BackupFileSource {
    SelectedPath,
    AgentConfig,
}

struct BackupFilePayload {
    entry: BackupFileEntry,
    data: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
struct EncryptedBackupArtifact {
    format: String,
    version: u32,
    cipher: String,
    compression: String,
    client_id: String,
    created_unix: u64,
    recipient_public_key_sha256_hex: String,
    ephemeral_public_key_hex: String,
    nonce_hex: String,
    ciphertext_sha256_hex: String,
    ciphertext_base64: String,
}

pub(crate) struct BackupCommandInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) config: &'a AgentConfig,
    pub(crate) config_path: &'a Path,
    pub(crate) paths: &'a [String],
    pub(crate) include_config: bool,
    pub(crate) recipient_public_key_hex: Option<&'a str>,
    pub(crate) output_tx: Option<mpsc::Sender<CommandOutput>>,
    pub(crate) timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
}

pub(crate) async fn execute_backup_command(
    input: BackupCommandInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let BackupCommandInput {
        job_id,
        config,
        config_path,
        paths,
        include_config,
        recipient_public_key_hex,
        output_tx,
        timeout_secs,
        cancel_token,
    } = input;
    run_cancelable("backup", cancel_token.clone(), async move {
        time::timeout(
            Duration::from_secs(timeout_secs.max(1)),
            create_encrypted_backup(
                job_id,
                config,
                config_path,
                paths,
                include_config,
                recipient_public_key_hex,
                output_tx,
                cancel_token,
            ),
        )
        .await
        .context("backup timed out")?
    })
    .await
}

async fn create_encrypted_backup(
    job_id: uuid::Uuid,
    config: &AgentConfig,
    config_path: &Path,
    paths: &[String],
    include_config: bool,
    recipient_public_key_hex: Option<&str>,
    output_tx: Option<mpsc::Sender<CommandOutput>>,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    cancel_token.check("backup")?;
    validate_backup_scope(paths, include_config)?;
    let recipient_public_key_hex = recipient_public_key_hex
        .or(config.backup.recipient_public_key_hex.as_deref())
        .context("backup recipient public key is not configured")?;
    cancel_token.check("backup")?;
    let recipient_public_key = decode_public_key(recipient_public_key_hex)?;
    let files = collect_backup_files(
        config_path,
        paths,
        include_config,
        config.backup.max_plaintext_bytes,
        &cancel_token,
    )
    .await?;
    cancel_token.check("backup")?;
    let file_count = files.len();
    let created_unix = unix_now();
    let archive = BackupArchive {
        format: BACKUP_ARCHIVE_FORMAT.to_string(),
        client_id: config.client_id.clone(),
        created_unix,
        files: files.iter().map(|file| file.entry.clone()).collect(),
    };
    let plaintext = encode_backup_tar_archive(&archive, &files, config.backup.max_plaintext_bytes)
        .context("failed to encode backup tar archive")?;
    cancel_token.check("backup")?;
    if plaintext.len() as u64 > config.backup.max_plaintext_bytes {
        anyhow::bail!(
            "backup archive exceeds plaintext limit: {} > {} bytes",
            plaintext.len(),
            config.backup.max_plaintext_bytes
        );
    }
    let compressed = lz4_flex::compress_prepend_size(&plaintext);
    cancel_token.check("backup")?;
    let artifact = encrypt_backup_artifact(config, &recipient_public_key, &compressed)?;
    cancel_token.check("backup")?;
    let artifact_bytes =
        serde_json::to_vec(&artifact).context("failed to encode encrypted backup artifact")?;
    let (mut outputs, streamed, chunk_count, chunk_bytes, artifact_sha256_hex) =
        if let Some(output_tx) = output_tx {
            let summary = super::file_pull::stream_buffered_payload_output(
                job_id,
                OutputStream::Stdout,
                &artifact_bytes,
                output_tx,
                "backup artifact output receiver dropped",
            )
            .await?;
            cancel_token.check("backup")?;
            (
                Vec::new(),
                true,
                summary.chunk_count,
                summary.chunk_bytes,
                summary.sha256_hex,
            )
        } else {
            (
                super::file_pull::chunked_output(job_id, OutputStream::Stdout, &artifact_bytes),
                false,
                artifact_bytes
                    .chunks(super::file_pull::COMMAND_OUTPUT_CHUNK_BYTES)
                    .count() as u64,
                super::file_pull::COMMAND_OUTPUT_CHUNK_BYTES,
                payload_hash(&artifact_bytes),
            )
        };
    let status = serde_json::json!({
        "type": "backup",
        "format": BACKUP_ARTIFACT_FORMAT,
        "encrypted": true,
        "compression": "lz4-size-prepended",
        "cipher": "x25519-chacha20poly1305",
        "archive_format": BACKUP_ARCHIVE_FORMAT,
        "paths": paths,
        "include_config": include_config,
        "file_count": file_count,
        "artifact_size_bytes": artifact_bytes.len(),
        "ciphertext_sha256_hex": artifact.ciphertext_sha256_hex,
        "artifact_sha256_hex": artifact_sha256_hex,
        "recipient_public_key_sha256_hex": artifact.recipient_public_key_sha256_hex,
        "ephemeral_public_key_hex": artifact.ephemeral_public_key_hex,
        "nonce_hex": artifact.nonce_hex,
        "chunk_bytes": chunk_bytes,
        "chunk_count": chunk_count,
        "streamed": streamed,
    });
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&status)?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

async fn collect_backup_files(
    config_path: &Path,
    paths: &[String],
    include_config: bool,
    max_plaintext_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<Vec<BackupFilePayload>> {
    let mut files = Vec::new();
    let mut total_bytes = 0_u64;
    for (index, path) in paths.iter().enumerate() {
        cancel_token.check("backup")?;
        files.push(
            read_backup_file(
                Path::new(path),
                path,
                BackupFileSource::SelectedPath,
                index,
                &mut total_bytes,
                max_plaintext_bytes,
                cancel_token,
            )
            .await?,
        );
    }
    if include_config {
        cancel_token.check("backup")?;
        files.push(
            read_backup_file(
                config_path,
                "vpsman:agent_config",
                BackupFileSource::AgentConfig,
                files.len(),
                &mut total_bytes,
                max_plaintext_bytes,
                cancel_token,
            )
            .await?,
        );
    }
    Ok(files)
}

async fn read_backup_file(
    path: &Path,
    archive_path: &str,
    source: BackupFileSource,
    tar_index: usize,
    total_bytes: &mut u64,
    max_plaintext_bytes: u64,
    cancel_token: &CommandCancelToken,
) -> Result<BackupFilePayload> {
    cancel_token.check("backup")?;
    if matches!(source, BackupFileSource::SelectedPath) {
        validate_absolute_file_path(archive_path)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to stat backup path {}", path.display()))?;
    cancel_token.check("backup")?;
    if !metadata.is_file() {
        anyhow::bail!("backup path is not a regular file: {}", path.display());
    }
    let remaining = max_plaintext_bytes.saturating_sub(*total_bytes);
    if metadata.len() > remaining {
        anyhow::bail!(
            "backup scope exceeds plaintext limit: {} > {} bytes",
            (*total_bytes).saturating_add(metadata.len()),
            max_plaintext_bytes
        );
    }
    let data = read_backup_file_bounded(path, remaining).await?;
    *total_bytes = total_bytes
        .checked_add(data.len() as u64)
        .context("backup size overflow")?;
    cancel_token.check("backup")?;
    let mtime_unix = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    Ok(BackupFilePayload {
        entry: BackupFileEntry {
            path: archive_path.to_string(),
            source,
            tar_path: format!("vpsman-backup/files/{tar_index:04}.bin"),
            mode: metadata.permissions().mode() & 0o777,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(&data),
            mtime_unix,
        },
        data,
    })
}

fn encode_backup_tar_archive(
    archive: &BackupArchive,
    files: &[BackupFilePayload],
    max_plaintext_bytes: u64,
) -> Result<Vec<u8>> {
    let mut builder = tar::Builder::new(LimitedVecWriter::new(max_plaintext_bytes));
    let manifest =
        serde_json::to_vec(archive).context("failed to encode backup archive manifest")?;
    append_tar_bytes(
        &mut builder,
        BACKUP_ARCHIVE_MANIFEST_PATH,
        0o600,
        archive.created_unix,
        &manifest,
    )?;
    for file in files {
        append_tar_bytes(
            &mut builder,
            &file.entry.tar_path,
            file.entry.mode,
            file.entry.mtime_unix.unwrap_or(archive.created_unix),
            &file.data,
        )?;
    }
    builder
        .finish()
        .context("failed to finish backup tar archive")?;
    Ok(builder
        .into_inner()
        .context("failed to collect backup tar archive")?
        .into_inner())
}

async fn read_backup_file_bounded(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open backup path {}", path.display()))?;
    let mut data = Vec::with_capacity((max_bytes.min(16 * 1024)) as usize);
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .with_context(|| format!("failed to read backup path {}", path.display()))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("backup file size overflow")?;
        if total > max_bytes {
            anyhow::bail!("backup file exceeds remaining plaintext limit while reading");
        }
        data.extend_from_slice(&buffer[..read]);
    }
    Ok(data)
}

fn append_tar_bytes<W: Write>(
    builder: &mut tar::Builder<W>,
    path: &str,
    mode: u32,
    mtime_unix: u64,
    bytes: &[u8],
) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_mtime(mtime_unix);
    header.set_cksum();
    builder
        .append_data(&mut header, path, bytes)
        .with_context(|| format!("failed to append backup tar entry {path}"))
}

fn validate_backup_scope(paths: &[String], include_config: bool) -> Result<()> {
    if paths.len() > MAX_BACKUP_PATHS {
        anyhow::bail!("backup path limit exceeded");
    }
    if !include_config && paths.is_empty() {
        anyhow::bail!("backup scope is empty");
    }
    for path in paths {
        validate_absolute_file_path(path).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    Ok(())
}

fn encrypt_backup_artifact(
    config: &AgentConfig,
    recipient_public_key: &PublicKey,
    compressed_archive: &[u8],
) -> Result<EncryptedBackupArtifact> {
    let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared_secret = ephemeral_secret.diffie_hellman(recipient_public_key);
    let key_bytes = backup_encryption_key(
        shared_secret.as_bytes(),
        recipient_public_key.as_bytes(),
        ephemeral_public.as_bytes(),
    );
    let mut nonce = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), compressed_archive)
        .map_err(|_| anyhow::anyhow!("failed to encrypt backup artifact"))?;
    Ok(EncryptedBackupArtifact {
        format: BACKUP_ARTIFACT_FORMAT.to_string(),
        version: 1,
        cipher: "x25519-chacha20poly1305".to_string(),
        compression: "lz4-size-prepended".to_string(),
        client_id: config.client_id.clone(),
        created_unix: unix_now(),
        recipient_public_key_sha256_hex: payload_hash(recipient_public_key.as_bytes()),
        ephemeral_public_key_hex: hex::encode(ephemeral_public.as_bytes()),
        nonce_hex: hex::encode(nonce),
        ciphertext_sha256_hex: payload_hash(&ciphertext),
        ciphertext_base64: BASE64_STANDARD.encode(ciphertext),
    })
}

fn decode_public_key(value: &str) -> Result<PublicKey> {
    let bytes = hex::decode(value).context("backup recipient public key is not valid hex")?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("backup recipient public key must be 32 bytes"))?;
    Ok(PublicKey::from(bytes))
}

fn backup_encryption_key(
    shared_secret: &[u8; 32],
    recipient_public_key: &[u8; 32],
    ephemeral_public_key: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"vpsman-backup-artifact-v1");
    hasher.update(shared_secret);
    hasher.update(recipient_public_key);
    hasher.update(ephemeral_public_key);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chacha20poly1305::aead::Aead;
    use vpsman_common::AgentBackupConfig;
    use x25519_dalek::StaticSecret;

    #[tokio::test]
    async fn creates_encrypted_backup_artifact_without_plaintext_leak() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let selected_path = dir.join("selected.txt");
        let config_path = dir.join("agent.toml");
        tokio::fs::write(&selected_path, b"selected secret contents")
            .await
            .unwrap();
        tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
            .await
            .unwrap();
        let recipient_secret = StaticSecret::from([7_u8; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);
        let mut config = AgentConfig {
            client_id: "client-a".to_string(),
            backup: AgentBackupConfig {
                recipient_public_key_hex: Some(hex::encode(recipient_public.as_bytes())),
                max_plaintext_bytes: 8192,
            },
            ..AgentConfig::default()
        };
        config.display_name = "client-a".to_string();

        let paths = vec![selected_path.to_string_lossy().to_string()];
        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: true,
            recipient_public_key_hex: None,
            output_tx: None,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();
        let artifact_bytes = outputs
            .iter()
            .filter(|output| output.stream == OutputStream::Stdout)
            .flat_map(|output| output.data.clone())
            .collect::<Vec<_>>();
        let artifact_text = String::from_utf8_lossy(&artifact_bytes);
        assert!(!artifact_text.contains("selected secret contents"));
        assert!(!artifact_text.contains("secret-privilege-key"));

        let artifact: EncryptedBackupArtifact = serde_json::from_slice(&artifact_bytes).unwrap();
        assert_eq!(artifact.format, BACKUP_ARTIFACT_FORMAT);
        let archive = decrypt_artifact(&recipient_secret, &artifact);
        assert_eq!(archive.format, BACKUP_ARCHIVE_FORMAT);
        assert_eq!(archive.client_id, "client-a");
        assert_eq!(archive.files.len(), 2);
        assert!(archive
            .files
            .iter()
            .any(|file| file.path == selected_path.to_string_lossy().as_ref()
                && file.sha256_hex == payload_hash(b"selected secret contents")));
        let status = outputs.iter().find(|output| output.done).unwrap();
        let status: serde_json::Value = serde_json::from_slice(&status.data).unwrap();
        assert_eq!(status["type"], "backup");
        assert_eq!(status["encrypted"], true);
        assert_eq!(status["file_count"], 2);
        assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn streams_backup_artifact_through_payload_sink() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-stream-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let selected_path = dir.join("selected.bin");
        let selected_data = (0..8192)
            .map(|value| (value % 251) as u8)
            .collect::<Vec<_>>();
        let config_path = dir.join("agent.toml");
        tokio::fs::write(&selected_path, &selected_data)
            .await
            .unwrap();
        tokio::fs::write(&config_path, b"noise_client_private_key_hex = \"secret\"")
            .await
            .unwrap();
        let recipient_secret = StaticSecret::from([11_u8; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);
        let mut config = AgentConfig {
            client_id: "client-stream".to_string(),
            backup: AgentBackupConfig {
                recipient_public_key_hex: Some(hex::encode(recipient_public.as_bytes())),
                max_plaintext_bytes: 64 * 1024,
            },
            ..AgentConfig::default()
        };
        config.display_name = "client-stream".to_string();
        let (tx, mut rx) = mpsc::channel(64);

        let paths = vec![selected_path.to_string_lossy().to_string()];
        let outputs = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &config_path,
            paths: &paths,
            include_config: true,
            recipient_public_key_hex: None,
            output_tx: Some(tx),
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap();

        let mut artifact_bytes = Vec::new();
        while let Some(output) = rx.recv().await {
            assert_eq!(output.stream, OutputStream::Stdout);
            assert!(!output.done);
            artifact_bytes.extend_from_slice(&output.data);
        }
        assert!(!artifact_bytes.is_empty());
        assert!(outputs
            .iter()
            .all(|output| output.stream == OutputStream::Status));
        let status: serde_json::Value = serde_json::from_slice(&outputs[0].data).unwrap();
        assert_eq!(status["type"], "backup");
        assert_eq!(status["streamed"], true);
        assert_eq!(status["artifact_sha256_hex"], payload_hash(&artifact_bytes));
        assert!(status["chunk_count"].as_u64().unwrap() >= 1);

        let artifact_text = String::from_utf8_lossy(&artifact_bytes);
        assert!(!artifact_text.contains("secret-privilege-key"));
        let artifact: EncryptedBackupArtifact = serde_json::from_slice(&artifact_bytes).unwrap();
        let archive = decrypt_artifact(&recipient_secret, &artifact);
        assert_eq!(archive.client_id, "client-stream");
        assert_eq!(archive.files.len(), 2);

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    #[tokio::test]
    async fn backup_rejects_missing_recipient_and_unsafe_scope() {
        let job_id = uuid::Uuid::new_v4();
        let dir = std::env::temp_dir().join(format!("vpsman-agent-backup-reject-{job_id}"));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let file_path = dir.join("selected.txt");
        tokio::fs::write(&file_path, b"contents").await.unwrap();

        let paths = vec![file_path.to_string_lossy().to_string()];
        let default_config = AgentConfig::default();
        let missing_key = execute_backup_command(BackupCommandInput {
            job_id,
            config: &default_config,
            config_path: &file_path,
            paths: &paths,
            include_config: false,
            recipient_public_key_hex: None,
            output_tx: None,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(missing_key
            .to_string()
            .contains("backup recipient public key is not configured"));

        let recipient_secret = StaticSecret::from([9_u8; 32]);
        let recipient_public = PublicKey::from(&recipient_secret);
        let config = AgentConfig {
            backup: AgentBackupConfig {
                recipient_public_key_hex: Some(hex::encode(recipient_public.as_bytes())),
                max_plaintext_bytes: 4,
            },
            ..AgentConfig::default()
        };
        let relative_paths = vec!["relative".to_string()];
        let relative = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &file_path,
            paths: &relative_paths,
            include_config: false,
            recipient_public_key_hex: None,
            output_tx: None,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(relative.to_string().contains("file path must be absolute"));

        let too_large = execute_backup_command(BackupCommandInput {
            job_id,
            config: &config,
            config_path: &file_path,
            paths: &paths,
            include_config: false,
            recipient_public_key_hex: None,
            output_tx: None,
            timeout_secs: 5,
            cancel_token: CommandCancelToken::default(),
        })
        .await
        .unwrap_err();
        assert!(too_large.to_string().contains("plaintext limit"));

        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    fn decrypt_artifact(
        recipient_secret: &StaticSecret,
        artifact: &EncryptedBackupArtifact,
    ) -> BackupArchive {
        let ephemeral_public = decode_public_key(&artifact.ephemeral_public_key_hex).unwrap();
        let shared = recipient_secret.diffie_hellman(&ephemeral_public);
        let recipient_public = PublicKey::from(recipient_secret);
        let key_bytes = backup_encryption_key(
            shared.as_bytes(),
            recipient_public.as_bytes(),
            ephemeral_public.as_bytes(),
        );
        let nonce = hex::decode(&artifact.nonce_hex).unwrap();
        let ciphertext = BASE64_STANDARD.decode(&artifact.ciphertext_base64).unwrap();
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
        let compressed = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .unwrap();
        let plaintext = lz4_flex::decompress_size_prepended(&compressed).unwrap();
        let mut tar_archive = tar::Archive::new(std::io::Cursor::new(plaintext));
        for entry in tar_archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == BACKUP_ARCHIVE_MANIFEST_PATH {
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
                return serde_json::from_slice(&data).unwrap();
            }
        }
        panic!("backup manifest missing")
    }
}
