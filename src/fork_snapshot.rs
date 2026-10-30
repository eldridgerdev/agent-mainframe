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

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(root).env("GIT_OPTIONAL_LOCKS", "0");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(variable);
    }
    command
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = git_command(root)
        .args(args)
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
    index_file: PathBuf,
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
    fingerprint_excluding(root, None)
}

fn fingerprint_excluding(root: &Path, excluded: Option<&Path>) -> Result<Fingerprint> {
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
    let symbolic = git_command(root)
        .args(["symbolic-ref", "-q", "HEAD"])
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
        Sha256::digest(fs::read(&index_path).context("Cannot read source index")?).into();
    let meta = root.metadata()?;
    let mut command = git_command(root);
    command.args([
        "-c",
        "status.renames=false",
        "-c",
        "core.filemode=true",
        "-c",
        "core.fsmonitor=false",
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--no-renames",
        "--ignore-submodules=none",
    ]);
    if let Some(excluded) = excluded {
        path_parts(excluded)?;
        let mut spec = OsString::from(":(exclude,literal)");
        spec.push(excluded.as_os_str());
        command.arg("--").arg(".").arg(spec);
    }
    let output = command.output().context("Failed to inspect fork status")?;
    ensure!(
        output.status.success(),
        "Cannot inspect fork status: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let status = output.stdout;
    Ok(Fingerprint {
        head,
        symbolic_head,
        gitdir,
        gitdir_dev: gitdir_meta.dev(),
        gitdir_ino: gitdir_meta.ino(),
        root_dev: meta.dev(),
        root_ino: meta.ino(),
        index_file: index_path,
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
    let mut child = git_command(root)
        .args(["check-ignore", "--no-index", "-q", "-z", "--stdin"])
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

// libc stat field widths differ by platform; the casts are needed on macOS.
#[allow(clippy::unnecessary_cast)]
fn check_special_entries(
    root: &Path,
    directory: &File,
    prefix: &Path,
    gitlinks: &BTreeSet<PathBuf>,
    reject_ignored: bool,
    excluded: Option<(&Path, u64, u64)>,
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
        let stat = unsafe { stat.assume_init() };
        let kind = stat.st_mode & libc::S_IFMT;
        if let Some((owned, device, inode)) = excluded
            && path == owned
        {
            ensure!(
                kind == libc::S_IFDIR
                    && stat.st_dev as u64 == device
                    && stat.st_ino as u64 == inode,
                "Owned fork checkout changed during source validation"
            );
            continue;
        }
        if kind == libc::S_IFDIR {
            if !gitlinks.contains(&path) && (reject_ignored || !ignored(root, &path)?) {
                let child = open_dir_at(directory, &n)
                    .with_context(|| format!("Cannot enumerate {:?}", path))?;
                check_special_entries(root, &child, &path, gitlinks, reject_ignored, excluded)?;
            }
        } else if kind != libc::S_IFREG
            && kind != libc::S_IFLNK
            && (reject_ignored || !ignored(root, &path)?)
        {
            bail!("Unsupported fork entry {:?}", path);
        }
    }
    Ok(())
}
fn check_supported(root: &Path, changed: &[PathBuf]) -> Result<BTreeSet<PathBuf>> {
    let mut gitlinks = BTreeSet::new();
    let sparse = git_command(root)
        .args(["config", "--bool", "--get", "core.sparseCheckout"])
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
    ensure!(
        !entries
            .split(|b| *b == 0)
            .any(|record| record.first().is_some_and(u8::is_ascii_lowercase)),
        "Assume-unchanged index entries are not supported for forks"
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
#[allow(clippy::unnecessary_cast)]
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
                before.is_file()
                    && before.dev() == stat.st_dev as u64
                    && before.ino() == stat.st_ino as u64,
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
    pub(crate) fn is_branch(&self, branch: &str) -> bool {
        self.fingerprint.symbolic_head == format!("refs/heads/{branch}\n").as_bytes()
    }
    pub(crate) fn touches(&self, path: &Path) -> bool {
        self.entries
            .iter()
            .any(|(changed, _)| changed.starts_with(path) || path.starts_with(changed))
    }
    pub(crate) fn capture(root: &Path) -> Result<Self> {
        let snapshot = Self::capture_once(root, None)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn capture_once(root: &Path, excluded: Option<(&Path, u64, u64)>) -> Result<Self> {
        let root = root.canonicalize().context("Cannot resolve fork source")?;
        let before = fingerprint_excluding(&root, excluded.map(|e| e.0))?;
        let changed = paths(&before.status)?;
        let gitlinks = check_supported(&root, &changed)?;
        let dir = File::open(&root)?;
        check_special_entries(&root, &dir, Path::new(""), &gitlinks, false, excluded)?;
        let spool = tempfile::tempdir().context("Cannot create fork snapshot spool")?;
        let mut entries = Vec::new();
        for (i, path) in changed.into_iter().enumerate() {
            entries.push((
                path.clone(),
                capture_entry(&dir, &path, &spool.path().join(i.to_string()))?,
            ));
        }
        ensure!(
            before == fingerprint_excluding(&root, excluded.map(|e| e.0))?,
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
        self.validate_excluding(None)
    }

    fn validate_excluding(&self, excluded: Option<(&Path, u64, u64)>) -> Result<()> {
        ensure!(
            self.fingerprint == fingerprint_excluding(&self.root, excluded.map(|e| e.0))?,
            "Fork source changed; prepare it again"
        );
        let current = Self::capture_once(&self.root, excluded)?;
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

/// Add a generated fork file without replacing source data or following a
/// source-controlled parent link. The caller owns this fresh checkout.
pub(crate) fn write_auxiliary(destination: &Path, relative: &Path, bytes: &[u8]) -> Result<()> {
    let parts = path_parts(relative)?;
    let (name, parents) = parts.split_last().context("Missing auxiliary filename")?;
    let mut directory = File::open(destination)?;
    for component in parents {
        let rc = unsafe { libc::mkdirat(directory.as_raw_fd(), component.as_ptr(), 0o755) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::AlreadyExists {
                return Err(err)
                    .with_context(|| format!("Cannot create parent for {:?}", relative));
            }
        }
        directory = open_dir_at(&directory, component)
            .with_context(|| format!("Unsafe parent for {:?}", relative))?;
    }
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).with_context(|| {
            format!(
                "Cannot create {:?}; it may already contain source data",
                relative
            )
        });
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    file.write_all(bytes)
        .with_context(|| format!("Cannot write {:?}", relative))?;
    Ok(())
}

/// Preserve the existing worktree settings merge for an unchanged tracked
/// settings file. A source-changed settings path is refused by the caller.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn merge_settings(destination: &Path, source: &[u8]) -> Result<()> {
    let relative = Path::new(".claude/settings.local.json");
    let root = File::open(destination)?;
    let Some((parent, leaf)) = parent_at(&root, relative)? else {
        return write_auxiliary(destination, relative, source);
    };
    let Some(before) = destination_entry(&parent, &leaf)? else {
        return write_auxiliary(destination, relative, source);
    };
    ensure!(
        before.mode & libc::S_IFMT == libc::S_IFREG,
        "Fork settings target is not a regular file"
    );
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("Cannot read fork settings");
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "Fork settings exceed size limit"
    );
    ensure!(
        destination_entry(&parent, &leaf)?.as_ref() == Some(&before),
        "Fork settings changed while reading"
    );
    let source: serde_json::Value = serde_json::from_slice(source)?;
    let mut target: serde_json::Value = serde_json::from_slice(&bytes)?;
    if let (Some(src), Some(dst)) = (source.as_object(), target.as_object_mut()) {
        for (key, value) in src {
            let entry = dst
                .entry(key.clone())
                .or_insert_with(|| serde_json::json!({}));
            if let (Some(src), Some(dst)) = (value.as_object(), entry.as_object_mut()) {
                for (key, value) in src {
                    dst.insert(key.clone(), value.clone());
                }
            } else {
                *entry = value.clone();
            }
        }
    }
    let temp = CString::new(format!(".amf-fork-settings-{}", uuid::Uuid::new_v4()))?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_CREAT | libc::O_EXCL,
            (before.mode & 0o777) as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("Cannot stage fork settings");
    }
    let mut output = unsafe { File::from_raw_fd(fd) };
    let result = (|| -> Result<()> {
        output.write_all(serde_json::to_string_pretty(&target)?.as_bytes())?;
        output.write_all(b"\n")?;
        if unsafe { libc::fchmod(output.as_raw_fd(), before.mode & 0o777) } < 0 {
            return Err(io::Error::last_os_error())
                .context("Cannot preserve fork settings permissions");
        }
        ensure!(
            destination_entry(&parent, &leaf)?.as_ref() == Some(&before),
            "Fork settings changed before merge"
        );
        if unsafe {
            libc::renameat(
                parent.as_raw_fd(),
                temp.as_ptr(),
                parent.as_raw_fd(),
                leaf.as_ptr(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error()).context("Cannot publish fork settings");
        }
        Ok(())
    })();
    if result.is_err() && unsafe { libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0) } < 0 {
        return Err(io::Error::last_os_error()).context("Cannot remove staged fork settings");
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestinationEntry {
    device: u64,
    inode: u64,
    mode: libc::mode_t,
    size: i64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
}

// libc stat field widths differ by platform (macOS: i32 dev); the casts are needed there.
#[allow(clippy::unnecessary_cast)]
fn destination_entry(directory: &File, name: &CString) -> Result<Option<DestinationEntry>> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } < 0
    {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(err.into());
    }
    let stat = unsafe { stat.assume_init() };
    Ok(Some(DestinationEntry {
        device: stat.st_dev as u64,
        inode: stat.st_ino as u64,
        mode: stat.st_mode,
        size: stat.st_size as i64,
        modified_seconds: stat.st_mtime as i64,
        modified_nanos: stat.st_mtime_nsec as i64,
        changed_seconds: stat.st_ctime as i64,
        changed_nanos: stat.st_ctime_nsec as i64,
    }))
}

fn entry_at(root: &File, path: &Path) -> Result<Option<DestinationEntry>> {
    match parent_at(root, path)? {
        Some((parent, name)) => destination_entry(&parent, &name),
        None => Ok(None),
    }
}
fn directory_identity_matches(a: &DestinationEntry, b: &DestinationEntry) -> bool {
    a.device == b.device && a.inode == b.inode && a.mode == b.mode
}
fn validate_destination_identity(
    root: &Path,
    fingerprint: &Fingerprint,
    git_entry: &DestinationEntry,
    fd: &File,
) -> Result<()> {
    let current = root.metadata()?;
    ensure!(
        current.dev() == fingerprint.root_dev && current.ino() == fingerprint.root_ino,
        "Fork destination was replaced"
    );
    let gitdir = fingerprint.gitdir.metadata()?;
    ensure!(
        gitdir.dev() == fingerprint.gitdir_dev && gitdir.ino() == fingerprint.gitdir_ino,
        "Fork destination Git directory was replaced"
    );
    ensure!(
        destination_entry(fd, &CString::new(".git")?)?.as_ref() == Some(git_entry),
        "Fork destination Git entry changed"
    );
    let current_index: [u8; 32] = Sha256::digest(fs::read(&fingerprint.index_file)?).into();
    ensure!(
        current_index == fingerprint.index,
        "Fork destination index changed"
    );
    ensure!(
        line(git(root, &["rev-parse", "--verify", "HEAD^{commit}"])?)? == fingerprint.head,
        "Fork destination HEAD changed"
    );
    Ok(())
}
fn ensure_parent(
    root: &File,
    path: &Path,
    baseline: &std::collections::BTreeMap<PathBuf, Option<DestinationEntry>>,
    owned: &mut std::collections::BTreeMap<PathBuf, DestinationEntry>,
    removed: &BTreeSet<PathBuf>,
) -> Result<(File, CString)> {
    let mut components = path_parts(path)?;
    let leaf = components.pop().context("Missing destination filename")?;
    let mut dir = root.try_clone()?;
    let mut relative = PathBuf::new();
    for component in components {
        relative.push(OsString::from_vec(component.as_bytes().to_vec()));
        let expected = owned
            .get(&relative)
            .or_else(|| baseline.get(&relative).and_then(Option::as_ref));
        let actual = destination_entry(&dir, &component)?;
        if let Some(expected) = expected.filter(|entry| entry.mode & libc::S_IFMT == libc::S_IFDIR)
        {
            let actual = actual.as_ref().context("Destination parent disappeared")?;
            ensure!(
                directory_identity_matches(expected, actual),
                "Destination parent changed: {:?}",
                relative
            );
        } else {
            ensure!(
                expected.is_none() || removed.contains(&relative),
                "Destination parent was removed externally: {:?}",
                relative
            );
            ensure!(
                actual.is_none(),
                "Unexpected destination parent: {:?}",
                relative
            );
            let rc = unsafe { libc::mkdirat(dir.as_raw_fd(), component.as_ptr(), 0o755) };
            if rc < 0 {
                return Err(io::Error::last_os_error())
                    .with_context(|| format!("Cannot create directory {:?}", relative));
            }
            let entry =
                destination_entry(&dir, &component)?.context("Created directory disappeared")?;
            owned.insert(relative.clone(), entry);
        }
        dir = open_dir_at(&dir, &component)
            .with_context(|| format!("Cannot open destination parent {:?}", relative))?;
    }
    Ok((dir, leaf))
}
impl Snapshot {
    /// Transfer only to a freshly created, pristine checkout of this exact HEAD.
    /// Errors retain the partially populated destination for manual recovery.
    pub(crate) fn transfer(&self, destination: &Path) -> Result<()> {
        self.transfer_inner(destination)
            .with_context(|| format!("Fork checkout retained at {}", destination.display()))
    }

    fn transfer_inner(&self, destination: &Path) -> Result<()> {
        use std::collections::BTreeMap;
        let destination = destination
            .canonicalize()
            .context("Cannot resolve fork destination")?;
        ensure!(
            destination != self.root,
            "Fork destination is the source checkout"
        );
        let destination_meta = destination.metadata()?;
        let relative = destination.strip_prefix(&self.root).ok();
        if let Some(relative) = relative {
            ensure!(
                !self.touches(relative),
                "Fork destination overlaps captured source data"
            );
        }
        let excluded = relative.map(|path| (path, destination_meta.dev(), destination_meta.ino()));
        self.validate_excluding(excluded)?;
        let dest = fingerprint(&destination)?;
        ensure!(
            dest.head == self.fingerprint.head,
            "Fork destination HEAD changed"
        );
        let dest_gitlinks = check_supported(&destination, &paths(&dest.status)?)?;
        let all_status = git(
            &destination,
            &[
                "-c",
                "core.filemode=true",
                "-c",
                "core.fsmonitor=false",
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored=traditional",
                "--no-renames",
                "--ignore-submodules=none",
            ],
        )?;
        ensure!(
            all_status.is_empty(),
            "Fork destination is not pristine; retained checkout: {}",
            destination.display()
        );
        let root = File::open(&destination)?;
        check_special_entries(
            &destination,
            &root,
            Path::new(""),
            &dest_gitlinks,
            true,
            None,
        )?;
        let meta = root.metadata()?;
        ensure!(
            meta.dev() == dest.root_dev && meta.ino() == dest.root_ino,
            "Fork destination was replaced"
        );
        let git_entry = destination_entry(&root, &CString::new(".git")?)?
            .context("Destination has no Git entry")?;
        let mut baseline = BTreeMap::<PathBuf, Option<DestinationEntry>>::new();
        for (path, _) in &self.entries {
            let mut prefix = PathBuf::new();
            for part in path.components() {
                prefix.push(part.as_os_str());
                if !baseline.contains_key(&prefix) {
                    baseline.insert(prefix.clone(), entry_at(&root, &prefix)?);
                }
            }
        }
        ensure!(
            fingerprint(&destination)? == dest,
            "Fork destination changed before transfer"
        );
        self.validate_excluding(excluded)?;
        let mut operations = 0usize;
        let mut apply = || -> Result<()> {
            let mut removed = BTreeSet::new();
            let mut order: Vec<_> = self.entries.iter().collect();
            order.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
            for (path, desired) in order {
                validate_destination_identity(&destination, &dest, &git_entry, &root)?;
                let expected = baseline.get(path).context("Missing destination baseline")?;
                let Some((parent, name)) = parent_at(&root, path)? else {
                    ensure!(expected.is_none(), "Destination parent changed: {:?}", path);
                    continue;
                };
                let actual = destination_entry(&parent, &name)?;
                let matches = match (expected, &actual) {
                    (None, None) => true,
                    (Some(a), Some(b)) if a.mode & libc::S_IFMT == libc::S_IFDIR => {
                        directory_identity_matches(a, b)
                    }
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                ensure!(matches, "Destination entry changed: {:?}", path);
                if let Some(actual) = actual {
                    let is_dir = actual.mode & libc::S_IFMT == libc::S_IFDIR;
                    if is_dir && matches!(desired, Entry::Missing) {
                        continue;
                    }
                    let flags = if is_dir { libc::AT_REMOVEDIR } else { 0 };
                    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) } < 0 {
                        return Err(io::Error::last_os_error())
                            .with_context(|| format!("Cannot remove destination {:?}", path));
                    }
                    operations += 1;
                    removed.insert(path.clone());
                }
            }
            let mut order: Vec<_> = self
                .entries
                .iter()
                .filter(|(_, entry)| !matches!(entry, Entry::Missing))
                .collect();
            order.sort_by_key(|(path, _)| path.components().count());
            let mut owned = BTreeMap::new();
            for (path, entry) in order {
                validate_destination_identity(&destination, &dest, &git_entry, &root)?;
                let (parent, leaf) = ensure_parent(&root, path, &baseline, &mut owned, &removed)?;
                match entry {
                    Entry::Symlink(target) => {
                        let target = CString::new(target.as_slice())?;
                        if unsafe {
                            libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), leaf.as_ptr())
                        } < 0
                        {
                            return Err(io::Error::last_os_error()).with_context(|| {
                                format!("Cannot create destination link {:?}", path)
                            });
                        }
                    }
                    Entry::Regular {
                        digest,
                        executable,
                        spool,
                    } => {
                        let temp = CString::new(format!(".amf-fork-{}", uuid::Uuid::new_v4()))?;
                        let fd = unsafe {
                            libc::openat(
                                parent.as_raw_fd(),
                                temp.as_ptr(),
                                libc::O_WRONLY
                                    | libc::O_CLOEXEC
                                    | libc::O_NOFOLLOW
                                    | libc::O_CREAT
                                    | libc::O_EXCL,
                                0o600,
                            )
                        };
                        if fd < 0 {
                            return Err(io::Error::last_os_error())
                                .with_context(|| format!("Cannot stage destination {:?}", path));
                        }
                        let mut output = unsafe { File::from_raw_fd(fd) };
                        let write = (|| -> Result<()> {
                            let mut input = File::open(spool)?;
                            let mut hasher = Sha256::new();
                            let mut buf = [0; 65536];
                            loop {
                                let n = input.read(&mut buf)?;
                                if n == 0 {
                                    break;
                                }
                                hasher.update(&buf[..n]);
                                output.write_all(&buf[..n])?;
                            }
                            let actual: [u8; 32] = hasher.finalize().into();
                            ensure!(&actual == digest, "Fork snapshot changed at {:?}", path);
                            if unsafe {
                                libc::fchmod(
                                    output.as_raw_fd(),
                                    if *executable { 0o755 } else { 0o644 },
                                )
                            } < 0
                            {
                                return Err(io::Error::last_os_error().into());
                            }
                            if unsafe {
                                libc::linkat(
                                    parent.as_raw_fd(),
                                    temp.as_ptr(),
                                    parent.as_raw_fd(),
                                    leaf.as_ptr(),
                                    0,
                                )
                            } < 0
                            {
                                return Err(io::Error::last_os_error()).with_context(|| {
                                    format!("Cannot publish destination {:?}", path)
                                });
                            }
                            Ok(())
                        })();
                        let cleanup =
                            unsafe { libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0) };
                        if cleanup < 0 {
                            return Err(io::Error::last_os_error()).with_context(|| {
                                format!("Cannot remove temporary file for {:?}", path)
                            });
                        }
                        write?;
                    }
                    Entry::Missing => unreachable!(),
                }
                operations += 1;
            }
            Ok(())
        };
        apply().with_context(|| {
            format!(
                "Fork transfer stopped after {operations} operations; checkout retained at {}",
                destination.display()
            )
        })
    }
}

