use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{FileError, files::bytes_are_text, paths};

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

pub(crate) fn build(root: &Path) -> Result<Vec<TreeEntry>, FileError> {
    paths::validate_root(root)?;
    build_directory(root, root)
}

fn build_directory(root: &Path, directory: &Path) -> Result<Vec<TreeEntry>, FileError> {
    let mut entries = Vec::new();
    let iterator = fs::read_dir(directory).map_err(|source| FileError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in iterator {
        let entry = entry.map_err(|source| FileError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        if entry.file_name() == OsStr::new(".git") {
            continue;
        }
        let absolute = entry.path();
        let relative = absolute
            .strip_prefix(root)
            .expect("directory traversal begins at root")
            .to_path_buf();
        if is_ignored(root, &relative)? {
            continue;
        }
        let metadata = fs::symlink_metadata(&absolute).map_err(|source| FileError::Io {
            path: absolute.clone(),
            source,
        })?;
        let (kind, enabled, children) = if metadata.file_type().is_symlink() {
            (TreeEntryKind::Symlink, false, Vec::new())
        } else if metadata.is_dir() {
            (
                TreeEntryKind::Directory,
                true,
                build_directory(root, &absolute)?,
            )
        } else {
            let enabled = fs::read(&absolute)
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

fn is_ignored(root: &Path, relative: &Path) -> Result<bool, FileError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", "--"])
        .arg(relative)
        .output()
        .map_err(|source| FileError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) | Some(128) => Ok(false),
        _ => Err(FileError::GitIgnore {
            message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        }),
    }
}
