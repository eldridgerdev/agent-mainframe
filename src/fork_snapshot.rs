//! Capture the working content of a Git checkout before creating a fork.
//!
//! The index is observed for freshness, never applied to the destination.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::{CString, OsString};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()
        .context("Failed to run git")?;
    ensure!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}
fn line(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(bytes.pop() == Some(b'\n'), "Unexpected git output");
    Ok(bytes)
}
fn name(component: &std::ffi::OsStr) -> Result<CString> {
    Ok(CString::new(component.as_bytes())?)
}
fn path_parts(path: &Path) -> Result<Vec<CString>> {
    let mut parts = Vec::new();
    for part in path.components() {
        match part {
            Component::Normal(s) if s != ".git" => parts.push(name(s)?),
            _ => bail!("Unsafe fork path {:?}", path),
        }
    }
    ensure!(!parts.is_empty(), "Empty fork path");
    Ok(parts)
}
fn open_dir_at(parent: &File, component: &CString) -> io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn parent_at(root: &File, path: &Path) -> Result<Option<(File, CString)>> {
    let mut parts = path_parts(path)?;
    let last = parts.pop().context("Missing filename")?;
    let mut dir = root.try_clone()?;
    for part in parts {
        match open_dir_at(&dir, &part) {
            Ok(next) => dir = next,
            Err(err) if matches!(err.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENOTDIR)) => {
                return Ok(None);
            }
            Err(err) => {
                return Err(err).with_context(|| format!("Cannot inspect parent of {:?}", path));
            }
        }
    }
    Ok(Some((dir, last)))
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Entry {
    Missing,
    Regular {
        digest: [u8; 32],
        executable: bool,
        spool: PathBuf,
    },
    Symlink(Vec<u8>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    head: Vec<u8>,
    symbolic_head: Vec<u8>,
    gitdir: PathBuf,
    gitdir_dev: u64,
    gitdir_ino: u64,
    root_dev: u64,
    root_ino: u64,
    index: [u8; 32],
    status: Vec<u8>,
}
#[derive(Debug)]
pub(crate) struct Snapshot {
    pub head: String,
    pub entries: Vec<(PathBuf, Entry)>,
    root: PathBuf,
    fingerprint: Fingerprint,
    _spool: tempfile::TempDir,
}
fn fingerprint(root: &Path) -> Result<Fingerprint> {
    ensure!(
        git(root, &["rev-parse", "--show-cdup"])? == b"\n",
        "Fork source must be a repository root"
    );
    let head = line(git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?)?;
    let gitdir = PathBuf::from(OsString::from_vec(line(git(
        root,
        &["rev-parse", "--absolute-git-dir"],
    )?)?))
    .canonicalize()?;
    let gitdir_meta = gitdir.metadata()?;
    let symbolic = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(root)
        .output()?;
    ensure!(
        symbolic.status.success() || symbolic.status.code() == Some(1),
        "Cannot read symbolic HEAD"
    );
    let symbolic_head = symbolic.stdout;
    let index_path = PathBuf::from(OsString::from_vec(line(git(
        root,
        &["rev-parse", "--git-path", "index"],
    )?)?));
    let index_path = if index_path.is_absolute() {
        index_path
    } else {
        root.join(index_path)
    };
    let index: [u8; 32] =
        Sha256::digest(fs::read(index_path).context("Cannot read source index")?).into();
    let meta = root.metadata()?;
    let status = git(
        root,
        &[
            "-c",
            "status.renames=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
            "--ignore-submodules=none",
        ],
    )?;
    Ok(Fingerprint {
        head,
        symbolic_head,
        gitdir,
        gitdir_dev: gitdir_meta.dev(),
        gitdir_ino: gitdir_meta.ino(),
        root_dev: meta.dev(),
        root_ino: meta.ino(),
        index,
        status,
    })
}
fn paths(status: &[u8]) -> Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    for record in status.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        ensure!(
            record.len() > 3 && record[2] == b' ',
            "Malformed Git status record"
        );
        let xy = &record[..2];
        ensure!(
            !xy.contains(&b'U') && xy != b"AA" && xy != b"DD",
            "Resolve conflicts before forking"
        );
        ensure!(
            !xy.contains(&b'R') && !xy.contains(&b'C'),
            "Unexpected rename record"
        );
        let path = PathBuf::from(OsString::from_vec(record[3..].to_vec()));
        path_parts(&path)?;
        paths.insert(path);
    }
    Ok(paths.into_iter().collect())
}