#[cfg(test)]
mod tests {

    /// A name with bytes plain text cannot carry. APFS rejects non-UTF-8
    /// names outright (EILSEQ), so macOS gets a non-ASCII UTF-8 suffix.
    fn raw_test_name(prefix: &[u8]) -> PathBuf {
        let mut name = prefix.to_vec();
        if cfg!(target_os = "macos") {
            name.extend_from_slice("\u{e9}".as_bytes());
        } else {
            name.push(0xff);
        }
        PathBuf::from(OsString::from_vec(name))
    }

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
        let raw = raw_test_name(b"untracked\n");
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

    #[test]
    fn executable_changes_are_captured_and_assume_unchanged_is_refused() {
        let dir = fixture();
        let root = dir.path();
        run(root, &["config", "core.filemode", "false"]);
        fs::set_permissions(root.join("old name"), fs::Permissions::from_mode(0o755)).unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        assert!(
            snapshot
                .entries
                .iter()
                .any(|(path, entry)| path == Path::new("old name")
                    && matches!(
                        entry,
                        Entry::Regular {
                            executable: true,
                            ..
                        }
                    ))
        );
        run(root, &["update-index", "--assume-unchanged", "old name"]);
        assert!(
            Snapshot::capture(root)
                .unwrap_err()
                .to_string()
                .contains("Assume-unchanged")
        );
    }

