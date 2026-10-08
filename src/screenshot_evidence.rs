//! Shared local evidence contract. No App, persistence or rendering dependencies.
use anyhow::{Context, Result, ensure};
use base64ct::{Base64, Encoding};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub(crate) const IMAGE_BYTES: u64 = 20 * 1024 * 1024;
pub(crate) const METADATA_BYTES: u64 = 16 * 1024;
pub(crate) const MAX_ITEMS: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceOwner {
    pub version: u32,
    pub scope_id: String,
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub project_name: String,
    pub feature_name: String,
    pub session_label: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub created_at: DateTime<Utc>,
}

impl EvidenceOwner {
    pub fn directory(&self) -> PathBuf {
        self.workdir
            .join(".amf/screenshots")
            .join(&self.feature_id)
            .join(&self.session_id)
            .join(&self.scope_id)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "Unsupported evidence ownership version");
        for id in [
            &self.scope_id,
            &self.project_id,
            &self.feature_id,
            &self.session_id,
        ] {
            ensure!(valid_id(id), "Invalid evidence ownership ID");
        }
        ensure!(
            self.workdir.is_absolute(),
            "Evidence workdir must be absolute"
        );
        ensure!(
            [&self.project_name, &self.feature_name, &self.session_label]
                .iter()
                .all(|s| s.len() <= 2000)
                && self.workdir.as_os_str().len() <= 4096,
            "Evidence ownership metadata exceeds processing limit"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Completion {
    pub version: u32,
    pub scope_id: String,
    pub image_id: String,
    pub file: String,
    pub sha256: String,
    pub caption: String,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceItem {
    pub key: String,
    pub scope_id: String,
    pub image_id: String,
    pub file: String,
    pub sha256: String,
    pub caption: String,
    pub captured_at: DateTime<Utc>,
    pub owner: EvidenceOwner,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceIssue {
    pub scope_id: String,
    pub file: String,
    pub message: String,
}

#[derive(Debug, Default, Serialize)]
pub struct EvidenceListing {
    pub items: Vec<EvidenceItem>,
    pub issues: Vec<EvidenceIssue>,
    pub owners: Vec<EvidenceOwner>,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ImageData {
    pub data_url: String,
    pub width: u32,
    pub height: u32,
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub(crate) fn supported_file(name: &str) -> bool {
    let extension = Path::new(name)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
}

/// Validate every existing component. Never follow producer-controlled links.
pub(crate) fn safe_path(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "Evidence path must be absolute");
    let mut prefix = PathBuf::new();
    for component in path.components() {
        ensure!(
            !matches!(component, std::path::Component::ParentDir),
            "Unsafe evidence path"
        );
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "Evidence paths cannot contain symlinks"
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn open_directory(path: &Path, create: bool) -> Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    ensure!(path.is_absolute(), "Evidence path must be absolute");
    let mut directory = std::fs::File::open("/")?;
    for component in path.components() {
        match component {
            std::path::Component::RootDir | std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => {
                let name = std::ffi::CString::new(name.as_bytes())?;
                if create
                    && unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) } < 0
                {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::EEXIST) {
                        return Err(error.into());
                    }
                }
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                directory = unsafe { std::fs::File::from_raw_fd(fd) };
            }
            _ => anyhow::bail!("Unsafe evidence path"),
        }
    }
    Ok(directory)
}

pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let directory = open_directory(path.parent().context("Missing evidence parent")?, false)?;
    let name = std::ffi::CString::new(
        path.file_name()
            .context("Missing evidence filename")?
            .as_bytes(),
    )?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    ensure!(meta.is_file(), "Evidence must be a regular file");
    ensure!(
        meta.len() <= limit,
        "Evidence file exceeds processing limit"
    );
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Evidence file exceeds processing limit"
    );
    Ok(bytes)
}

fn directory_names(directory: &std::fs::File, limit: usize) -> Result<Vec<std::ffi::CString>> {
    use std::os::fd::AsRawFd;
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
    ensure!(fd >= 0, "Could not read evidence directory");
    let pointer = unsafe { libc::fdopendir(fd) };
    if pointer.is_null() {
        unsafe {
            libc::close(fd);
        }
        return Err(std::io::Error::last_os_error().into());
    }
    let dir = Dir(pointer);
    let mut names = Vec::new();
    while names.len() < limit {
        let entry = unsafe { libc::readdir(dir.0) };
        if entry.is_null() {
            break;
        }
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(crate) fn ensure_directory(owner: &EvidenceOwner) -> Result<()> {
    owner.validate()?;
    let dir = owner.directory();
    safe_path(&dir)?;
    let directory = open_directory(&dir, true)?;
    // Registering a destination must not dirty the repository before capture.
    // Keep an existing project's rules; never overwrite a shared ignore file.
    let namespace = owner.workdir.join(".amf/screenshots");
    let ignore = namespace.join(".gitignore");
    safe_path(&ignore)?;
    let namespace = open_directory(&namespace, false)?;
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        let fd = unsafe {
            libc::openat(
                namespace.as_raw_fd(),
                c".gitignore".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_EXCL | libc::O_CREAT,
                0o600,
            )
        };
        if fd >= 0 {
            use std::io::Write;
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            file.write_all(b"*\n")?;
        } else {
            let error = std::io::Error::last_os_error();
            ensure!(
                error.kind() == std::io::ErrorKind::AlreadyExists,
                "Could not register evidence ignore rules: {error}"
            );
        }
    }
    let path = dir.join("owner.json");
    if path.exists() {
        let existing: EvidenceOwner =
            serde_json::from_slice(&read_bounded(&path, METADATA_BYTES)?)?;
        ensure!(
            &existing == owner,
            "Conflicting evidence directory ownership"
        );
    } else {
        use std::os::fd::{AsRawFd, FromRawFd};
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                c"owner.json".as_ptr(),
                libc::O_WRONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_EXCL | libc::O_CREAT,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(owner)?)?;
    }
    Ok(())
}

fn validate_completion(
    owner: &EvidenceOwner,
    manifest_name: &str,
    completion: &Completion,
) -> Result<()> {
    ensure!(
        completion.version == 1,
        "Unsupported evidence metadata version"
    );
    ensure!(
        completion.scope_id == owner.scope_id && valid_id(&completion.image_id),
        "Evidence attribution mismatch"
    );
    ensure!(
        manifest_name == format!("{}.json", completion.image_id),
        "Evidence manifest identity mismatch"
    );
    ensure!(
        completion.file
            == format!(
                "{}.{}",
                completion.image_id,
                Path::new(&completion.file)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
            ),
        "Unsafe evidence filename"
    );
    ensure!(supported_file(&completion.file), "Unsupported image format");
    ensure!(
        completion.caption.chars().count() <= 2000,
        "Evidence caption exceeds limit"
    );
    ensure!(
        completion.sha256.len() == 64
            && completion
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "Invalid image content hash"
    );
    Ok(())
}

pub(crate) fn image_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    ensure!(
        matches!(
            reader.format(),
            Some(
                image::ImageFormat::Png
                    | image::ImageFormat::Jpeg
                    | image::ImageFormat::WebP
                    | image::ImageFormat::Gif
            )
        ),
        "Unsupported image format"
    );
    let (w, h) = reader.into_dimensions()?;
    ensure!(
        w > 0 && h > 0 && w <= 16384 && h <= 16384 && u64::from(w) * u64::from(h) <= 40_000_000,
        "Image dimensions exceed processing limit"
    );
    Ok((w, h))
}

