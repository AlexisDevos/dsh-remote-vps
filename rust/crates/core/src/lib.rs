use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use dsh_protocol::dsh::remote::v1::NodeType;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathPolicyError {
    #[error("unknown root: {0}")]
    UnknownRoot(String),
    #[error("path must be relative to its root")]
    AbsolutePath,
    #[error("parent traversal is not allowed")]
    ParentTraversal,
    #[error("path contains a NUL byte")]
    NulByte,
    #[error("path escapes root")]
    OutsideRoot,
    #[error("path has no filename")]
    MissingFilename,
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug)]
pub struct Root {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct RootMap {
    roots: HashMap<String, Root>,
}

impl RootMap {
    pub fn from_specs(specs: &[String]) -> Result<Self, PathPolicyError> {
        let mut roots = HashMap::new();
        for spec in specs {
            let (id, raw_path) = spec
                .split_once('=')
                .ok_or_else(|| PathPolicyError::UnknownRoot(spec.clone()))?;
            if id.is_empty() || raw_path.is_empty() || id.contains('/') {
                return Err(PathPolicyError::UnknownRoot(spec.clone()));
            }
            let path = fs::canonicalize(raw_path)?;
            if !path.is_dir() {
                return Err(PathPolicyError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    path.display().to_string(),
                )));
            }
            if roots
                .insert(
                    id.to_owned(),
                    Root {
                        id: id.to_owned(),
                        path,
                    },
                )
                .is_some()
            {
                return Err(PathPolicyError::UnknownRoot(format!(
                    "duplicate root: {id}"
                )));
            }
        }
        Ok(Self { roots })
    }

    pub fn get(&self, id: &str) -> Result<&Root, PathPolicyError> {
        self.roots
            .get(id)
            .ok_or_else(|| PathPolicyError::UnknownRoot(id.to_owned()))
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.roots.keys().map(String::as_str)
    }

    /// Resolve a relative path and reject traversal through `..` or symlinks.
    /// Existing targets and their parents are canonicalized independently so a
    /// symlink cannot silently move an operation outside the configured root.
    pub fn resolve(&self, root_id: &str, relative: &str) -> Result<PathBuf, PathPolicyError> {
        let root = self.get(root_id)?;
        let path = Self::validate_relative(relative)?;
        let candidate = root.path.join(&path);
        let resolved = if candidate.exists() {
            fs::canonicalize(&candidate)?
        } else {
            let parent = candidate.parent().ok_or(PathPolicyError::MissingFilename)?;
            let filename = candidate
                .file_name()
                .ok_or(PathPolicyError::MissingFilename)?;
            fs::canonicalize(parent)?.join(filename)
        };

        if !resolved.starts_with(&root.path) {
            return Err(PathPolicyError::OutsideRoot);
        }
        Ok(resolved)
    }

    /// Resolve a target whose parent directories may not exist yet. The
    /// nearest existing ancestor is canonicalized, so existing symlinks are
    /// still checked before the missing suffix is appended.
    pub fn resolve_for_create(
        &self,
        root_id: &str,
        relative: &str,
    ) -> Result<PathBuf, PathPolicyError> {
        let root = self.get(root_id)?;
        let path = Self::validate_relative(relative)?;
        let candidate = root.path.join(&path);
        if candidate.exists() {
            return self.resolve(root_id, relative);
        }

        let mut ancestor = candidate.as_path();
        let mut missing = Vec::new();
        while !ancestor.exists() {
            missing.push(
                ancestor
                    .file_name()
                    .ok_or(PathPolicyError::MissingFilename)?
                    .to_owned(),
            );
            ancestor = ancestor.parent().ok_or(PathPolicyError::OutsideRoot)?;
        }
        let mut resolved = fs::canonicalize(ancestor)?;
        if !resolved.starts_with(&root.path) {
            return Err(PathPolicyError::OutsideRoot);
        }
        for component in missing.iter().rev() {
            resolved.push(component);
        }
        Ok(resolved)
    }

    fn validate_relative(relative: &str) -> Result<PathBuf, PathPolicyError> {
        if relative.as_bytes().contains(&0) {
            return Err(PathPolicyError::NulByte);
        }

        let path = Path::new(relative);
        if path.is_absolute() {
            return Err(PathPolicyError::AbsolutePath);
        }
        for component in path.components() {
            match component {
                Component::ParentDir => return Err(PathPolicyError::ParentTraversal),
                Component::Prefix(_) | Component::RootDir => {
                    return Err(PathPolicyError::AbsolutePath)
                }
                Component::CurDir | Component::Normal(_) => {}
            }
        }
        Ok(path.to_owned())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub node_type: i32,
    pub size: u64,
    pub version: String,
}

pub fn file_info(path: &Path) -> Result<FileInfo, std::io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    let node_type = if metadata.file_type().is_file() {
        NodeType::File as i32
    } else if metadata.file_type().is_dir() {
        NodeType::Directory as i32
    } else if metadata.file_type().is_symlink() {
        NodeType::Symlink as i32
    } else {
        NodeType::Other as i32
    };

    #[cfg(unix)]
    let (inode, device, ctime_ns) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.ino(), metadata.dev(), metadata.ctime_nsec())
    };
    #[cfg(not(unix))]
    let (inode, device, ctime_ns) = (0, 0, 0);

    let mtime_ns = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();

    Ok(FileInfo {
        node_type,
        size: metadata.len(),
        version: format!("{device}:{inode}:{mtime_ns}:{ctime_ns}:{}", metadata.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, File};
    use std::os::unix::fs::symlink;
    use tempfile::tempdir;

    #[test]
    fn rejects_parent_traversal_and_absolute_paths() {
        let root = tempdir().expect("temp root");
        let map = RootMap::from_specs(&[format!("project={}", root.path().display())]).unwrap();
        assert!(matches!(
            map.resolve("project", "../outside"),
            Err(PathPolicyError::ParentTraversal)
        ));
        assert!(matches!(
            map.resolve("project", "/etc/passwd"),
            Err(PathPolicyError::AbsolutePath)
        ));
    }

    #[test]
    fn rejects_symlink_escape_for_existing_target() {
        let root = tempdir().expect("temp root");
        let outside = tempdir().expect("outside root");
        File::create(outside.path().join("secret.txt")).unwrap();
        symlink(
            outside.path().join("secret.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        let map = RootMap::from_specs(&[format!("project={}", root.path().display())]).unwrap();
        assert!(matches!(
            map.resolve("project", "link.txt"),
            Err(PathPolicyError::OutsideRoot)
        ));
    }

    #[test]
    fn permits_nested_file_and_reports_version() {
        let root = tempdir().expect("temp root");
        create_dir_all(root.path().join("src")).unwrap();
        let file = root.path().join("src/main.rs");
        File::create(&file).unwrap();
        let map = RootMap::from_specs(&[format!("project={}", root.path().display())]).unwrap();
        let resolved = map.resolve("project", "src/main.rs").unwrap();
        let info = file_info(&resolved).unwrap();
        assert_eq!(info.node_type, NodeType::File as i32);
        assert!(info.version.contains(':'));
    }

    #[test]
    fn resolves_target_when_parent_is_missing() {
        let root = tempdir().expect("temp root");
        let map = RootMap::from_specs(&[format!("project={}", root.path().display())]).unwrap();
        let resolved = map
            .resolve_for_create("project", "nested/deep/file.txt")
            .unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(root.path())
                .unwrap()
                .join("nested/deep/file.txt")
        );
    }
}
