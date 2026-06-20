use std::{
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use uuid::Uuid;

const DEFAULT_PRIVATE_MODE: u32 = 0o600;

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

fn temp_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "config".into());
    path.with_file_name(format!(".{file_name}.tmp-{}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::write_private_file_atomically;
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
}