    #[test]
    fn transfers_working_content_and_keeps_destination_index_at_head() {
        let dir = fixture();
        let root = dir.path();
        run(root, &["mv", "old name", "renamed"]);
        fs::write(root.join("renamed"), b"working, not staged").unwrap();
        fs::remove_file(root.join("deleted")).unwrap();
        fs::create_dir(root.join("deleted")).unwrap();
        fs::write(root.join("deleted/new"), b"nested").unwrap();
        symlink("../external", root.join("link")).unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        snapshot.transfer(&dest).unwrap();
        assert!(!dest.join("old name").exists());
        assert_eq!(
            fs::read(dest.join("renamed")).unwrap(),
            b"working, not staged"
        );
        assert_eq!(fs::read(dest.join("deleted/new")).unwrap(), b"nested");
        assert_eq!(
            fs::read_link(dest.join("link")).unwrap(),
            Path::new("../external")
        );
        run(&dest, &["diff", "--cached", "--quiet"]);
    }

    #[test]
    fn refuses_dirty_destination_and_stale_source_before_writes() {
        let dir = fixture();
        let root = dir.path();
        fs::write(root.join("old name"), b"source").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        fs::write(dest.join("extra"), b"user").unwrap();
        assert!(snapshot.transfer(&dest).is_err());
        assert_eq!(fs::read(dest.join("old name")).unwrap(), b"head");
        fs::remove_file(dest.join("extra")).unwrap();
        fs::write(root.join("old name"), b"changed").unwrap();
        assert!(snapshot.transfer(&dest).is_err());
        assert_eq!(fs::read(dest.join("old name")).unwrap(), b"head");
    }

