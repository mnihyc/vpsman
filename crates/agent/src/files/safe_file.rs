use std::{
    fs::{File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FileIdentity {
    dev: u64,
    ino: u64,
    len: u64,
    mtime_sec: i64,
    mtime_nsec: i64,
    ctime_sec: i64,
    ctime_nsec: i64,
}

impl FileIdentity {
    pub(crate) fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            len: metadata.len(),
            mtime_sec: metadata.mtime(),
            mtime_nsec: metadata.mtime_nsec(),
            ctime_sec: metadata.ctime(),
            ctime_nsec: metadata.ctime_nsec(),
        }
    }
}

pub(crate) struct OpenedReadFile {
    pub(crate) file: File,
    pub(crate) metadata: Metadata,
    pub(crate) identity: FileIdentity,
}

pub(crate) struct BoundedRead {
    pub(crate) data: Vec<u8>,
    pub(crate) metadata: Metadata,
}

pub(crate) fn open_regular_file_for_read(
    path: &Path,
    follow_symlinks: bool,
    symlink_error: &str,
) -> Result<OpenedReadFile> {
    let mut options = OpenOptions::new();
    options.read(true);
    if !follow_symlinks {
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) => {
            if !follow_symlinks
                && std::fs::symlink_metadata(path)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false)
            {
                anyhow::bail!("{symlink_error}");
            }
            return Err(error).with_context(|| format!("failed to open {}", path.display()));
        }
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("failed to stat opened file {}", path.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("path is not a regular file");
    }
    let identity = FileIdentity::from_metadata(&metadata);
    Ok(OpenedReadFile {
        file,
        metadata,
        identity,
    })
}

pub(crate) fn read_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
    follow_symlinks: bool,
    limit_label: &str,
    symlink_error: &str,
) -> Result<BoundedRead> {
    let opened = open_regular_file_for_read(path, follow_symlinks, symlink_error)?;
    if opened.metadata.len() > max_bytes {
        anyhow::bail!(
            "{limit_label}: {} > {max_bytes} bytes",
            opened.metadata.len()
        );
    }
    let data = read_opened_file_bounded(opened.file, max_bytes, limit_label)?;
    Ok(BoundedRead {
        data,
        metadata: opened.metadata,
    })
}

pub(crate) fn read_opened_file_bounded(
    mut file: File,
    max_bytes: u64,
    limit_label: &str,
) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity((max_bytes.min(16 * 1024)) as usize);
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .context("failed to read opened file")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("file read size overflow")?;
        if total > max_bytes {
            anyhow::bail!("{limit_label}: {total} > {max_bytes} bytes");
        }
        data.extend_from_slice(&buffer[..read]);
    }
    Ok(data)
}

pub(crate) fn hash_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
    follow_symlinks: bool,
    limit_label: &str,
    symlink_error: &str,
) -> Result<(String, Metadata, FileIdentity)> {
    let opened = open_regular_file_for_read(path, follow_symlinks, symlink_error)?;
    if opened.metadata.len() > max_bytes {
        anyhow::bail!(
            "{limit_label}: {} > {max_bytes} bytes",
            opened.metadata.len()
        );
    }
    let hash = hash_opened_file_bounded(opened.file, max_bytes, limit_label)?;
    Ok((hash, opened.metadata, opened.identity))
}

pub(crate) fn hash_opened_file_bounded(
    mut file: File,
    max_bytes: u64,
    limit_label: &str,
) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .context("failed to read opened file")?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .context("file hash size overflow")?;
        if total > max_bytes {
            anyhow::bail!("{limit_label}: {total} > {max_bytes} bytes");
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn read_regular_file_chunk_checked(
    path: &Path,
    offset: u64,
    size: usize,
    follow_symlinks: bool,
    expected_identity: &FileIdentity,
    symlink_error: &str,
) -> Result<Vec<u8>> {
    let mut opened = open_regular_file_for_read(path, follow_symlinks, symlink_error)?;
    if &opened.identity != expected_identity {
        anyhow::bail!("download source changed since session start");
    }
    opened
        .file
        .seek(SeekFrom::Start(offset))
        .context("failed to seek download source")?;
    let mut chunk = vec![0_u8; size];
    let read = opened
        .file
        .read(&mut chunk)
        .context("failed to read download source")?;
    chunk.truncate(read);
    Ok(chunk)
}