// Git deliberately omits untracked FIFOs, sockets, and devices from status.
// Enumerate directories without following links, consulting Git's own ignore
// rules before descending so ignored build trees are not treated as fork data.
fn ignored(root: &Path, path: &Path) -> Result<bool> {
    use std::process::Stdio;
    let mut child = Command::new("git")
        .args(["check-ignore", "--no-index", "-q", "-z", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().context("Missing git stdin")?;
    let sent = input
        .write_all(path.as_os_str().as_bytes())
        .and_then(|()| input.write_all(&[0]));
    drop(input);
    let out = child.wait_with_output()?;
    sent.context("Cannot send path to git check-ignore")?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "Cannot evaluate ignore rules for {:?}: {}",
            path,
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

fn check_special_entries(
    root: &Path,
    directory: &File,
    prefix: &Path,
    gitlinks: &BTreeSet<PathBuf>,
) -> Result<()> {
    struct Dir(*mut libc::DIR);
    impl Drop for Dir {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(io::Error::last_os_error().into());
    }
    let stream = Dir(stream);
    loop {
        // readdir returns null both at end and on error; clear errno first.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let errno = unsafe { libc::__errno_location() };
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        let errno = unsafe { libc::__error() };
        unsafe {
            *errno = 0;
        }
        let raw = unsafe { libc::readdir(stream.0) };
        if raw.is_null() {
            let code = unsafe { *errno };
            if code != 0 {
                return Err(io::Error::from_raw_os_error(code).into());
            }
            break;
        }
        let n = unsafe { std::ffi::CStr::from_ptr((*raw).d_name.as_ptr()) }.to_owned();
        if n.as_bytes() == b"." || n.as_bytes() == b".." {
            continue;
        }
        if n.as_bytes() == b".git" {
            ensure!(
                prefix.as_os_str().is_empty(),
                "Nested Git checkout is not supported: {:?}",
                prefix
            );
            continue;
        }
        let path = prefix.join(OsString::from_vec(n.as_bytes().to_vec()));
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                n.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(io::Error::last_os_error())
                .with_context(|| format!("Cannot enumerate {:?}", path));
        }
        let kind = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
        if kind == libc::S_IFDIR {
            if !gitlinks.contains(&path) && !ignored(root, &path)? {
                let child = open_dir_at(directory, &n)
                    .with_context(|| format!("Cannot enumerate {:?}", path))?;
                check_special_entries(root, &child, &path, gitlinks)?;
            }
        } else if kind != libc::S_IFREG && kind != libc::S_IFLNK && !ignored(root, &path)? {
            bail!("Unsupported fork entry {:?}", path);
        }
    }
    Ok(())
}
fn check_supported(root: &Path, changed: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let mut gitlinks = BTreeSet::new();
    let sparse = Command::new("git")
        .args(["config", "--bool", "--get", "core.sparseCheckout"])
        .current_dir(root)
        .output()?;
    match sparse.status.code() {
        Some(0) => ensure!(
            sparse.stdout == b"false\n",
            "Sparse checkout is not supported for forks"
        ),
        Some(1) => (),
        _ => bail!("Cannot determine sparse checkout configuration"),
    }
    let entries = git(root, &["ls-files", "-v", "-z"])?;
    ensure!(
        !entries
            .split(|b| *b == 0)
            .any(|record| matches!(record.first(), Some(b'S' | b's'))),
        "Sparse checkout is not supported for forks"
    );
    let stages = git(root, &["ls-files", "--stage", "-z"])?;
    for record in stages.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .context("Malformed index record")?;
        if record.starts_with(b"160000 ") {
            let path = PathBuf::from(OsString::from_vec(record[tab + 1..].to_vec()));
            ensure!(
                !changed.contains(&path),
                "Changed submodule cannot be forked: {:?}",
                path
            );
            gitlinks.insert(path);
        }
    }
    let tree = git(root, &["ls-tree", "-r", "-z", "HEAD"])?;
    for record in tree.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .context("Malformed tree record")?;
        if record.starts_with(b"160000 ") {
            let path = PathBuf::from(OsString::from_vec(record[tab + 1..].to_vec()));
            ensure!(
                !changed.contains(&path),
                "Changed submodule cannot be forked: {:?}",
                path
            );
            gitlinks.insert(path);
        }
    }
    Ok(gitlinks)
}
fn capture_entry(root: &File, path: &Path, spool: &Path) -> Result<Entry> {
    let Some((dir, leaf)) = parent_at(root, path)? else {
        return Ok(Entry::Missing);
    };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::NotFound {
            return Ok(Entry::Missing);
        }
        return Err(err).with_context(|| format!("Cannot inspect {:?}", path));
    }
    let stat = unsafe { stat.assume_init() };
    match stat.st_mode & libc::S_IFMT {
        libc::S_IFLNK => {
            let mut buf = vec![0; 256];
            loop {
                let n = unsafe {
                    libc::readlinkat(
                        dir.as_raw_fd(),
                        leaf.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n < 0 {
                    return Err(io::Error::last_os_error())
                        .with_context(|| format!("Cannot read link {:?}", path));
                }
                if (n as usize) < buf.len() {
                    buf.truncate(n as usize);
                    return Ok(Entry::Symlink(buf));
                }
                ensure!(buf.len() < 1024 * 1024, "Link target too long: {:?}", path);
                buf.resize(buf.len() * 2, 0);
            }
        }
        libc::S_IFREG => {
            let fd = unsafe {
                libc::openat(
                    dir.as_raw_fd(),
                    leaf.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("Cannot read {:?}", path));
            }
            let mut src = unsafe { File::from_raw_fd(fd) };
            let before = src.metadata()?;
            ensure!(
                before.is_file() && before.dev() == stat.st_dev && before.ino() == stat.st_ino,
                "Source changed while reading {:?}",
                path
            );
            let mut dest = File::create(spool)?;
            let mut hash = Sha256::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hash.update(&buf[..n]);
                dest.write_all(&buf[..n])?;
            }
            let after = src.metadata()?;
            ensure!(
                before.len() == after.len()
                    && before.mtime() == after.mtime()
                    && before.mtime_nsec() == after.mtime_nsec()
                    && before.ctime() == after.ctime()
                    && before.ctime_nsec() == after.ctime_nsec(),
                "Source changed while reading {:?}",
                path
            );
            Ok(Entry::Regular {
                digest: hash.finalize().into(),
                executable: before.mode() & 0o111 != 0,
                spool: spool.to_owned(),
            })
        }
        // A tracked file can be replaced by a directory. Its children are
        // captured separately by Git's untracked-files=all enumeration.
        libc::S_IFDIR => Ok(Entry::Missing),
        _ => bail!("Unsupported fork entry {:?}", path),
    }
}
impl Snapshot {
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        let snapshot = Self::capture_once(root)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn capture_once(root: &Path) -> Result<Self> {
        let root = root.canonicalize().context("Cannot resolve fork source")?;
        let before = fingerprint(&root)?;
        let changed = paths(&before.status)?;
        let gitlinks = check_supported(&root, &changed)?;
        let dir = File::open(&root)?;
        check_special_entries(&root, &dir, Path::new(""), &gitlinks)?;
        let spool = tempfile::tempdir().context("Cannot create fork snapshot spool")?;
        let mut entries = Vec::new();
        for (i, path) in changed.into_iter().enumerate() {
            entries.push((
                path.clone(),
                capture_entry(&dir, &path, &spool.path().join(i.to_string()))?,
            ));
        }
        ensure!(
            before == fingerprint(&root)?,
            "Source changed while preparing fork"
        );
        let head = String::from_utf8(before.head.clone()).context("Invalid Git HEAD")?;
        Ok(Self {
            head,
            entries,
            root,
            fingerprint: before,
            _spool: spool,
        })
    }

    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.fingerprint == fingerprint(&self.root)?,
            "Fork source changed; prepare it again"
        );
        let current = Self::capture_once(&self.root)?;
        ensure!(
            self.entries.len() == current.entries.len(),
            "Fork paths changed; prepare again"
        );
        for ((path, original), (new_path, new)) in self.entries.iter().zip(&current.entries) {
            let same = match (original, new) {
                (Entry::Missing, Entry::Missing) => true,
                (Entry::Symlink(a), Entry::Symlink(b)) => a == b,
                (
                    Entry::Regular {
                        digest: a,
                        executable: am,
                        ..
                    },
                    Entry::Regular {
                        digest: b,
                        executable: bm,
                        ..
                    },
                ) => a == b && am == bm,
                _ => false,
            };
            ensure!(
                path == new_path && same,
                "Fork source changed at {:?}; prepare it again",
                path
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn run(root: &Path, args: &[&str]) {
        let result = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &["init", "-q"]);
        fs::write(dir.path().join("old name"), "head").unwrap();
        fs::write(dir.path().join("deleted"), "head").unwrap();
        run(dir.path(), &["add", "."]);
        run(dir.path(), &["commit", "-qm", "initial"]);
        dir
    }
    #[test]
    fn snapshot_records_final_working_bytes_non_utf8_and_symlink_not_index() {
        let dir = fixture();
        let root = dir.path();
        run(root, &["mv", "old name", "new name"]);
        fs::write(root.join("new name"), "working after staged rename").unwrap();
        run(root, &["rm", "-q", "deleted"]);
        fs::write(root.join("deleted"), "recreated").unwrap();
        let raw = PathBuf::from(OsString::from_vec(b"untracked\n\xff".to_vec()));
        fs::write(root.join(&raw), b"bytes").unwrap();
        fs::set_permissions(root.join(&raw), fs::Permissions::from_mode(0o755)).unwrap();
        symlink("../outside-missing", root.join("link")).unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        assert_eq!(
            snapshot.head.as_bytes(),
            line(git(root, &["rev-parse", "HEAD"]).unwrap()).unwrap()
        );
        let entry = |path: &Path| &snapshot.entries.iter().find(|(p, _)| p == path).unwrap().1;
        assert!(matches!(entry(Path::new("old name")), Entry::Missing));
        match entry(Path::new("new name")) {
            Entry::Regular { spool, .. } => {
                assert_eq!(fs::read(spool).unwrap(), b"working after staged rename")
            }
            other => panic!("{other:?}"),
        }
        match entry(Path::new("deleted")) {
            Entry::Regular { spool, .. } => assert_eq!(fs::read(spool).unwrap(), b"recreated"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            entry(&raw),
            Entry::Regular {
                executable: true,
                ..
            }
        ));
        assert_eq!(
            entry(Path::new("link")),
            &Entry::Symlink(b"../outside-missing".to_vec())
        );
        snapshot.validate().unwrap();
        fs::write(root.join("new name"), "changed after preview").unwrap();
        assert!(snapshot.validate().is_err());
    }
    #[test]
    fn snapshot_rejects_fifo_and_stale_index() {
        let dir = fixture();
        let root = dir.path();
        fs::write(root.join("new"), "value").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        run(root, &["add", "new"]);
        assert!(snapshot.validate().is_err());
        fs::remove_file(root.join("old name")).unwrap();
        let path = CString::new(root.join("old name").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(Snapshot::capture(root).is_err());
    }

    #[test]
    fn untracked_special_file_is_rejected_unless_ignored() {
        let dir = fixture();
        let root = dir.path();
        let path = CString::new(root.join("fifo").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        assert!(Snapshot::capture(root).is_err());
        fs::write(root.join(".gitignore"), "fifo\n").unwrap();
        Snapshot::capture(root).unwrap();
    }

    #[test]
    fn sparse_configuration_and_nested_repository_are_refused() {
        let dir = fixture();
        let root = dir.path();
        run(root, &["config", "core.sparseCheckout", "true"]);
        assert!(
            Snapshot::capture(root)
                .unwrap_err()
                .to_string()
                .contains("Sparse")
        );
        run(root, &["config", "--unset", "core.sparseCheckout"]);
        fs::create_dir(root.join("nested")).unwrap();
        run(&root.join("nested"), &["init", "-q"]);
        assert!(
            Snapshot::capture(root)
                .unwrap_err()
                .to_string()
                .contains("Nested Git")
        );
    }
}