    #[test]
    fn refuses_ignored_destination_files_instead_of_overwriting() {
        let dir = fixture();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "ignored\n").unwrap();
        run(root, &["add", ".gitignore"]);
        run(root, &["commit", "-qm", "ignore fixture"]);
        fs::write(root.join("old name"), b"source").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        fs::write(dest.join("ignored"), b"user data").unwrap();
        assert!(snapshot.transfer(&dest).is_err());
        assert_eq!(fs::read(dest.join("ignored")).unwrap(), b"user data");
        assert_eq!(fs::read(dest.join("old name")).unwrap(), b"head");
        fs::remove_file(dest.join("ignored")).unwrap();
        let fifo = CString::new(dest.join("ignored").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(snapshot.transfer(&dest).is_err());
        assert_eq!(fs::read(dest.join("old name")).unwrap(), b"head");
    }

    #[test]
    fn transfers_directory_to_file_and_non_utf8_executable() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir(root.join("directory")).unwrap();
        fs::write(root.join("directory/child"), b"old").unwrap();
        run(root, &["add", "directory"]);
        run(root, &["commit", "-qm", "directory fixture"]);
        fs::remove_file(root.join("directory/child")).unwrap();
        fs::remove_dir(root.join("directory")).unwrap();
        fs::write(root.join("directory"), b"replacement").unwrap();
        let raw = raw_test_name(b"quote'\n");
        fs::write(root.join(&raw), b"executable").unwrap();
        fs::set_permissions(root.join(&raw), fs::Permissions::from_mode(0o755)).unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        snapshot.transfer(&dest).unwrap();
        assert_eq!(fs::read(dest.join("directory")).unwrap(), b"replacement");
        assert_eq!(fs::read(dest.join(&raw)).unwrap(), b"executable");
        assert_ne!(fs::metadata(dest.join(&raw)).unwrap().mode() & 0o111, 0);
        run(&dest, &["diff", "--cached", "--quiet"]);
    }

    #[test]
    fn changed_symlink_parent_is_never_followed_into_another_directory() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/file"), b"tracked").unwrap();
        run(root, &["add", "sub"]);
        run(root, &["commit", "-qm", "subdirectory fixture"]);
        fs::write(root.join("sub/file"), b"updated").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        let outside = parent.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("file"), b"outside").unwrap();
        fs::remove_file(dest.join("sub/file")).unwrap();
        fs::remove_dir(dest.join("sub")).unwrap();
        symlink(&outside, dest.join("sub")).unwrap();
        assert!(snapshot.transfer(&dest).is_err());
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
    }

    #[test]
    fn settings_merge_preserves_tracked_fields_and_refuses_a_link() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".claude")).unwrap();
        let settings = dir.path().join(".claude/settings.local.json");
        fs::write(
            &settings,
            br#"{"nested":{"keep":1,"override":"old"},"other":true}"#,
        )
        .unwrap();
        merge_settings(dir.path(), br#"{"nested":{"override":"new"},"added":2}"#).unwrap();
        let actual: serde_json::Value =
            serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        assert_eq!(
            actual,
            serde_json::json!({"nested":{"keep":1,"override":"new"},"other":true,"added":2})
        );
        fs::remove_file(&settings).unwrap();
        let outside = dir.path().join("outside");
        fs::write(&outside, b"unchanged").unwrap();
        symlink(&outside, &settings).unwrap();
        assert!(merge_settings(dir.path(), b"{}").is_err());
        assert_eq!(fs::read(outside).unwrap(), b"unchanged");
    }

    #[test]
    fn owned_destination_inside_source_does_not_hide_other_untracked_worktree_data() {
        let dir = fixture();
        let root = dir.path();
        fs::create_dir_all(root.join(".worktrees/other")).unwrap();
        fs::write(root.join(".worktrees/other/user-file"), b"keep").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let dest = root.join(".worktrees/fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        snapshot.transfer(&dest).unwrap();
        assert_eq!(
            fs::read(dest.join(".worktrees/other/user-file")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn transfer_failure_reports_partial_progress_and_keeps_checkout() {
        let dir = fixture();
        let root = dir.path();
        fs::write(root.join("old name"), b"changed").unwrap();
        let snapshot = Snapshot::capture(root).unwrap();
        let (_, Entry::Regular { spool, .. }) = &snapshot.entries[0] else {
            panic!("expected captured file")
        };
        fs::write(spool, b"corrupted spool").unwrap();
        let parent = tempfile::tempdir().unwrap();
        let dest = parent.path().join("fork");
        run(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "fork",
                dest.to_str().unwrap(),
                "HEAD",
            ],
        );
        let error = format!("{:#}", snapshot.transfer(&dest).unwrap_err());
        assert!(error.contains("after 1 operations") && error.contains("snapshot changed"));
        assert!(dest.exists());
        assert!(!dest.join("old name").exists());
        assert!(!fs::read_dir(&dest).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".amf-fork-")
        }));
    }
}