pub(crate) fn scan(owners: Vec<EvidenceOwner>) -> EvidenceListing {
    let mut listing = EvidenceListing {
        owners: owners.clone(),
        ..Default::default()
    };
    let mut budget = 100 * 1024 * 1024u64;
    for owner in owners {
        let mut issue = |file: &str, message: String| {
            listing.issues.push(EvidenceIssue {
                scope_id: owner.scope_id.clone(),
                file: file.to_string(),
                message,
            })
        };
        let entries = (|| -> Result<Vec<String>> {
            owner.validate()?;
            safe_path(&owner.directory())?;
            let stored: EvidenceOwner = serde_json::from_slice(&read_bounded(
                &owner.directory().join("owner.json"),
                METADATA_BYTES,
            )?)?;
            ensure!(stored == owner, "Conflicting or missing evidence ownership");
            let entries = directory_names(&open_directory(&owner.directory(), false)?, 1001)?
                .into_iter()
                .map(|name| name.to_string_lossy().into_owned())
                .collect();
            Ok(entries)
        })();
        let mut entries = match entries {
            Ok(entries) => entries,
            Err(e) => {
                issue("owner.json", e.to_string());
                continue;
            }
        };
        if entries.len() > 1000 {
            listing.truncated = true;
            entries.truncate(1000);
        }
        entries.sort();
        for name in &entries {
            if budget == 0 {
                listing.truncated = true;
                break;
            }
            if name == "owner.json" || !name.ends_with(".json") {
                continue;
            }
            if listing.items.len() >= MAX_ITEMS {
                listing.truncated = true;
                break;
            }
            let result = (|| -> Result<EvidenceItem> {
                let manifest: Completion = serde_json::from_slice(&read_bounded(
                    &owner.directory().join(name),
                    METADATA_BYTES,
                )?)
                .context("Invalid evidence metadata")?;
                validate_completion(&owner, name, &manifest)?;
                let bytes = read_bounded(
                    &owner.directory().join(&manifest.file),
                    IMAGE_BYTES.min(budget),
                )?;
                ensure!(
                    bytes.len() as u64 <= budget,
                    "Evidence scan byte budget exceeded"
                );
                budget -= bytes.len() as u64;
                ensure!(
                    hash(&bytes) == manifest.sha256,
                    "Incomplete image: content hash does not match manifest"
                );
                image_dimensions(&bytes).context("Incomplete or invalid image")?;
                Ok(EvidenceItem {
                    key: format!("{}:{}", owner.scope_id, manifest.image_id),
                    scope_id: owner.scope_id.clone(),
                    image_id: manifest.image_id,
                    file: manifest.file,
                    sha256: manifest.sha256,
                    caption: manifest.caption,
                    captured_at: manifest.captured_at,
                    owner: owner.clone(),
                })
            })();
            match result {
                Ok(item) => listing.items.push(item),
                Err(e) => listing.issues.push(EvidenceIssue {
                    scope_id: owner.scope_id.clone(),
                    file: name.clone(),
                    message: format!("{e:#}"),
                }),
            }
        }
        for name in entries.iter().filter(|n| supported_file(n)) {
            let manifest = Path::new(name)
                .with_extension("json")
                .to_string_lossy()
                .into_owned();
            if !entries.contains(&manifest) {
                listing.issues.push(EvidenceIssue {
                    scope_id: owner.scope_id.clone(),
                    file: name.clone(),
                    message: "Incomplete evidence: completion manifest is missing".into(),
                });
            }
        }
    }
    listing.items.sort_by(|a, b| {
        b.captured_at
            .cmp(&a.captured_at)
            .then_with(|| a.key.cmp(&b.key))
    });
    listing
}

