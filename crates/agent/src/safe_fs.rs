use std::{
    ffi::{CString, OsStr, OsString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct NodeIdentity {
    dev: u64,
    ino: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct StatInfo {
    pub(crate) identity: NodeIdentity,
    pub(crate) mode: u32,
    pub(crate) uid: u32,
}

impl StatInfo {
    pub(crate) fn is_dir(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFDIR
    }

    pub(crate) fn is_file(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFREG
    }

    pub(crate) fn is_symlink(&self) -> bool {
        self.mode & libc::S_IFMT == libc::S_IFLNK
    }

    pub(crate) fn permission_bits(&self) -> u32 {
        self.mode & 0o777
    }
}

pub(crate) struct SafeParent {
    dir: File,
    name: OsString,
}

impl SafeParent {
    pub(crate) fn dir(&self) -> &File {
        &self.dir
    }

    pub(crate) fn name(&self) -> &OsStr {
        &self.name
    }

    pub(crate) fn child_stat_nofollow(&self) -> Result<Option<StatInfo>> {
        stat_child(&self.dir, &self.name, false)
    }

    pub(crate) fn open_child_file_read(&self, follow_symlinks: bool) -> Result<File> {
        open_child_file_read(&self.dir, &self.name, follow_symlinks)
    }

    pub(crate) fn open_child_readwrite_nofollow(&self) -> Result<File> {
        open_child(
            &self.dir,
            &self.name,
            libc::O_RDWR | libc::O_NOFOLLOW,
            0,
            "failed to open file",
        )
    }
}

pub(crate) fn resolve_parent(path: &Path) -> Result<SafeParent> {
    let (parent_components, leaf) = split_parent_components(path)?;
    let dir = open_components_no_symlinks(parent_components)?;
    Ok(SafeParent { dir, name: leaf })
}

pub(crate) fn ensure_dir_all_no_symlinks(path: &Path) -> Result<File> {
    Ok(ensure_dir_all_no_symlinks_with_mode(path, 0o777)?.0)
}

pub(crate) fn ensure_dir_all_no_symlinks_with_mode(
    path: &Path,
    final_mode: u32,
) -> Result<(File, bool)> {
    if !path.is_absolute() {
        anyhow::bail!("path must be absolute");
    }
    let components = normal_components(path)?;
    let mut dir = open_root_dir()?;
    let mut created_final = false;
    let last_index = components.len().saturating_sub(1);
    for (index, name) in components.into_iter().enumerate() {
        if stat_child(&dir, &name, false)?.is_none() {
            let mode = if index == last_index {
                final_mode
            } else {
                0o777
            };
            let created = match mkdir_child(&dir, &name, mode) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("failed to create directory {}", path.display()));
                }
            };
            if created && index == last_index {
                created_final = true;
            }
        }
        let next = open_child_dir_no_symlinks(&dir, &name).with_context(|| {
            format!(
                "path component is not a real directory under {}",
                path.display()
            )
        })?;
        dir = next;
    }
    Ok((dir, created_final))
}

pub(crate) fn create_child_dir(parent: &File, name: &OsStr, mode: u32) -> Result<File> {
    mkdir_child(parent, name, mode)?;
    let dir = open_child_dir_no_symlinks(parent, name)?;
    fchmod_file(&dir, mode)?;
    sync_dir_best_effort(parent);
    Ok(dir)
}

pub(crate) fn open_child_dir_no_symlinks(parent: &File, name: &OsStr) -> Result<File> {
    open_child(
        parent,
        name,
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        0,
        "failed to open directory",
    )
}

pub(crate) fn open_child_dir(parent: &File, name: &OsStr, follow_symlinks: bool) -> Result<File> {
    let mut flags = libc::O_RDONLY | libc::O_DIRECTORY;
    if !follow_symlinks {
        flags |= libc::O_NOFOLLOW;
    }
    open_child(parent, name, flags, 0, "failed to open directory")
}

pub(crate) fn open_child_file_read(
    parent: &File,
    name: &OsStr,
    follow_symlinks: bool,
) -> Result<File> {
    let mut flags = libc::O_RDONLY;
    if !follow_symlinks {
        flags |= libc::O_NOFOLLOW;
    }
    open_child(parent, name, flags, 0, "failed to open file")
}

pub(crate) fn create_private_temp_file(
    parent: &File,
    destination_name: &OsStr,
    kind: &str,
) -> Result<(File, OsString)> {
    let destination_name = destination_name.to_string_lossy();
    for _ in 0..16 {
        let name = OsString::from(format!(
            ".vpsman-{kind}-{destination_name}-{}",
            Uuid::new_v4()
        ));
        match open_child(
            parent,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o600,
            "failed to create temporary file",
        ) {
            Ok(file) => return Ok((file, name)),
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::AlreadyExists) => {}
            Err(error) => return Err(error),
        }
    }
    anyhow::bail!("failed to allocate a unique temporary file name")
}

pub(crate) fn create_private_child_file(parent: &File, name: &OsStr) -> Result<File> {
    open_child(
        parent,
        name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
        0o600,
        "failed to create file",
    )
}

pub(crate) fn rename_child(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
    replace: bool,
) -> io::Result<()> {
    if replace {
        renameat(
            source_parent,
            source_name,
            destination_parent,
            destination_name,
        )
    } else {
        renameat_no_replace(
            source_parent,
            source_name,
            destination_parent,
            destination_name,
        )
    }
}

