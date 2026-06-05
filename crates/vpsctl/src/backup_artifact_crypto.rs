use std::{io::Cursor, path::PathBuf};

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpsman_common::payload_hash;
use x25519_dalek::{PublicKey, StaticSecret};

pub(crate) const MAX_BACKUP_ARTIFACT_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;

const BACKUP_ARTIFACT_FORMAT: &str = "vpsman.backup_artifact.v1";
const BACKUP_ARCHIVE_FORMAT: &str = "vpsman.backup_tar.v1";
const LEGACY_BACKUP_ARCHIVE_FORMAT: &str = "vpsman.backup_archive.v1";
const BACKUP_ARCHIVE_MANIFEST_PATH: &str = "vpsman-backup/manifest.json";

#[derive(Debug, Deserialize)]
struct EncryptedBackupArtifact {
    format: String,
    version: u32,
    cipher: String,
    compression: String,
    client_id: String,
    recipient_public_key_sha256_hex: String,
    ephemeral_public_key_hex: String,
    nonce_hex: String,
    ciphertext_sha256_hex: String,
    ciphertext_base64: String,
}

#[derive(Debug, Deserialize)]
struct BackupArchive {
    format: String,
}

pub(crate) fn validate_artifact_metadata(
    object_key: &str,
    sha256_hex: &str,
    size_bytes: i64,
) -> Result<()> {
    validate_artifact_object_key(object_key)?;
    anyhow::ensure!(
        sha256_hex.len() == 64 && sha256_hex.as_bytes().iter().all(u8::is_ascii_hexdigit),
        "artifact sha256 must be 64 hex characters"
    );
    anyhow::ensure!(size_bytes > 0, "artifact size must be positive");
    Ok(())
}

pub(crate) fn validate_artifact_object_key(object_key: &str) -> Result<()> {
    anyhow::ensure!(
        !object_key.trim().is_empty(),
        "artifact object key is required"
    );
    anyhow::ensure!(
        object_key.len() <= 1024
            && !object_key.as_bytes().contains(&0)
            && !object_key.starts_with('/')
            && !object_key.contains('\\')
            && !object_key
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == ".."),
        "artifact object key must be a relative object key without . or .. segments"
    );
    Ok(())
}

pub(crate) fn restore_artifact_bytes(
    api_url: &str,
    token: Option<&str>,
    source_backup_request_id: Uuid,
    artifact_file: Option<&PathBuf>,
) -> Result<Vec<u8>> {
    match artifact_file {
        Some(path) => read_bounded_artifact_file(path),
        None => {
            let bytes = crate::http::http_get_bytes(
                api_url,
                &format!("/api/v1/backups/{source_backup_request_id}/artifact"),
                token,
            )
            .context("failed to download backup artifact from object store")?;
            anyhow::ensure!(
                (1..=MAX_BACKUP_ARTIFACT_UPLOAD_BYTES as usize).contains(&bytes.len()),
                "downloaded backup artifact size must be between 1 and {MAX_BACKUP_ARTIFACT_UPLOAD_BYTES} bytes"
            );
            Ok(bytes)
        }
    }
}

pub(crate) fn decrypt_backup_artifact(
    artifact_bytes: &[u8],
    private_key_hex: &str,
) -> Result<Vec<u8>> {
    let artifact: EncryptedBackupArtifact =
        serde_json::from_slice(artifact_bytes).context("backup artifact JSON is invalid")?;
    anyhow::ensure!(
        artifact.format == BACKUP_ARTIFACT_FORMAT,
        "backup artifact format is invalid"
    );
    anyhow::ensure!(artifact.version == 1, "backup artifact version is invalid");
    anyhow::ensure!(
        artifact.cipher == "x25519-chacha20poly1305",
        "backup artifact cipher is invalid"
    );
    anyhow::ensure!(
        artifact.compression == "lz4-size-prepended",
        "backup artifact compression is invalid"
    );
    anyhow::ensure!(
        !artifact.client_id.trim().is_empty(),
        "backup artifact client id is empty"
    );
    let ciphertext = BASE64
        .decode(&artifact.ciphertext_base64)
        .context("backup artifact ciphertext is invalid base64")?;
    anyhow::ensure!(
        payload_hash(&ciphertext) == artifact.ciphertext_sha256_hex,
        "backup artifact ciphertext sha256 mismatch"
    );
    let private_bytes = decode_fixed_hex(private_key_hex, "backup private key")?;
    let recipient_secret = StaticSecret::from(private_bytes);
    let recipient_public = PublicKey::from(&recipient_secret);
    anyhow::ensure!(
        payload_hash(recipient_public.as_bytes()) == artifact.recipient_public_key_sha256_hex,
        "backup private key does not match artifact recipient"
    );
    let ephemeral_public = PublicKey::from(decode_fixed_hex(
        &artifact.ephemeral_public_key_hex,
        "backup artifact ephemeral public key",
    )?);
    let nonce = hex::decode(&artifact.nonce_hex).context("backup artifact nonce is invalid hex")?;
    anyhow::ensure!(nonce.len() == 12, "backup artifact nonce must be 12 bytes");
    let shared_secret = recipient_secret.diffie_hellman(&ephemeral_public);
    let key_bytes = backup_encryption_key(
        shared_secret.as_bytes(),
        recipient_public.as_bytes(),
        ephemeral_public.as_bytes(),
    );
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let compressed_archive = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("failed to decrypt backup artifact"))?;
    let archive_bytes = lz4_flex::decompress_size_prepended(&compressed_archive)
        .context("failed to decompress backup archive")?;
    validate_backup_archive(&archive_bytes)?;
    Ok(archive_bytes)
}

fn validate_backup_archive(archive_bytes: &[u8]) -> Result<()> {
    if archive_bytes.first() == Some(&b'{') {
        let archive: BackupArchive = serde_json::from_slice(archive_bytes)
            .context("legacy backup archive JSON is invalid")?;
        anyhow::ensure!(
            archive.format == LEGACY_BACKUP_ARCHIVE_FORMAT,
            "backup archive format is invalid"
        );
        return Ok(());
    }
    let mut archive = tar::Archive::new(Cursor::new(archive_bytes));
    for entry in archive.entries().context("backup tar archive is invalid")? {
        let mut entry = entry.context("backup tar entry is invalid")?;
        if entry
            .path()
            .context("backup tar entry path is invalid")?
            .to_string_lossy()
            == BACKUP_ARCHIVE_MANIFEST_PATH
        {
            let mut manifest_bytes = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut manifest_bytes)
                .context("failed to read backup tar manifest")?;
            let manifest: BackupArchive = serde_json::from_slice(&manifest_bytes)
                .context("backup tar manifest JSON is invalid")?;
            anyhow::ensure!(
                manifest.format == BACKUP_ARCHIVE_FORMAT,
                "backup archive format is invalid"
            );
            return Ok(());
        }
    }
    anyhow::bail!("backup tar manifest is missing")
}

fn read_bounded_artifact_file(path: &PathBuf) -> Result<Vec<u8>> {
    let metadata =
        std::fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
    anyhow::ensure!(metadata.is_file(), "artifact file must be a regular file");
    anyhow::ensure!(
        (1..=MAX_BACKUP_ARTIFACT_UPLOAD_BYTES).contains(&metadata.len()),
        "artifact file size must be between 1 and {MAX_BACKUP_ARTIFACT_UPLOAD_BYTES} bytes"
    );
    std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn decode_fixed_hex(value: &str, label: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(value.trim()).with_context(|| format!("{label} is not valid hex"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("{label} must be 32 bytes"))
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