pub(crate) fn decode(bytes: &[u8], thumbnail: bool) -> Result<ImageData> {
    ensure!(
        bytes.len() as u64 <= IMAGE_BYTES,
        "Image exceeds processing limit"
    );
    static DECODE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DECODE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("Image decoder unavailable"))?;
    let (width, height) = image_dimensions(bytes)?;
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(160 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?;
    let image = if thumbnail {
        image.thumbnail(240, 160)
    } else {
        image
    };
    let mut buffer = Cursor::new(Vec::new());
    image.write_to(&mut buffer, image::ImageFormat::Png)?;
    let encoded = buffer.into_inner();
    ensure!(
        encoded.len() as u64 <= if thumbnail { 256 * 1024 } else { IMAGE_BYTES },
        "Encoded image exceeds transport limit"
    );
    Ok(ImageData {
        data_url: format!("data:image/png;base64,{}", Base64::encode_string(&encoded)),
        width,
        height,
    })
}

pub(crate) fn load_image(
    owner: &EvidenceOwner,
    image_id: &str,
    expected_hash: &str,
    thumbnail: bool,
) -> Result<ImageData> {
    ensure!(valid_id(image_id), "Invalid image identity");
    let stored: EvidenceOwner = serde_json::from_slice(&read_bounded(
        &owner.directory().join("owner.json"),
        METADATA_BYTES,
    )?)?;
    ensure!(&stored == owner, "Evidence ownership changed; refresh");
    let name = format!("{image_id}.json");
    let completion: Completion = serde_json::from_slice(&read_bounded(
        &owner.directory().join(&name),
        METADATA_BYTES,
    )?)?;
    validate_completion(owner, &name, &completion)?;
    ensure!(
        completion.sha256 == expected_hash,
        "Image changed; refresh evidence"
    );
    let bytes = read_bounded(&owner.directory().join(&completion.file), IMAGE_BYTES)?;
    ensure!(
        hash(&bytes) == expected_hash,
        "Image changed or is incomplete; refresh evidence"
    );
    decode(&bytes, thumbnail)
}

