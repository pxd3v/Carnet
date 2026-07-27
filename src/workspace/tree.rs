use std::{
    ffi::OsStr,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use cap_std::fs::Dir;
use std::os::unix::process::CommandExt;

use super::{FileError, files::bytes_are_text};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TreeEntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreeEntry {
    path: PathBuf,
    kind: TreeEntryKind,
    enabled: bool,
    children: Vec<TreeEntry>,
}

impl TreeEntry {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> TreeEntryKind {
        self.kind
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn children(&self) -> &[TreeEntry] {
        &self.children
    }
}

pub(crate) fn build(directory: &Dir, root: &Path) -> Result<Vec<TreeEntry>, FileError> {
    build_directory(directory, directory, root, Path::new("."))
}

fn build_directory(
    root_directory: &Dir,
    directory: &Dir,
    root: &Path,
    relative_directory: &Path,
) -> Result<Vec<TreeEntry>, FileError> {
    let mut entries = Vec::new();
    let iterator = directory.entries().map_err(|source| FileError::Io {
        path: relative_directory.to_path_buf(),
        source,
    })?;
    for entry in iterator {
        let entry = entry.map_err(|source| FileError::Io {
            path: relative_directory.to_path_buf(),
            source,
        })?;
        if entry.file_name() == OsStr::new(".git") {
            continue;
        }
        let relative = relative_directory.join(entry.file_name());
        let relative = relative
            .strip_prefix(".")
            .unwrap_or(&relative)
            .to_path_buf();
        if is_ignored(root_directory, root, &relative)? {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| FileError::Io {
            path: relative.clone(),
            source,
        })?;
        let (kind, enabled, children) = if file_type.is_symlink() {
            (TreeEntryKind::Symlink, false, Vec::new())
        } else if file_type.is_dir() {
            let child = entry.open_dir().map_err(|source| FileError::Io {
                path: relative.clone(),
                source,
            })?;
            (
                TreeEntryKind::Directory,
                true,
                build_directory(root_directory, &child, root, &relative)?,
            )
        } else {
            let enabled = entry
                .open()
                .and_then(|mut file| {
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes)?;
                    Ok(bytes)
                })
                .map(|bytes| bytes_are_text(&bytes))
                .unwrap_or(false);
            (TreeEntryKind::File, enabled, Vec::new())
        };
        entries.push(TreeEntry {
            path: relative,
            kind,
            enabled,
            children,
        });
    }
    entries.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(entries)
}

fn is_ignored(directory: &Dir, root: &Path, relative: &Path) -> Result<bool, FileError> {
    let directory_fd = directory.as_raw_fd();
    let mut command = Command::new("git");
    command
        .args(["check-ignore", "-q", "--"])
        .arg(relative)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: this closure calls only async-signal-safe fchdir with a raw fd
    // kept alive by `directory` until after the child has spawned.
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(directory_fd) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = command.output().map_err(|source| FileError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(FileError::GitIgnore {
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
    }
}
