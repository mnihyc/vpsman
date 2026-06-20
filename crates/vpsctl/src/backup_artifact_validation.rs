use anyhow::Result;

pub(crate) const MAX_BACKUP_ARTIFACT_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;

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