pub(crate) fn remove_owned_directory(owner: &EvidenceOwner) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    owner.validate()?;
    let path = owner.directory();
    safe_path(&path)?;
    if !path.exists() {
        return Ok(());
    }
    let parent = open_directory(path.parent().context("Missing scope parent")?, false)?;
    let name = std::ffi::CString::new(owner.scope_id.as_str())?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let directory = unsafe { std::fs::File::from_raw_fd(fd) };
    remove_directory_contents(&directory, 0, &mut 20_000, std::time::Instant::now())?;
    if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn remove_directory_contents(
    directory: &std::fs::File,
    depth: usize,
    remaining: &mut usize,
    started: std::time::Instant,
) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    ensure!(
        depth < 32,
        "Cleanup nesting limit reached; remove unexpected nested directories and retry"
    );
    loop {
        let names = directory_names(directory, 1000)?;
        if names.is_empty() {
            return Ok(());
        }
        for name in names {
            ensure!(
                *remaining > 0 && started.elapsed() < std::time::Duration::from_secs(30),
                "Cleanup processing limit reached; retry remaining files"
            );
            *remaining -= 1;
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
                return Err(std::io::Error::last_os_error().into());
            }
            let stat = unsafe { stat.assume_init() };
            let is_directory = stat.st_mode & libc::S_IFMT == libc::S_IFDIR;
            if is_directory {
                let fd = unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    )
                };
                if fd < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let child = unsafe { std::fs::File::from_raw_fd(fd) };
                remove_directory_contents(&child, depth + 1, remaining, started)?;
            }
            if unsafe {
                libc::unlinkat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    if is_directory { libc::AT_REMOVEDIR } else { 0 },
                )
            } < 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
}

pub(crate) fn guidance(owner: &EvidenceOwner) -> String {
    format!(
        "AMF screenshot evidence: Capture only when the user explicitly requests visual validation. This guidance does not authorize capture. Your producing AMF session is {} (feature {}). Use only {}. Do not modify owner.json or recreate this directory after cleanup. Write a completed PNG/JPEG/WebP/GIF as <image-id>.<extension>, then atomically publish <image-id>.json LAST with version:1, scope_id:{}, image_id, file, sha256 (lowercase SHA-256 of final image), caption and captured_at (RFC3339). IDs use ASCII letters/digits/hyphens/underscores. Close/rename the image before writing metadata. For replacement keep image_id and publish the new hash. Maximum image 20 MiB, 16384 pixels per axis and 40 million pixels. Check original owner.json still exists before publishing. Do not write another session's destination.",
        owner.session_id,
        owner.feature_id,
        owner.directory().display(),
        owner.scope_id
    )
}