pub(crate) fn remove_child_file(parent: &File, name: &OsStr) -> io::Result<()> {
    unlink_child(parent, name, 0)
}

pub(crate) fn remove_child_dir(parent: &File, name: &OsStr) -> io::Result<()> {
    unlink_child(parent, name, libc::AT_REMOVEDIR)
}

pub(crate) fn read_dir_names(dir: &File) -> Result<Vec<OsString>> {
    let mut entries = Vec::new();
    for entry in
        std::fs::read_dir(fd_path(dir)).with_context(|| "failed to read directory by descriptor")?
    {
        let entry = entry?;
        entries.push(entry.file_name());
    }
    entries.sort();
    Ok(entries)
}

pub(crate) fn stat_child(
    parent: &File,
    name: &OsStr,
    follow_symlinks: bool,
) -> Result<Option<StatInfo>> {
    let name = cstring(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    let result =
        unsafe { libc::fstatat(parent.as_raw_fd(), name.as_ptr(), stat.as_mut_ptr(), flags) };
    if result != 0 {
        let error = io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(libc::ENOENT) | Some(libc::ENOTDIR)
        ) {
            return Ok(None);
        }
        return Err(error).context("failed to stat path component");
    }
    let stat = unsafe { stat.assume_init() };
    Ok(Some(StatInfo {
        identity: NodeIdentity {
            dev: stat.st_dev,
            ino: stat.st_ino,
        },
        mode: stat.st_mode,
        uid: stat.st_uid,
    }))
}

pub(crate) fn stat_file(file: &File) -> Result<StatInfo> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    let result = unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("failed to stat opened file");
    }
    let stat = unsafe { stat.assume_init() };
    Ok(StatInfo {
        identity: NodeIdentity {
            dev: stat.st_dev,
            ino: stat.st_ino,
        },
        mode: stat.st_mode,
        uid: stat.st_uid,
    })
}

pub(crate) fn ensure_identity(file: &File, expected: &NodeIdentity, label: &str) -> Result<()> {
    let actual = stat_file(file)?.identity;
    if &actual != expected {
        anyhow::bail!("{label}");
    }
    Ok(())
}

pub(crate) fn fchmod_file(file: &File, mode: u32) -> Result<()> {
    let result = unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("failed to set file mode");
    }
    Ok(())
}

pub(crate) fn fchown_file(file: &File, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    let uid = uid
        .map(|value| value as libc::uid_t)
        .unwrap_or(!0 as libc::uid_t);
    let gid = gid
        .map(|value| value as libc::gid_t)
        .unwrap_or(!0 as libc::gid_t);
    let result = unsafe { libc::fchown(file.as_raw_fd(), uid, gid) };
    if result != 0 {
        return Err(io::Error::last_os_error()).context("failed to change file ownership");
    }
    Ok(())
}

pub(crate) fn sync_dir_best_effort(dir: &File) {
    let _ = dir.sync_all();
}

fn split_parent_components(path: &Path) -> Result<(Vec<OsString>, OsString)> {
    let mut components = normal_components(path)?;
    let leaf = components.pop().context("path has no final component")?;
    Ok((components, leaf))
}

fn normal_components(path: &Path) -> Result<Vec<OsString>> {
    if !path.is_absolute() {
        anyhow::bail!("path must be absolute");
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => components.push(name.to_os_string()),
            _ => anyhow::bail!("path contains unsupported component"),
        }
    }
    Ok(components)
}

fn open_components_no_symlinks(components: Vec<OsString>) -> Result<File> {
    let mut dir = open_root_dir()?;
    for component in components {
        dir = open_child_dir_no_symlinks(&dir, &component).with_context(|| {
            format!(
                "parent path component is not a real directory: {}",
                component.to_string_lossy()
            )
        })?;
    }
    Ok(dir)
}

fn open_root_dir() -> Result<File> {
    let root = cstring(OsStr::new("/"))?;
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("failed to open filesystem root");
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn mkdir_child(parent: &File, name: &OsStr, mode: u32) -> io::Result<()> {
    let name = cstring(name)?;
    let result = unsafe {
        libc::mkdirat(
            parent.as_raw_fd(),
            name.as_ptr(),
            (mode & 0o777) as libc::mode_t,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn open_child(parent: &File, name: &OsStr, flags: i32, mode: u32, label: &str) -> Result<File> {
    let name = cstring(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            (mode & 0o777) as libc::mode_t,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context(label.to_string());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn renameat(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
) -> io::Result<()> {
    let source_name = cstring(source_name)?;
    let destination_name = cstring(destination_name)?;
    let result = unsafe {
        libc::renameat(
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn renameat_no_replace(
    source_parent: &File,
    source_name: &OsStr,
    destination_parent: &File,
    destination_name: &OsStr,
) -> io::Result<()> {
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let source_name = cstring(source_name)?;
    let destination_name = cstring(destination_name)?;
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            source_parent.as_raw_fd(),
            source_name.as_ptr(),
            destination_parent.as_raw_fd(),
            destination_name.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn unlink_child(parent: &File, name: &OsStr, flags: i32) -> io::Result<()> {
    let name = cstring(name)?;
    let result = unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn fd_path(file: &File) -> String {
    format!("/proc/self/fd/{}", file.as_raw_fd())
}

fn cstring(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains nul byte"))
}
