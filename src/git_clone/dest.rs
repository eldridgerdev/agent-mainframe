//! Destination derivation and validation for "new project from git URL".
//!
//! Pure with respect to the application: registered project paths are passed
//! in. Nothing here creates, modifies or deletes anything on disk.

use std::fmt;
use std::path::{Path, PathBuf};

/// Why a destination cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestError {
    /// Something already lives at the path (file, directory of any size,
    /// symlink, dangling or not).
    Exists(PathBuf),
    /// An AMF project is already registered at this path.
    AlreadyRegistered(PathBuf),
    /// The parent directory does not exist.
    ParentMissing(PathBuf),
    /// The parent exists but is not a directory.
    ParentNotDirectory(PathBuf),
    /// The parent directory does not appear to be writable.
    ParentUnwritable(PathBuf),
    /// The path has no leaf name (for example `/` or ends in `..`).
    NoLeafName(PathBuf),
}

impl fmt::Display for DestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exists(p) => write!(
                f,
                "{} already exists. Choose a different destination. If it is a \
                 leftover from an earlier clone that crashed, remove it yourself \
                 and try again.",
                p.display()
            ),
            Self::AlreadyRegistered(p) => write!(
                f,
                "{} is already registered as an AMF project. Choose a different destination.",
                p.display()
            ),
            Self::ParentMissing(p) => write!(
                f,
                "The parent directory {} does not exist. Create it or choose another destination.",
                p.display()
            ),
            Self::ParentNotDirectory(p) => write!(
                f,
                "The parent path {} is not a directory. Choose another destination.",
                p.display()
            ),
            Self::ParentUnwritable(p) => write!(
                f,
                "The parent directory {} is not writable. Fix its permissions or choose another destination.",
                p.display()
            ),
            Self::NoLeafName(p) => write!(
                f,
                "{} has no final folder name. Choose a destination such as <dir>/<repo>.",
                p.display()
            ),
        }
    }
}

impl std::error::Error for DestError {}

/// Default destination: `root/repo_name`.
pub fn destination(root: &Path, repo_name: &str) -> PathBuf {
    root.join(repo_name)
}

/// Canonicalize `parent` and re-attach `leaf`; `None` if the parent cannot be
/// resolved.
fn canonical_parent_leaf(path: &Path) -> Option<PathBuf> {
    let leaf = path.file_name()?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    Some(std::fs::canonicalize(parent).ok()?.join(leaf))
}

/// Best-effort canonical form used for comparing paths: the full path if it
/// resolves, else canonical parent + leaf, else the path as given.
fn comparable(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .ok()
        .or_else(|| canonical_parent_leaf(path))
        .unwrap_or_else(|| path.to_path_buf())
}

/// Whether the current process can create entries in `dir`.
///
/// On Unix this asks the kernel via `access(W_OK | X_OK)`, which honours
/// mode bits, ownership, ACLs and read-only mounts without creating anything.
/// Limits: it uses the real (not effective) uid, cannot see quota or disk-full
/// conditions, and is advisory (time-of-check/time-of-use). The clone itself
/// remains the final authority. Elsewhere it falls back to the read-only
/// permission flag.
fn dir_is_writable(dir: &Path, meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let _ = meta;
        match std::ffi::CString::new(dir.as_os_str().as_bytes()) {
            Ok(c) => unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 },
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        !meta.permissions().readonly()
    }
}

/// Validate that `dest` can be used for a new clone. Never touches the disk
/// beyond reading metadata.
///
/// `registered` are the repo paths of existing AMF projects.
pub fn validate(dest: &Path, registered: &[PathBuf]) -> Result<(), DestError> {
    if dest.file_name().is_none() {
        return Err(DestError::NoLeafName(dest.to_path_buf()));
    }

    // Registered first: it is the more specific explanation when the project
    // directory also exists.
    let target = comparable(dest);
    if registered.iter().any(|r| comparable(r) == target) {
        return Err(DestError::AlreadyRegistered(dest.to_path_buf()));
    }

    // Never follow a symlink at the leaf: a dangling one still occupies it.
    // Any other lookup failure (for example the parent is a file or is
    // unreadable) is explained by the parent checks below.
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(DestError::Exists(dest.to_path_buf()));
    }

    let parent = parent_of(dest);
    let meta = match std::fs::metadata(&parent) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(DestError::ParentMissing(parent));
        }
        Err(_) => return Err(DestError::ParentUnwritable(parent)),
    };
    if !meta.is_dir() {
        return Err(DestError::ParentNotDirectory(parent));
    }
    if !dir_is_writable(&parent, &meta) {
        return Err(DestError::ParentUnwritable(parent));
    }
    Ok(())
}

