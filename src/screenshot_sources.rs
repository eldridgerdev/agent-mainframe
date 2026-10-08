//! Source-specific remote evidence retrieval. Credentials never reach renderers.
use crate::screenshot_evidence::{IMAGE_BYTES, MAX_ITEMS, hash, supported_file, valid_id};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Debug, Clone)]
pub(crate) struct PrContext {
    pub workflow_id: String,
    pub feature_id: String,
    pub workdir: PathBuf,
    pub owner: String,
    pub repo: String,
    pub number: u32,
    pub head_sha: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunChoice {
    pub id: u64,
    pub attempt: u64,
    pub name: String,
    pub head_sha: String,
    pub status: String,
    pub conclusion: String,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct RemoteItem {
    pub key: String,
    pub caption: String,
    pub provenance: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct BrowserGallery {
    pub url: String,
    pub reason: String,
    pub provenance: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct SourceIssue {
    pub source: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct RemoteListing {
    pub request_id: String,
    pub items: Vec<RemoteItem>,
    pub galleries: Vec<BrowserGallery>,
    pub issues: Vec<SourceIssue>,
    pub runs: Vec<RunChoice>,
    pub selected_run: Option<u64>,
    pub run_page: u32,
    pub more_runs: bool,
}
#[derive(Debug, Clone)]
pub(crate) enum Resource {
    Url(String, bool),
    Repository {
        owner: String,
        repo: String,
        commit: String,
        path: String,
    },
    Gallery {
        manifest: String,
        image_id: String,
        url: String,
    },
    Bytes(std::sync::Arc<Vec<u8>>),
}
#[derive(Debug)]
pub(crate) struct Retrieved {
    pub listing: RemoteListing,
    pub resources: HashMap<String, Resource>,
}

impl Retrieved {
    fn issue(&mut self, issue: SourceIssue) {
        if self.listing.issues.len() < 200 {
            self.listing.issues.push(issue);
        } else if let Some(last) = self.listing.issues.last_mut() {
            last.message = "Additional source notices omitted; processing limit reached".into();
        }
    }
    fn gallery_notice(&mut self, gallery: BrowserGallery) {
        if self.listing.galleries.len() < 200 {
            self.listing.galleries.push(gallery);
        } else {
            self.issue(SourceIssue {
                source: "Galleries".into(),
                message: "Gallery discovery limit reached".into(),
            });
        }
    }
}

pub(crate) trait EvidenceGithub: Send + Sync {
    fn json(&self, workdir: &Path, endpoint: &str) -> Result<Value>;
    fn raw(&self, workdir: &Path, endpoint: &str, limit: u64) -> Result<Vec<u8>>;
    fn http(&self, workdir: &Path, url: &str, authenticated: bool, limit: u64) -> Result<Download>;
}
pub(crate) struct GithubEvidence;
pub(crate) struct Download {
    pub bytes: Vec<u8>,
    pub final_url: String,
    pub content_type: String,
}

struct GhChild(std::process::Child, bool);
impl Drop for GhChild {
    fn drop(&mut self) {
        if !self.1 {
            return;
        }
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

fn gh(workdir: &Path, args: &[&str], limit: u64) -> Result<Vec<u8>> {
    use std::os::unix::process::CommandExt;
    let child = Command::new("gh")
        .args(args)
        .current_dir(workdir)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .context("Could not launch GitHub CLI")?;
    let mut child = GhChild(child, true);
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let errors = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(16385).read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = Instant::now();
    loop {
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "GitHub request timed out; retry"
        );
        if reader.is_finished() && errors.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("GitHub output reader failed"))??;
    let errors = errors
        .join()
        .map_err(|_| anyhow::anyhow!("GitHub error reader failed"))??;
    ensure!(
        bytes.len() as u64 <= limit && errors.len() <= 16384,
        "GitHub response exceeds processing limit"
    );
    let status = loop {
        if let Some(s) = child.0.try_wait()? {
            break s;
        }
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "GitHub request timed out; retry"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    // try_wait has reaped it: prevent the guard from signaling a reusable PID.
    child.1 = false;
    ensure!(
        status.success(),
        "GitHub request failed: check gh authentication and source permissions, then retry"
    );
    Ok(bytes)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            !a.is_private()
                && !a.is_loopback()
                && !a.is_link_local()
                && !a.is_unspecified()
                && !a.is_multicast()
                && a.octets()[0] != 0
                && a.octets()[0] < 224
                && !(a.octets()[0] == 100 && (64..128).contains(&a.octets()[1]))
                && !(a.octets()[0] == 198 && (18..20).contains(&a.octets()[1]))
        }
        IpAddr::V6(a) => a
            .to_ipv4_mapped()
            .map(|a| public_ip(IpAddr::V4(a)))
            .unwrap_or_else(|| {
                (a.segments()[0] & 0xe000) == 0x2000
                    && !(a.segments()[0] == 0x2001 && a.segments()[1] == 0xdb8)
            }),
    }
}
#[derive(Debug)]
struct PublicResolver;
impl ureq::unversioned::resolver::Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> std::result::Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let addresses = ureq::unversioned::resolver::DefaultResolver::default()
            .resolve(uri, config, timeout)?;
        if addresses.iter().any(|a| !public_ip(a.ip())) {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}

pub(crate) fn validated_url(url: &str) -> Result<Url> {
    ensure!(url.len() <= 8192, "Source URL exceeds processing limit");
    let parsed = Url::parse(url)?;
    ensure!(
        matches!(parsed.scheme(), "http" | "https")
            && parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none(),
        "Unsupported or unsafe source URL"
    );
    if let Some(host) = parsed.host_str()
        && let Ok(ip) = host.trim_matches(['[', ']']).parse()
    {
        ensure!(
            public_ip(ip),
            "Remote images cannot access private network addresses"
        );
    }
    Ok(parsed)
}

fn redirect_destination(
    current: &Url,
    location: &str,
    credential_origin: &url::Origin,
) -> Result<(Url, bool)> {
    let next = validated_url(current.join(location)?.as_str())?;
    ensure!(
        current.scheme() != "https" || next.scheme() == "https",
        "Source redirect downgraded HTTPS"
    );
    let same_origin = next.origin() == *credential_origin;
    Ok((next, same_origin))
}

impl EvidenceGithub for GithubEvidence {
    fn json(&self, workdir: &Path, endpoint: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&gh(
            workdir,
            &["api", endpoint],
            1024 * 1024,
        )?)?)
    }
    fn raw(&self, workdir: &Path, endpoint: &str, limit: u64) -> Result<Vec<u8>> {
        gh(
            workdir,
            &[
                "api",
                endpoint,
                "-H",
                "Accept: application/vnd.github.raw+json",
            ],
            limit,
        )
    }
    fn http(&self, workdir: &Path, url: &str, authenticated: bool, limit: u64) -> Result<Download> {
        let mut current = validated_url(url)?;
        let mut token = if authenticated {
            ensure!(
                current.scheme() == "https"
                    && matches!(
                        current.host_str(),
                        Some(
                            "github.com"
                                | "api.github.com"
                                | "user-images.githubusercontent.com"
                                | "private-user-images.githubusercontent.com"
                        )
                    ),
                "Unsupported authenticated attachment origin"
            );
            Some(
                String::from_utf8(gh(
                    workdir,
                    &["auth", "token", "--hostname", "github.com"],
                    4096,
                )?)?
                .trim()
                .to_string(),
            )
        } else {
            None
        };
        let origin = current.origin();
        let start = Instant::now();
        for _ in 0..=5 {
            let remaining = Duration::from_secs(30)
                .checked_sub(start.elapsed())
                .context("Remote request timed out")?;
            let config = ureq::config::Config::builder()
                .tls_config(
                    crate::http_client::https_agent()
                        .config()
                        .tls_config()
                        .clone(),
                )
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(remaining))
                .proxy(None)
                .build();
            let agent = ureq::Agent::with_parts(
                config,
                ureq::unversioned::transport::DefaultConnector::default(),
                PublicResolver,
            );
            let mut request = agent.get(current.as_str());
            if let Some(token) = &token {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            let mut response = request.call().map_err(|_| {
                anyhow::anyhow!("Remote retrieval failed; check connectivity and authentication")
            })?;
            let status = response.status().as_u16();
            if (300..400).contains(&status) {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .context("Source redirect is missing a destination")?;
                let (next, retain_credentials) = redirect_destination(&current, location, &origin)?;
                if !retain_credentials {
                    token = None;
                }
                current = next;
                continue;
            }
            ensure!(
                (200..300).contains(&status),
                "Source returned HTTP {status}; check authentication or expired evidence and retry"
            );
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(limit + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| anyhow::anyhow!("Source body read failed; retry"))?;
            ensure!(
                bytes.len() as u64 <= limit,
                "Source exceeds processing limit"
            );
            return Ok(Download {
                bytes,
                final_url: current.to_string(),
                content_type,
            });
        }
        bail!("Source exceeded redirect limit")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MarkupLink {
    pub url: String,
    pub image: bool,
    pub label: String,
}
pub(crate) fn markup_links(body: &str) -> Vec<MarkupLink> {
    use pulldown_cmark::{Event, Parser, Tag};
    let mut links = Vec::new();
    let mut code = 0;
    let mut active_link: Option<usize> = None;
    let mut active_image: Option<usize> = None;
    for event in Parser::new(body) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => code += 1,
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => code -= 1,
            Event::Start(Tag::Image { dest_url, .. }) if code == 0 => {
                active_image = Some(links.len());
                links.push(MarkupLink {
                    url: dest_url.into_string(),
                    image: true,
                    label: String::new(),
                });
            }
            Event::End(pulldown_cmark::TagEnd::Image) => active_image = None,
            Event::Start(Tag::Link { dest_url, .. }) if code == 0 => {
                active_link = Some(links.len());
                links.push(MarkupLink {
                    url: dest_url.into_string(),
                    image: false,
                    label: String::new(),
                });
            }
            Event::End(pulldown_cmark::TagEnd::Link) => active_link = None,
            Event::Text(text) if active_image.is_some() || active_link.is_some() => {
                if let Some(index) = active_image.or(active_link) {
                    links[index].label.push_str(&text);
                }
            }
            Event::Html(html) | Event::InlineHtml(html) if code == 0 => {
                let fragment = scraper::Html::parse_fragment(&html);
                let selector = scraper::Selector::parse("img[src]").unwrap();
                for image in fragment.select(&selector) {
                    links.push(MarkupLink {
                        url: image.value().attr("src").unwrap().into(),
                        image: true,
                        label: image.value().attr("alt").unwrap_or("").into(),
                    });
                }
            }
            _ => {}
        }
    }
    links
}

fn repo_slug(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "Invalid GitHub repository identity"
    );
    Ok(())
}
fn encode_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
fn decode_path(value: &str) -> Result<String> {
    let decoded = percent_decode(value)?;
    ensure!(
        !decoded.contains(['\\', '\0']) && !decoded.starts_with('/'),
        "Unsafe repository image path"
    );
    let parts: Vec<_> = decoded.split('/').filter(|s| *s != ".").collect();
    ensure!(
        !parts.is_empty() && parts.iter().all(|s| !s.is_empty() && *s != ".."),
        "Repository image path escapes its root"
    );
    Ok(parts.join("/"))
}
fn percent_decode(value: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut input = value.bytes();
    while let Some(b) = input.next() {
        if b == b'%' {
            let a = input.next().context("Invalid escaped path")?;
            let c = input.next().context("Invalid escaped path")?;
            let pair = std::str::from_utf8(&[a, c]).map(str::to_owned)?;
            bytes.push(u8::from_str_radix(&pair, 16)?);
        } else {
            bytes.push(b);
        }
    }
    Ok(String::from_utf8(bytes)?)
}
fn contents_endpoint(owner: &str, repo: &str, commit: &str, path: &str) -> String {
    format!(
        "repos/{owner}/{repo}/contents/{}?ref={}",
        path.split('/')
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/"),
        encode_segment(commit)
    )
}

fn repository_resource(
    context: &PrContext,
    head_owner: &str,
    head_repo: &str,
    link: &str,
    github: &dyn EvidenceGithub,
) -> Result<Option<Resource>> {
    if !link.contains("://") && !link.starts_with("//") {
        if link.starts_with('#') {
            return Ok(None);
        }
        let path = decode_path(
            link.split(['?', '#'])
                .next()
                .unwrap_or("")
                .trim_start_matches('/'),
        )?;
        if !supported_file(&path) {
            return Ok(None);
        }
        ensure!(
            !head_owner.is_empty() && !head_repo.is_empty(),
            "PR head repository is unavailable; relative images cannot be resolved"
        );
        repo_slug(head_owner)?;
        repo_slug(head_repo)?;
        return Ok(Some(Resource::Repository {
            owner: head_owner.into(),
            repo: head_repo.into(),
            commit: context.head_sha.clone(),
            path,
        }));
    }
    let parsed = validated_url(link)?;
    let segments: Vec<_> = parsed.path().trim_start_matches('/').split('/').collect();
    let start = if parsed.host_str() == Some("github.com")
        && segments.len() >= 5
        && matches!(segments[2], "blob" | "raw")
    {
        3
    } else if parsed.host_str() == Some("raw.githubusercontent.com") && segments.len() >= 4 {
        2
    } else {
        return Ok(None);
    };
    let owner = percent_decode(segments[0])?;
    let repo = percent_decode(segments[1])?;
    repo_slug(&owner)?;
    repo_slug(&repo)?;
    for split in (start + 1..segments.len()).rev().take(32) {
        let revision = percent_decode(&segments[start..split].join("/"))?;
        if let Ok(commit) = github.json(
            &context.workdir,
            &format!("repos/{owner}/{repo}/commits/{}", encode_segment(&revision)),
        ) && let Some(sha) = commit["sha"].as_str()
        {
            let path = decode_path(&segments[split..].join("/"))?;
            return Ok(Some(Resource::Repository {
                owner,
                repo,
                commit: sha.into(),
                path,
            }));
        }
    }
    bail!("Repository image revision could not be resolved")
}

fn add_resource(result: &mut Retrieved, resource: Resource, caption: String, origin: String) {
    let identity = match &resource {
        Resource::Url(url, _) => url.clone(),
        Resource::Repository {
            owner,
            repo,
            commit,
            path,
        } => format!("{owner}/{repo}@{commit}:{path}"),
        Resource::Gallery {
            manifest, image_id, ..
        } => format!("{manifest}#{image_id}"),
        Resource::Bytes(bytes) => hash(bytes),
    };
    let key = hash(identity.as_bytes());
    if let Some(existing) = result.listing.items.iter_mut().find(|i| i.key == key) {
        if !existing.provenance.contains(&origin) {
            existing.provenance.push(origin);
        }
        return;
    }
    if result.listing.items.len() >= MAX_ITEMS {
        result.issue(SourceIssue {
            source: origin,
            message: "Evidence item limit reached".into(),
        });
        return;
    }
    result.listing.items.push(RemoteItem {
        key: key.clone(),
        caption,
        provenance: vec![origin],
    });
    result.resources.insert(key, resource);
}

#[derive(Deserialize)]
struct GalleryManifest {
    version: u32,
    title: Option<String>,
    images: Vec<GalleryImage>,
}
#[derive(Deserialize)]
struct GalleryImage {
    id: String,
    url: String,
    caption: Option<String>,
}

fn gallery(
    result: &mut Retrieved,
    context: &PrContext,
    url: &str,
    origin: &str,
    github: &dyn EvidenceGithub,
) -> Result<()> {
    let download = github.http(&context.workdir, url, false, 1024 * 1024)?;
    ensure!(
        download
            .content_type
            .starts_with("application/vnd.amf.screenshots+json")
            || Url::parse(url)?.path().ends_with("/amf-screenshots.json"),
        "Unsupported public gallery: open in browser"
    );
    let manifest: GalleryManifest = serde_json::from_slice(&download.bytes)?;
    ensure!(
        manifest.version == 1 && manifest.images.len() <= MAX_ITEMS,
        "Unsupported or oversized gallery manifest"
    );
    let base = validated_url(&download.final_url)?;
    let mut ids = HashSet::new();
    for image in &manifest.images {
        ensure!(
            valid_id(&image.id) && ids.insert(image.id.clone()),
            "Invalid or duplicate gallery image ID"
        );
        validated_url(base.join(&image.url)?.as_str())?;
    }
    for image in manifest.images {
        let resolved = base.join(&image.url)?;
        let caption = image
            .caption
            .unwrap_or_else(|| manifest.title.clone().unwrap_or_else(|| image.id.clone()));
        let mut label = format!("{origin} · Gallery {url} · image {}", image.id);
        if caption.chars().count() > 2000 {
            bail!("Gallery caption exceeds limit");
        }
        // The manifest and image ID are retained in provenance; signed image URLs are never displayed.
        label.push_str(" (public manifest)");
        add_resource(
            result,
            Resource::Gallery {
                manifest: url.into(),
                image_id: image.id,
                url: resolved.into(),
            },
            caption,
            label,
        );
    }
    Ok(())
}

fn extract_body(
    result: &mut Retrieved,
    context: &PrContext,
    head_owner: &str,
    head_repo: &str,
    body: &str,
    origin: &str,
    github: &dyn EvidenceGithub,
) {
    for link in markup_links(body).into_iter().take(1000) {
        let mut url = link.url.clone();
        if let Ok(mut parsed) = Url::parse(&url) {
            parsed.set_fragment(None);
            url = parsed.into();
        }
        if !link.image
            && !supported_file(url.split(['?', '#']).next().unwrap_or(&url))
            && (url.starts_with("https://github.com/") && url.contains("/blob/")
                || !url.starts_with("http://") && !url.starts_with("https://"))
        {
            continue;
        }
        match repository_resource(context, head_owner, head_repo, &url, github) {
            Ok(Some(resource)) => {
                let label = if let Resource::Repository {
                    owner,
                    repo,
                    commit,
                    path,
                } = &resource
                {
                    format!("{origin} · {owner}/{repo} @ {commit} · {path}")
                } else {
                    origin.into()
                };
                add_resource(
                    result,
                    resource,
                    if link.label.is_empty() {
                        "Repository image".into()
                    } else {
                        link.label.chars().take(2000).collect()
                    },
                    label,
                );
                continue;
            }
            Err(e) => {
                result.issue(SourceIssue {
                    source: origin.into(),
                    message: e.to_string(),
                });
                continue;
            }
            _ => {}
        }
        if !url.starts_with("https://") && !url.starts_with("http://") {
            continue;
        }
        let parsed = match validated_url(&url) {
            Ok(p) => p,
            Err(e) => {
                result.issue(SourceIssue {
                    source: origin.into(),
                    message: e.to_string(),
                });
                continue;
            }
        };
        let attachment = parsed.host_str() == Some("github.com")
            && parsed.path().starts_with("/user-attachments/")
            || matches!(
                parsed.host_str(),
                Some(
                    "user-images.githubusercontent.com"
                        | "private-user-images.githubusercontent.com"
                )
            );
        if link.image || supported_file(parsed.path()) || attachment {
            add_resource(
                result,
                Resource::Url(url, attachment),
                if !link.label.is_empty() {
                    link.label.chars().take(2000).collect()
                } else if attachment {
                    "GitHub attachment".into()
                } else {
                    "Linked image".into()
                },
                format!(
                    "{origin} · {} · discovered at {}",
                    if attachment {
                        "GitHub attachment"
                    } else {
                        "External image"
                    },
                    context.head_sha
                ),
            );
        } else {
            let label = link.label.to_lowercase();
            if !parsed.path().ends_with(".json")
                && !["gallery", "screenshot", "visual proof", "visual validation"]
                    .iter()
                    .any(|term| label.contains(term))
            {
                continue;
            }
            // Explicitly linked manifests and vendor responses only; unsupported pages get a browser action.
            if let Err(error) = gallery(result, context, &url, origin, github) {
                result.gallery_notice(BrowserGallery {
                    url,
                    reason: format!("{error}"),
                    provenance: origin.into(),
                });
            }
        }
    }
}

pub(crate) struct ArchiveImages {
    images: Vec<(String, Vec<u8>)>,
    issues: Vec<String>,
}

pub(crate) fn archive_images(bytes: &[u8]) -> Result<ArchiveImages> {
    ensure!(
        bytes.len() <= 100 * 1024 * 1024,
        "Artifact archive exceeds compressed limit"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(
        archive.len() <= 1000,
        "Artifact archive exceeds entry limit"
    );
    let mut names = HashSet::new();
    let mut size = 0u64;
    let mut images = Vec::new();
    let mut issues = Vec::new();
    let mut pixels = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        let components: Vec<_> = name.trim_end_matches('/').split('/').collect();
        ensure!(
            !name.starts_with('/')
                && !name.contains(['\\', '\0', ':'])
                && components
                    .iter()
                    .all(|p| !p.is_empty() && *p != ".." && *p != ".")
                && names.insert(name.trim_end_matches('/').to_string())
                && file.enclosed_name().is_some(),
            "Unsafe artifact archive path"
        );
        ensure!(
            !file.encrypted() && file.unix_mode().is_none_or(|m| m & 0o170000 != 0o120000),
            "Unsupported artifact archive member"
        );
        ensure!(
            file.is_dir()
                || !matches!(
                    name.rsplit('.')
                        .next()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar"
                ),
            "Nested artifact archives are unsupported"
        );
        size = size
            .checked_add(file.size())
            .context("Archive size overflow")?;
        ensure!(
            size <= 200 * 1024 * 1024,
            "Artifact archive exceeds extracted limit"
        );
        if file.is_dir() || !supported_file(&name) {
            continue;
        }
        if file.size() > IMAGE_BYTES {
            issues.push(format!("{name}: image exceeds processing limit"));
            continue;
        }
        let mut image = Vec::new();
        if let Err(error) = file.by_ref().take(IMAGE_BYTES + 1).read_to_end(&mut image) {
            issues.push(format!(
                "{name}: archive member could not be read ({error})"
            ));
            continue;
        }
        ensure!(
            image.len() as u64 <= IMAGE_BYTES,
            "Artifact image exceeds limit"
        );
        match crate::screenshot_evidence::image_dimensions(&image).and_then(|(w, h)| {
            ensure!(
                pixels + u64::from(w) * u64::from(h) <= 200_000_000,
                "Archive decoded pixel budget reached"
            );
            pixels += u64::from(w) * u64::from(h);
            crate::screenshot_evidence::decode(&image, true).map(|_| ())
        }) {
            Ok(()) => images.push((name, image)),
            Err(error) => issues.push(format!("{name}: {error}")),
        }
    }
    Ok(ArchiveImages { images, issues })
}

fn paged_bodies(
    result: &mut Retrieved,
    context: &PrContext,
    head_owner: &str,
    head_repo: &str,
    endpoint: &str,
    label: &str,
    github: &dyn EvidenceGithub,
) {
    for page in 1..=100 {
        let response = match github.json(
            &context.workdir,
            &format!("{endpoint}?per_page=100&page={page}"),
        ) {
            Ok(v) => v,
            Err(e) => {
                result.issue(SourceIssue {
                    source: label.into(),
                    message: e.to_string(),
                });
                break;
            }
        };
        let Some(comments) = response.as_array() else {
            result.issue(SourceIssue {
                source: label.into(),
                message: "Invalid GitHub comment page".into(),
            });
            break;
        };
        for comment in comments {
            if let Some(body) = comment["body"].as_str() {
                extract_body(
                    result,
                    context,
                    head_owner,
                    head_repo,
                    body,
                    &format!("{label} #{}", comment["id"]),
                    github,
                );
            }
        }
        if comments.len() < 100 {
            break;
        }
        if page == 100 {
            result.issue(SourceIssue {
                source: label.into(),
                message: "Comment pagination limit reached".into(),
            });
        }
    }
}

fn run_choice(value: &Value) -> RunChoice {
    RunChoice {
        id: value["id"].as_u64().unwrap_or(0),
        attempt: value["run_attempt"].as_u64().unwrap_or(1),
        name: value["name"].as_str().unwrap_or("Workflow").into(),
        head_sha: value["head_sha"].as_str().unwrap_or("").into(),
        status: value["status"].as_str().unwrap_or("").into(),
        conclusion: value["conclusion"].as_str().unwrap_or("").into(),
        created_at: value["run_started_at"]
            .as_str()
            .or_else(|| value["created_at"].as_str())
            .unwrap_or("")
            .into(),
    }
}

fn artifacts(
    result: &mut Retrieved,
    context: &PrContext,
    run: &RunChoice,
    github: &dyn EvidenceGithub,
) -> Result<bool> {
    let mut found = false;
    let mut cache_bytes = 0usize;
    for page in 1..=100 {
        let response = github.json(
            &context.workdir,
            &format!(
                "repos/{}/{}/actions/runs/{}/artifacts?per_page=100&page={page}",
                context.owner, context.repo, run.id
            ),
        )?;
        let artifacts = response["artifacts"]
            .as_array()
            .context("Invalid artifact list")?;
        for artifact in artifacts {
            let id = artifact["id"]
                .as_u64()
                .context("Missing artifact identity")?;
            if artifact["expired"].as_bool().unwrap_or(true) {
                result.issue(SourceIssue {
                    source: format!("Run {} artifact {id}", run.id),
                    message: "Artifact expired".into(),
                });
                continue;
            }
            let url = format!(
                "https://api.github.com/repos/{}/{}/actions/artifacts/{id}/zip",
                context.owner, context.repo
            );
            let outcome = (|| -> Result<()> {
                let download = github.http(&context.workdir, &url, true, 100 * 1024 * 1024)?;
                let archive = archive_images(&download.bytes)?;
                for message in archive.issues {
                    result.issue(SourceIssue {
                        source: format!("Artifact {id}"),
                        message,
                    });
                }
                for (member, bytes) in archive.images {
                    cache_bytes += bytes.len();
                    ensure!(
                        cache_bytes <= 100 * 1024 * 1024,
                        "Artifact image cache budget reached"
                    );
                    found = true;
                    let key = hash(
                        format!(
                            "{}/{}/run/{}/artifact/{id}/{member}",
                            context.owner, context.repo, run.id
                        )
                        .as_bytes(),
                    );
                    if result.listing.items.len() >= MAX_ITEMS {
                        bail!("Evidence item limit reached");
                    }
                    result.listing.items.push(RemoteItem {
                        key: key.clone(),
                        caption: member.clone(),
                        provenance: vec![format!(
                            "Actions · {} · run {} (current attempt {}; artifact attempt not provided by GitHub) · artifact {} ({}) · {} · commit {}",
                            run.name,
                            run.id,
                            run.attempt,
                            id,
                            artifact["name"].as_str().unwrap_or(""),
                            member,
                            run.head_sha
                        )],
                    });
                    result
                        .resources
                        .insert(key, Resource::Bytes(std::sync::Arc::new(bytes)));
                }
                Ok(())
            })();
            if let Err(error) = outcome {
                result.issue(SourceIssue {
                    source: format!("Run {} artifact {id}", run.id),
                    message: error.to_string(),
                });
            }
        }
        if artifacts.len() < 100 {
            break;
        }
    }
    Ok(found)
}

pub(crate) fn retrieve(
    context: &PrContext,
    request_id: String,
    selected_run: Option<u64>,
    run_page: u32,
    github: &dyn EvidenceGithub,
) -> Result<Retrieved> {
    repo_slug(&context.owner)?;
    repo_slug(&context.repo)?;
    ensure!(run_page > 0 && run_page <= 1000, "Invalid run page");
    let mut result = Retrieved {
        listing: RemoteListing {
            request_id,
            items: vec![],
            galleries: vec![],
            issues: vec![],
            runs: vec![],
            selected_run: None,
            run_page,
            more_runs: false,
        },
        resources: HashMap::new(),
    };
    let endpoint = format!("repos/{}/{}", context.owner, context.repo);
    let pr = github.json(
        &context.workdir,
        &format!("{endpoint}/pulls/{}", context.number),
    )?;
    ensure!(
        pr["head"]["sha"].as_str() == Some(context.head_sha.as_str()),
        "PR head changed; refresh PR Triage first"
    );
    let head_owner = pr["head"]["repo"]["owner"]["login"].as_str().unwrap_or("");
    let head_repo = pr["head"]["repo"]["name"].as_str().unwrap_or("");
    extract_body(
        &mut result,
        context,
        head_owner,
        head_repo,
        pr["body"].as_str().unwrap_or(""),
        &format!("PR #{} description", context.number),
        github,
    );
    for (suffix, label) in [
        (
            format!("issues/{}/comments", context.number),
            "Conversation comment",
        ),
        (
            format!("pulls/{}/comments", context.number),
            "Review comment/reply",
        ),
        (
            format!("pulls/{}/reviews", context.number),
            "Review summary",
        ),
    ] {
        paged_bodies(
            &mut result,
            context,
            head_owner,
            head_repo,
            &format!("{endpoint}/{suffix}"),
            label,
            github,
        );
    }
    let runs = github.json(
        &context.workdir,
        &format!("{endpoint}/actions/runs?per_page=100&page={run_page}"),
    );
    if let Ok(runs) = runs {
        if let Some(values) = runs["workflow_runs"].as_array() {
            result.listing.more_runs = values.len() == 100;
            result.listing.runs = values
                .iter()
                .filter(|v| {
                    v["head_sha"].as_str() == Some(&context.head_sha)
                        || v["pull_requests"].as_array().is_some_and(|prs| {
                            prs.iter()
                                .any(|p| p["number"].as_u64() == Some(u64::from(context.number)))
                        })
                })
                .map(run_choice)
                .collect();
        }
    } else {
        result.issue(SourceIssue {
            source: "Actions".into(),
            message: "Could not list Actions runs; check Actions read permissions and retry".into(),
        });
    }
    if let Some(id) = selected_run {
        let run = result
            .listing
            .runs
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .context("Selected run is not on this PR's current run page")?;
        if !artifacts(&mut result, context, &run, github)? {
            result.issue(SourceIssue {
                source: format!("Run {id}"),
                message: "No supported unexpired screenshot images".into(),
            });
        }
        result.listing.selected_run = Some(id);
    } else {
        // Query the current head independently of the older-run page; all workflows/conclusions are eligible.
        let mut eligible = Vec::new();
        for page in 1..=100 {
            match github.json(
                &context.workdir,
                &format!(
                    "{endpoint}/actions/runs?head_sha={}&status=completed&per_page=100&page={page}",
                    encode_segment(&context.head_sha)
                ),
            ) {
                Ok(response) => {
                    let runs = response["workflow_runs"]
                        .as_array()
                        .context("Invalid Actions run list")?;
                    eligible.extend(
                        runs.iter()
                            .filter(|v| {
                                v["head_sha"].as_str() == Some(&context.head_sha)
                                    && v["status"].as_str() == Some("completed")
                            })
                            .map(run_choice),
                    );
                    if runs.len() < 100 {
                        break;
                    }
                    if page == 100 {
                        result.issue(SourceIssue {
                            source: "Actions".into(),
                            message: "Run pagination limit reached".into(),
                        });
                    }
                }
                Err(error) => {
                    result.issue(SourceIssue {
                        source: "Actions".into(),
                        message: error.to_string(),
                    });
                    break;
                }
            }
        }
        eligible.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        for run in eligible {
            match artifacts(&mut result, context, &run, github) {
                Ok(true) => {
                    result.listing.selected_run = Some(run.id);
                    if !result.listing.runs.iter().any(|r| r.id == run.id) {
                        result.listing.runs.insert(0, run);
                    }
                    break;
                }
                Ok(false) => {}
                Err(error) => result.issue(SourceIssue {
                    source: format!("Actions run {}", run.id),
                    message: error.to_string(),
                }),
            }
        }
    }
    Ok(result)
}

pub(crate) fn resource_image(
    context: &PrContext,
    resource: &Resource,
    thumbnail: bool,
    github: &dyn EvidenceGithub,
) -> Result<crate::screenshot_evidence::ImageData> {
    let bytes = match resource {
        Resource::Gallery { url, .. } => {
            github
                .http(&context.workdir, url, false, IMAGE_BYTES)?
                .bytes
        }
        Resource::Url(url, auth) => {
            github
                .http(&context.workdir, url, *auth, IMAGE_BYTES)?
                .bytes
        }
        Resource::Repository {
            owner,
            repo,
            commit,
            path,
        } => github.raw(
            &context.workdir,
            &contents_endpoint(owner, repo, commit, path),
            IMAGE_BYTES,
        )?,
        Resource::Bytes(bytes) => {
            return crate::screenshot_evidence::decode(bytes.as_slice(), thumbnail);
        }
    };
    crate::screenshot_evidence::decode(&bytes, thumbnail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    struct Fixture {
        responses: HashMap<String, Value>,
        downloads: HashMap<String, Vec<u8>>,
        calls: Mutex<Vec<String>>,
    }
    impl EvidenceGithub for Fixture {
        fn json(&self, _: &Path, endpoint: &str) -> Result<Value> {
            self.calls.lock().unwrap().push(endpoint.into());
            if let Some(value) = self.responses.get(endpoint) {
                return Ok(value.clone());
            }
            if endpoint.contains("/actions/runs?") {
                return Ok(json!({"workflow_runs":[]}));
            }
            if endpoint.contains("/artifacts?") {
                return Ok(json!({"artifacts":[]}));
            }
            if endpoint.contains("comments?") || endpoint.contains("reviews?") {
                return Ok(json!([]));
            }
            bail!("fixture missing {endpoint}")
        }
        fn raw(&self, _: &Path, endpoint: &str, _: u64) -> Result<Vec<u8>> {
            self.calls.lock().unwrap().push(endpoint.into());
            Ok(png())
        }
        fn http(&self, _: &Path, url: &str, authenticated: bool, _: u64) -> Result<Download> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("http {authenticated} {url}"));
            let bytes = self
                .downloads
                .get(url)
                .context("fixture denied download")?
                .clone();
            Ok(Download {
                bytes,
                final_url: url.into(),
                content_type: if url.ends_with("amf-screenshots.json") {
                    "application/json".into()
                } else {
                    "image/png".into()
                },
            })
        }
    }
    fn context() -> PrContext {
        PrContext {
            workflow_id: "workflow".into(),
            feature_id: "feature".into(),
            workdir: PathBuf::from("/fixture"),
            owner: "base".into(),
            repo: "repo".into(),
            number: 1,
            head_sha: "current".into(),
        }
    }
    fn fixture(body: &str) -> Fixture {
        Fixture {
            responses: HashMap::from([(
                "repos/base/repo/pulls/1".into(),
                json!({"body":body,"head":{"sha":"current","repo":{"owner":{"login":"fork"},"name":"source"}}}),
            )]),
            downloads: HashMap::new(),
            calls: Mutex::new(vec![]),
        }
    }
    fn png() -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }
    fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }
    #[test]
    fn markup_skips_code_comments_and_scripts_but_supports_reference_and_html_images() {
        let body = "![Inline](./one.png)\n\n![Reference][image]\n\n[image]: ./two.png\n\n<img alt=\"test\" src=\"https://example.com/three.png?x=1&amp;y=2\">\n\n`![not](code.png)`\n\n```\n![not](block.png)\n```\n\n<!-- <img src=\"comment.png\"> -->\n\n<script><img src=\"script.png\"></script>\n";
        let links = markup_links(body);
        assert_eq!(links.len(), 3);
        assert_eq!(links[0].label, "Inline");
        assert_eq!(links[2].label, "test");
        assert_eq!(links[2].url, "https://example.com/three.png?x=1&y=2");
    }
    #[test]
    fn relative_images_are_pinned_to_fork_head_and_duplicates_keep_origins() {
        let mut f = fixture("![ready](./docs/ready.png)");
        f.responses.insert(
            "repos/base/repo/issues/1/comments?per_page=100&page=1".into(),
            json!([{"id":10,"body":"![again](docs/ready.png)"}]),
        );
        let result = retrieve(&context(), "request".into(), None, 1, &f).unwrap();
        assert_eq!(result.listing.items.len(), 1);
        assert_eq!(result.listing.items[0].provenance.len(), 2);
        let resource = result.resources.values().next().unwrap();
        match resource {
            Resource::Repository {
                owner,
                repo,
                commit,
                path,
            } => assert_eq!(
                (
                    owner.as_str(),
                    repo.as_str(),
                    commit.as_str(),
                    path.as_str()
                ),
                ("fork", "source", "current", "docs/ready.png")
            ),
            _ => panic!("wrong source"),
        };
        resource_image(&context(), resource, true, &f).unwrap();
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "repos/fork/source/contents/docs/ready.png?ref=current")
        );
        for path in [
            "../outside.png",
            "%2e%2e/outside.png",
            "foo/%2e%2e/outside.png",
            "foo\\outside.png",
        ] {
            assert!(decode_path(path).is_err());
        }
    }
    #[test]
    fn unavailable_fork_only_blocks_relative_images_and_keeps_independent_sources() {
        let mut f = fixture(
            "![relative](./ready.png)\n![attachment](https://github.com/user-attachments/assets/ready)",
        );
        f.responses.get_mut("repos/base/repo/pulls/1").unwrap()["head"]["repo"] = Value::Null;
        let result = retrieve(&context(), "request".into(), None, 1, &f).unwrap();
        assert_eq!(result.listing.items.len(), 1);
        assert!(result.listing.items[0].provenance[0].contains("GitHub attachment"));
        assert!(
            result
                .listing
                .issues
                .iter()
                .any(|issue| issue.message.contains("relative images cannot be resolved"))
        );
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call.contains("/actions/runs?"))
        );
    }
    #[test]
    fn public_manifest_and_protected_gallery_have_distinct_behavior() {
        let mut f = fixture(
            "[Screenshot gallery](https://example.com/amf-screenshots.json)\n[Private gallery](https://private.example/gallery)\n[Documentation](https://example.com/docs)",
        );
        f.downloads.insert("https://example.com/amf-screenshots.json".into(),serde_json::to_vec(&json!({"version":1,"images":[{"id":"ready","url":"./ready.png","caption":"Ready"}]})).unwrap());
        let result = retrieve(&context(), "request".into(), None, 1, &f).unwrap();
        assert_eq!(result.listing.items.len(), 1);
        assert_eq!(result.listing.galleries.len(), 1);
        assert!(
            matches!(result.resources.values().next().unwrap(),Resource::Gallery {image_id,..} if image_id=="ready")
        );
        assert!(
            !f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.contains("example.com/docs"))
        );
    }
    #[test]
    fn redirects_keep_credentials_only_on_original_origin_and_refuse_unsafe_destinations() {
        let initial =
            validated_url("https://api.github.com/repos/owner/repo/artifacts/1/zip").unwrap();
        let (_, retain) = redirect_destination(&initial, "/fresh", &initial.origin()).unwrap();
        assert!(retain);
        let (signed, retain) = redirect_destination(
            &initial,
            "https://release-assets.githubusercontent.com/archive?signature=fixture",
            &initial.origin(),
        )
        .unwrap();
        assert!(!retain);
        // The caller only clears its token; returning to GitHub cannot restore it.
        let mut token = Some("fixture");
        if !retain {
            token = None;
        }
        let (_, retain) =
            redirect_destination(&signed, "https://api.github.com/final", &initial.origin())
                .unwrap();
        assert!(retain && token.is_none());
        for destination in [
            "http://api.github.com/downgrade",
            "https://127.0.0.1/private",
            "file:///tmp/escape",
            "https://user:secret@example.com/image",
        ] {
            assert!(redirect_destination(&initial, destination, &initial.origin()).is_err());
        }
    }
    #[test]
    fn attachment_authentication_is_separate_from_repository_retrieval() {
        let f = fixture("<img src=\"https://github.com/user-attachments/assets/asset\">");
        let result = retrieve(&context(), "request".into(), None, 1, &f).unwrap();
        let resource = result.resources.values().next().unwrap();
        assert!(resource_image(&context(), resource, true, &f).is_err());
        assert!(
            f.calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "http true https://github.com/user-attachments/assets/asset")
        );
        assert!(validated_url("http://127.0.0.1/x").is_err());
        assert!(validated_url("file:///tmp/x").is_err());
        assert!(validated_url("https://user:secret@example.com/x").is_err());
        assert!(!public_ip("10.0.0.1".parse().unwrap()));
        assert!(!public_ip("::ffff:127.0.0.1".parse().unwrap()));
    }
    #[test]
    fn default_run_skips_new_empty_running_expired_and_old_commit_runs() {
        let mut f = fixture("");
        let runs = json!([
            {"id":1,"head_sha":"old","status":"completed","created_at":"2026-10-07T04:00:00Z","pull_requests":[{"number":1}]},
            {"id":2,"head_sha":"current","status":"in_progress","created_at":"2026-10-07T03:00:00Z"},
            {"id":3,"head_sha":"current","status":"completed","created_at":"2026-10-07T02:00:00Z"},
            {"id":4,"head_sha":"current","status":"completed","conclusion":"failure","run_attempt":2,"created_at":"2026-10-07T01:00:00Z"}]);
        f.responses.insert(
            "repos/base/repo/actions/runs?per_page=100&page=1".into(),
            json!({"workflow_runs":runs}),
        );
        f.responses.insert(
            "repos/base/repo/actions/runs?head_sha=current&status=completed&per_page=100&page=1"
                .into(),
            json!({"workflow_runs":runs}),
        );
        f.responses.insert(
            "repos/base/repo/actions/runs/3/artifacts?per_page=100&page=1".into(),
            json!({"artifacts":[{"id":30,"expired":true}]}),
        );
        f.responses.insert(
            "repos/base/repo/actions/runs/4/artifacts?per_page=100&page=1".into(),
            json!({"artifacts":[{"id":40,"expired":false,"name":"visual"}]}),
        );
        f.downloads.insert(
            "https://api.github.com/repos/base/repo/actions/artifacts/40/zip".into(),
            zip(&[("ready.png", png())]),
        );
        let result = retrieve(&context(), "request".into(), None, 1, &f).unwrap();
        assert_eq!(result.listing.selected_run, Some(4));
        assert_eq!(result.listing.items.len(), 1);
        assert!(result.listing.items[0].provenance[0].contains("artifact 40"));
        assert!(result.listing.items[0].provenance[0].contains("artifact attempt not provided"));
        let old = retrieve(&context(), "old-request".into(), Some(1), 1, &f).unwrap();
        assert_eq!(old.listing.selected_run, Some(1));
        assert!(old.listing.items.is_empty());
    }
    #[test]
    fn rerun_started_later_wins_across_pages_and_older_page_is_selectable() {
        let mut f = fixture("");
        let page_one: Vec<_> = (1..=100).map(|id| json!({"id":id,"head_sha":"current","status":"completed","created_at":"2026-10-01T00:00:00Z"})).collect();
        let rerun = json!({"id":900,"name":"Rerun","head_sha":"current","status":"completed","run_attempt":3,"created_at":"2026-09-01T00:00:00Z","run_started_at":"2026-10-07T01:00:00Z"});
        for query in ["", "head_sha=current&status=completed&"] {
            f.responses.insert(
                format!("repos/base/repo/actions/runs?{query}per_page=100&page=1"),
                json!({"workflow_runs":page_one}),
            );
            f.responses.insert(
                format!("repos/base/repo/actions/runs?{query}per_page=100&page=2"),
                json!({"workflow_runs":[rerun]}),
            );
        }
        f.responses.insert(
            "repos/base/repo/actions/runs/900/artifacts?per_page=100&page=1".into(),
            json!({"artifacts":[{"id":9000,"expired":false,"name":"screenshots"}]}),
        );
        f.downloads.insert(
            "https://api.github.com/repos/base/repo/actions/artifacts/9000/zip".into(),
            zip(&[("ready.png", png())]),
        );
        let default = retrieve(&context(), "default".into(), None, 1, &f).unwrap();
        assert_eq!(default.listing.selected_run, Some(900));
        assert!(default.listing.more_runs);
        let older = retrieve(&context(), "older".into(), Some(900), 2, &f).unwrap();
        assert_eq!(older.listing.run_page, 2);
        assert!(!older.listing.more_runs);
        assert_eq!(default.listing.items[0].key, older.listing.items[0].key);
        assert!(older.listing.items[0].provenance[0].contains("current attempt 3"));
    }
    #[test]
    fn comments_paginate_and_bad_archive_images_do_not_hide_valid_neighbors() {
        let mut f = fixture("");
        let first: Vec<_> = (0..100)
            .map(|id| json!({"id":id,"body":"No images"}))
            .collect();
        f.responses.insert(
            "repos/base/repo/issues/1/comments?per_page=100&page=1".into(),
            json!(first),
        );
        f.responses.insert(
            "repos/base/repo/issues/1/comments?per_page=100&page=2".into(),
            json!([{"id":100,"body":"![second-page](./ready.png)"}]),
        );
        assert_eq!(
            retrieve(&context(), "request".into(), None, 1, &f)
                .unwrap()
                .listing
                .items
                .len(),
            1
        );
        let result = archive_images(&zip(&[
            ("bad.png", b"not an image".to_vec()),
            ("ready.png", png()),
        ]))
        .unwrap();
        assert_eq!(result.images.len(), 1);
        assert_eq!(result.issues.len(), 1);
        assert!(archive_images(&zip(&[("nested.zip", zip(&[("ready.png", png())]))])).is_err());
    }
    #[test]
    fn archives_are_bounded_and_path_escape_is_rejected_without_extracting() {
        assert_eq!(
            archive_images(&zip(&[("nested/ready.png", png())]))
                .unwrap()
                .images
                .len(),
            1
        );
        for path in [
            "../ready.png",
            "/ready.png",
            "C:/ready.png",
            "a\\ready.png",
            "a//ready.png",
        ] {
            assert!(
                archive_images(&zip(&[(path, png())])).is_err(),
                "accepted {path}"
            );
        }
    }
}
