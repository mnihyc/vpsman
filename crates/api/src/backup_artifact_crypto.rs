use std::io::Cursor;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use vpsman_common::{payload_hash, MAX_INLINE_FILE_PUSH_BYTES};
use x25519_dalek::{PublicKey, StaticSecret};

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
struct BackupArchiveManifest {
    format: String,
    files: Vec<serde_json::Value>,
}

pub(crate) struct PreparedBackupArchive {
    pub(crate) bytes: Vec<u8>,
    pub(crate) client_id: String,
    pub(crate) archive_format: String,
    pub(crate) file_count: usize,
}

pub(crate) fn prepare_backup_archive_for_restore(
    artifact_bytes: &[u8],
    private_key_hex: &str,
    expected_client_id: &str,
) -> Result<PreparedBackupArchive> {
    let artifact = parse_encrypted_artifact(artifact_bytes, expected_client_id)?;
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
    anyhow::ensure!(
        !archive_bytes.is_empty() && archive_bytes.len() <= MAX_INLINE_FILE_PUSH_BYTES,
        "restore archive must be between 1 and {MAX_INLINE_FILE_PUSH_BYTES} bytes for inline dispatch"
    );
    let (archive_format, file_count) = inspect_backup_archive(&archive_bytes)?;
    Ok(PreparedBackupArchive {
        bytes: archive_bytes,
        client_id: artifact.client_id,
        archive_format,
        file_count,
    })
}

fn parse_encrypted_artifact(
    artifact_bytes: &[u8],
    expected_client_id: &str,
) -> Result<EncryptedBackupArtifact> {
    let artifact: EncryptedBackupArtifact =
        serde_json::from_slice(artifact_bytes).context("backup artifact JSON is invalid")?;
    anyhow::ensure!(
        artifact.format == BACKUP_ARTIFACT_FORMAT,
        "backup artifact format is invalid"
    );
    anyhow::ensure!(artifact.version == 1, "backup artifact version is invalid");
    anyhow::ensure!(
        artifact.client_id == expected_client_id,
        "backup artifact client mismatch"
    );
    anyhow::ensure!(
        artifact.cipher == "x25519-chacha20poly1305",
        "backup artifact cipher is invalid"
    );
    anyhow::ensure!(
        artifact.compression == "lz4-size-prepended",
        "backup artifact compression is invalid"
    );
    Ok(artifact)
}

fn inspect_backup_archive(bytes: &[u8]) -> Result<(String, usize)> {
    if bytes.first() == Some(&b'{') {
        let manifest: BackupArchiveManifest =
            serde_json::from_slice(bytes).context("legacy backup archive JSON is invalid")?;
        anyhow::ensure!(
            manifest.format == LEGACY_BACKUP_ARCHIVE_FORMAT,
            "backup archive format is invalid"
        );
        return Ok((manifest.format, manifest.files.len()));
    }

    let mut archive = tar::Archive::new(Cursor::new(bytes));
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
            let manifest: BackupArchiveManifest = serde_json::from_slice(&manifest_bytes)
                .context("backup tar manifest JSON is invalid")?;
            anyhow::ensure!(
                manifest.format == BACKUP_ARCHIVE_FORMAT,
                "backup archive format is invalid"
            );
            return Ok((manifest.format, manifest.files.len()));
        }
    }
    anyhow::bail!("backup tar manifest is missing")
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
