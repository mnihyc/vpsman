use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Write},
    os::fd::AsRawFd,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use tokio::io::AsyncWriteExt;
use uuid::Uuid;

pub const PRIVATE_FILE_MODE: u32 = 0o600;
pub const PRIVATE_DIR_MODE: u32 = 0o700;

const DEFAULT_PRIVATE_MODE: u32 = PRIVATE_FILE_MODE;

#[derive(Clone, Copy, Debug)]
pub struct PrivateFileWriteOptions {
    pub default_mode: u32,
    pub preserve_existing_owner_only_mode: bool,
}

impl Default for PrivateFileWriteOptions {
    fn default() -> Self {
        Self {
            default_mode: DEFAULT_PRIVATE_MODE,
            preserve_existing_owner_only_mode: true,
        }
    }
}

pub fn write_private_file_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_private_file_atomically_with_options(path, contents, PrivateFileWriteOptions::default())
}

pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    clamp_private_dir(path)
}

pub fn ensure_private_dir_tree(root: &Path, path: &Path) -> io::Result<()> {
    ensure_private_dir(root)?;
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory path must be under private root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                current.push(name);
                ensure_private_dir(&current)?;
            }
            Component::CurDir => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private directory path must not contain traversal components",
                ));
            }
        }
    }
    Ok(())
}

pub async fn ensure_private_dir_async(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(path).await?;
    clamp_private_dir_async(path).await
}

pub async fn ensure_private_dir_tree_async(root: &Path, path: &Path) -> io::Result<()> {
    ensure_private_dir_async(root).await?;
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory path must be under private root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                current.push(name);
                ensure_private_dir_async(&current).await?;
            }
            Component::CurDir => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private directory path must not contain traversal components",
                ));
            }
        }
    }
    Ok(())
}

pub fn create_private_file_new(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure_open_file_private(&file)?;
    Ok(file)
}

pub fn open_private_file_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure_open_file_private(&file)?;
    Ok(file)
}

pub fn open_private_file_read_write(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(create)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure_open_file_private(&file)?;
    Ok(file)
}

pub fn open_private_file_read(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure_open_file_private(&file)?;
    Ok(file)
}

pub fn repair_private_file_permissions(path: &Path) -> io::Result<()> {
    let file = open_private_file_read(path)?;
    drop(file);
    Ok(())
}

pub async fn create_private_file_new_async(path: &Path) -> io::Result<tokio::fs::File> {
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .await?;
    ensure_open_file_private_async(&file).await?;
    Ok(file)
}

pub async fn open_private_file_read_write_async(
    path: &Path,
    create: bool,
) -> io::Result<tokio::fs::File> {
    let file = tokio::fs::OpenOptions::new()
        .create(create)
        .read(true)
        .write(true)
        .truncate(false)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .await?;
    ensure_open_file_private_async(&file).await?;
    Ok(file)
}

pub async fn open_private_file_read_async(path: &Path) -> io::Result<tokio::fs::File> {
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .await?;
    ensure_open_file_private_async(&file).await?;
    Ok(file)
}

pub async fn repair_private_file_permissions_async(path: &Path) -> io::Result<()> {
    let file = open_private_file_read_async(path).await?;
    drop(file);
    Ok(())
}

pub async fn write_private_file_atomically_async(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir_async(parent).await?;
    }
    let tmp_path = temp_path_for(path);
    let result = async {
        let mut file = create_private_file_new_async(&tmp_path).await?;
        file.write_all(contents).await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&tmp_path, path).await?;
        sync_parent_async(path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp_path).await;
    }
    result
}

pub fn write_private_file_atomically_with_options(
    path: &Path,
    contents: &[u8],
    options: PrivateFileWriteOptions,
) -> io::Result<()> {
    let final_mode = final_private_mode(path, options)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = temp_path_for(path);
    let result = write_and_replace(path, &tmp_path, contents, final_mode);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_and_replace(
    path: &Path,
    tmp_path: &Path,
    contents: &[u8],
    final_mode: u32,
) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(DEFAULT_PRIVATE_MODE)
        .open(tmp_path)?;
    file.write_all(contents)?;
    file.set_permissions(Permissions::from_mode(final_mode))?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp_path, path)?;
    sync_parent(path)?;
    Ok(())
}

fn clamp_private_dir(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory target must be a directory",
        ));
    }
    fs::set_permissions(path, Permissions::from_mode(PRIVATE_DIR_MODE))
}

async fn clamp_private_dir_async(path: &Path) -> io::Result<()> {
    let metadata = tokio::fs::symlink_metadata(path).await?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private directory target must be a directory",
        ));
    }
    tokio::fs::set_permissions(path, Permissions::from_mode(PRIVATE_DIR_MODE)).await
}

fn ensure_open_file_private(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file target must be a regular file",
        ));
    }
    fchmod(file.as_raw_fd(), PRIVATE_FILE_MODE)
}

async fn ensure_open_file_private_async(file: &tokio::fs::File) -> io::Result<()> {
    let metadata = file.metadata().await?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private file target must be a regular file",
        ));
    }
    fchmod(file.as_raw_fd(), PRIVATE_FILE_MODE)
}

fn fchmod(fd: std::os::fd::RawFd, mode: u32) -> io::Result<()> {
    let rc = unsafe { libc::fchmod(fd, mode as libc::mode_t) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn final_private_mode(path: &Path, options: PrivateFileWriteOptions) -> io::Result<u32> {
    if !options.preserve_existing_owner_only_mode {
        return Ok(options.default_mode);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() || !file_type.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private file target must be a regular file",
                ));
            }
            let mode = metadata.permissions().mode() & 0o7777;
            if mode & 0o077 == 0 && mode & 0o600 == 0o600 {
                Ok(mode)
            } else {
                Ok(options.default_mode)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(options.default_mode),
        Err(error) => Err(error),
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)?.sync_all()
}

async fn sync_parent_async(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    tokio::fs::File::open(parent).await?.sync_all().await
}

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "config".into());
    path.with_file_name(format!(".{file_name}.tmp-{}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    #[test]
    fn private_atomic_write_clamps_default_readable_modes() {
        let path = std::env::temp_dir().join(format!(
            "vpsman-private-write-{}.toml",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        write_private_file_atomically(&path, b"new").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn private_atomic_write_clamps_group_readable_and_preserves_owner_only_modes() {
        let path = std::env::temp_dir().join(format!(
            "vpsman-private-preserve-{}.toml",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        write_private_file_atomically(&path, b"new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        write_private_file_atomically(&path, b"newer").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn private_dir_tree_clamps_intermediate_dirs() {
        let root =
            std::env::temp_dir().join(format!("vpsman-private-dir-{}", uuid::Uuid::new_v4()));
        let nested = root.join("a").join("b");

        ensure_private_dir_tree(&root, &nested).unwrap();

        assert_eq!(mode(&root), PRIVATE_DIR_MODE);
        assert_eq!(mode(&root.join("a")), PRIVATE_DIR_MODE);
        assert_eq!(mode(&nested), PRIVATE_DIR_MODE);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn private_file_open_clamps_existing_file() {
        let path =
            std::env::temp_dir().join(format!("vpsman-private-open-{}", uuid::Uuid::new_v4()));
        fs::write(&path, b"secret").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let _file = open_private_file_read(&path).unwrap();

        assert_eq!(mode(&path), PRIVATE_FILE_MODE);
        let _ = fs::remove_file(path);
    }

    fn mode(path: &std::path::Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}