fn parent_of(dest: &Path) -> PathBuf {
    match dest.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_joins_root_and_name() {
        assert_eq!(
            destination(Path::new("/home/u/code"), "repo"),
            PathBuf::from("/home/u/code/repo")
        );
    }

    #[test]
    fn free_path_in_writable_parent_is_ok() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(validate(&t.path().join("new"), &[]), Ok(()));
        assert!(!t.path().join("new").exists(), "validate must not create");
    }

    #[test]
    fn existing_empty_dir_blocks() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("d");
        std::fs::create_dir(&d).unwrap();
        assert_eq!(validate(&d, &[]), Err(DestError::Exists(d.clone())));
        assert!(d.exists());
        assert!(DestError::Exists(d).to_string().contains("crashed"));
    }

    #[test]
    fn existing_file_blocks() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("f");
        std::fs::write(&f, "x").unwrap();
        assert_eq!(validate(&f, &[]), Err(DestError::Exists(f)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_block_including_dangling() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let ok = t.path().join("ok");
        symlink(&real, &ok).unwrap();
        assert_eq!(validate(&ok, &[]), Err(DestError::Exists(ok)));
        let dangling = t.path().join("dangling");
        symlink(t.path().join("nowhere"), &dangling).unwrap();
        assert_eq!(validate(&dangling, &[]), Err(DestError::Exists(dangling)));
        assert!(!t.path().join("nowhere").exists());
    }

    #[test]
    fn registered_path_blocks_even_if_absent() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("proj");
        assert_eq!(
            validate(&p, std::slice::from_ref(&p)),
            Err(DestError::AlreadyRegistered(p))
        );
    }

    #[test]
    fn registered_path_with_dotdot_and_trailing_forms_blocks() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("proj");
        std::fs::create_dir(&p).unwrap();
        let alias = t.path().join("sub/../proj");
        std::fs::create_dir(t.path().join("sub")).unwrap();
        assert!(matches!(
            validate(&p, &[alias]),
            Err(DestError::AlreadyRegistered(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn registered_via_symlinked_parent_blocks() {
        use std::os::unix::fs::symlink;
        let t = tempfile::tempdir().unwrap();
        let real = t.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = t.path().join("link");
        symlink(&real, &link).unwrap();
        // Registered through the real path; candidate typed through the link.
        let registered = real.join("proj");
        let dest = link.join("proj");
        assert_eq!(
            validate(&dest, &[registered]),
            Err(DestError::AlreadyRegistered(dest))
        );
    }

    #[test]
    fn unrelated_registered_paths_do_not_block() {
        let t = tempfile::tempdir().unwrap();
        let other = t.path().join("other");
        assert_eq!(validate(&t.path().join("proj"), &[other]), Ok(()));
    }

    #[test]
    fn missing_parent_blocks() {
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("nope");
        assert_eq!(
            validate(&parent.join("x"), &[]),
            Err(DestError::ParentMissing(parent.clone()))
        );
        assert!(!parent.exists());
    }

    #[test]
    fn parent_that_is_a_file_blocks() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("file");
        std::fs::write(&f, "x").unwrap();
        assert_eq!(
            validate(&f.join("x"), &[]),
            Err(DestError::ParentNotDirectory(f))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_parent_blocks() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let parent = t.path().join("ro");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = validate(&parent.join("x"), &[]);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Root bypasses mode bits; only assert when we are not root.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(result, Err(DestError::ParentUnwritable(parent)));
        }
    }

    #[test]
    fn no_leaf_name_blocks() {
        assert!(matches!(
            validate(Path::new("/"), &[]),
            Err(DestError::NoLeafName(_))
        ));
    }

    #[test]
    fn messages_are_distinct() {
        let p = PathBuf::from("/x/y");
        let msgs = [
            DestError::Exists(p.clone()),
            DestError::AlreadyRegistered(p.clone()),
            DestError::ParentMissing(p.clone()),
            DestError::ParentNotDirectory(p.clone()),
            DestError::ParentUnwritable(p.clone()),
            DestError::NoLeafName(p),
        ]
        .map(|e| e.to_string());
        for (i, a) in msgs.iter().enumerate() {
            for b in &msgs[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }
}