/// Limit concurrent reads/downloads; decoder allocation is separately serialized.
pub(crate) struct ImagePermit(&'static (std::sync::Mutex<usize>, std::sync::Condvar));
impl Drop for ImagePermit {
    fn drop(&mut self) {
        let mut active = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        self.0.1.notify_one();
    }
}
pub(crate) fn image_worker() -> Result<ImagePermit> {
    static WORKERS: (std::sync::Mutex<usize>, std::sync::Condvar) =
        (std::sync::Mutex::new(0), std::sync::Condvar::new());
    let mut active = WORKERS
        .0
        .lock()
        .map_err(|_| anyhow::anyhow!("Image worker unavailable"))?;
    while *active >= 4 {
        active = WORKERS
            .1
            .wait(active)
            .map_err(|_| anyhow::anyhow!("Image worker unavailable"))?;
    }
    *active += 1;
    Ok(ImagePermit(&WORKERS))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(dir: &Path) -> EvidenceOwner {
        EvidenceOwner {
            version: 1,
            scope_id: "scope".into(),
            project_id: "project".into(),
            feature_id: "feature".into(),
            session_id: "session".into(),
            project_name: "Project".into(),
            feature_name: "Feature".into(),
            session_label: "Claude 1".into(),
            workdir: dir.to_path_buf(),
            is_worktree: false,
            created_at: Utc::now(),
        }
    }
    pub(crate) fn publish(owner: &EvidenceOwner, color: u8) {
        ensure_directory(owner).unwrap();
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            3,
            image::Rgb([color, 0, 0]),
        ))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
        let bytes = bytes.into_inner();
        fs::write(owner.directory().join("ready.png"), &bytes).unwrap();
        let metadata = Completion {
            version: 1,
            scope_id: owner.scope_id.clone(),
            image_id: "ready".into(),
            file: "ready.png".into(),
            sha256: hash(&bytes),
            caption: "Ready".into(),
            captured_at: Utc::now(),
        };
        fs::write(
            owner.directory().join("ready.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn producer_registration_keeps_git_clean_and_preserves_existing_ignore_rules() {
        let dir = tempfile::tempdir().unwrap();
        let mut git = std::process::Command::new("git");
        git.current_dir(dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        assert!(git.args(["init", "-q"]).status().unwrap().success());
        let owner = owner(dir.path());
        publish(&owner, 20);
        let status = std::process::Command::new("git")
            .current_dir(dir.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args([
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                ".amf/screenshots",
            ])
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(
            status.stdout.is_empty(),
            "Evidence dirtied the repo: {:?}",
            status.stdout
        );
        let ignore = dir.path().join(".amf/screenshots/.gitignore");
        std::fs::write(&ignore, "# Existing project choices\n*\n").unwrap();
        ensure_directory(&owner).unwrap();
        assert_eq!(
            std::fs::read_to_string(ignore).unwrap(),
            "# Existing project choices\n*\n"
        );
    }
    #[test]
    fn reconciliation_recovers_and_replacement_invalidates_old_thumbnail() {
        let temp = tempfile::tempdir().unwrap();
        let owner = owner(temp.path());
        publish(&owner, 20);
        let listing = scan(vec![owner.clone()]);
        assert_eq!(listing.items.len(), 1);
        let old = listing.items[0].sha256.clone();
        fs::write(owner.directory().join("ready.png"), b"unfinished").unwrap();
        assert!(scan(vec![owner.clone()]).items.is_empty());
        publish(&owner, 40);
        let new = scan(vec![owner.clone()]);
        assert_eq!(new.items.len(), 1);
        assert_ne!(old, new.items[0].sha256);
        assert!(load_image(&owner, "ready", &old, true).is_err());
        assert!(load_image(&owner, "ready", &new.items[0].sha256, true).is_ok());
        assert_eq!(scan(vec![owner]).items.len(), 1);
    }
    #[test]
    fn invalid_metadata_does_not_hide_neighbors_and_symlinks_are_refused() {
        let temp = tempfile::tempdir().unwrap();
        let owner = owner(temp.path());
        publish(&owner, 20);
        fs::write(owner.directory().join("broken.json"), b"{").unwrap();
        assert_eq!(scan(vec![owner.clone()]).items.len(), 1);
        fs::remove_file(owner.directory().join("ready.png")).unwrap();
        std::os::unix::fs::symlink(
            temp.path().join("outside"),
            owner.directory().join("ready.png"),
        )
        .unwrap();
        let listing = scan(vec![owner]);
        assert!(listing.items.is_empty());
        assert_eq!(listing.issues.len(), 2);
    }
    #[test]
    fn manifest_paths_and_claims_cannot_escape_scope() {
        let temp = tempfile::tempdir().unwrap();
        let owner = owner(temp.path());
        publish(&owner, 1);
        let path = owner.directory().join("ready.json");
        let mut manifest: Completion = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for file in [
            "../ready.png",
            "/ready.png",
            "ready/ready.png",
            "ready%2f.png",
        ] {
            manifest.file = file.into();
            assert!(validate_completion(&owner, "ready.json", &manifest).is_err());
        }
        manifest.file = "ready.png".into();
        manifest.scope_id = "another".into();
        assert!(validate_completion(&owner, "ready.json", &manifest).is_err());
    }
}
