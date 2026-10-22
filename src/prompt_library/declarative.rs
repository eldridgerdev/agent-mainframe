//! Scoped prompt-file operations shared by desktop and TUI authoring.
//!
//! Keep the raw document, take the ordinary config writer locks in sorted order,
//! recheck bytes/path identities and atomically replace one canonical document.
//! Hand editors do not take these locks: their final-read/rename race cannot be
//! made into a filesystem compare-and-swap, so stale reads are refused as late
//! as possible without claiming protection against every uncooperative writer.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{PlaceholderKind, PromptPlaceholder, PromptTemplate, validate_user_draft};

const KNOWN_FIELDS: &[&str] = &[
    "id",
    "name",
    "body",
    "description",
    "tags",
    "placeholders",
    "created_at",
    "updated_at",
];

pub(crate) fn revision(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Location {
    Global(PathBuf),
    Local(PathBuf),
}
impl Location {
    pub fn canonical(&self) -> PathBuf {
        match self {
            Self::Global(path) => path.clone(),
            Self::Local(repo) => crate::extension::project_config_path(repo),
        }
    }
    fn legacy(&self) -> Option<PathBuf> {
        match self {
            Self::Global(_) => None,
            Self::Local(repo) => Some(crate::extension::legacy_project_config_path(repo)),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Document {
    pub location: Location,
    pub active_path: PathBuf,
    pub value: Value,
    pub binding: String,
    legacy_bytes: Option<Vec<u8>>,
}

fn bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn metadata_identity(path: &Path) -> Option<Value> {
    let metadata = std::fs::metadata(path).ok();
    #[cfg(unix)]
    let metadata = metadata.map(|value| {
        use std::os::unix::fs::MetadataExt;
        json!([
            value.dev(),
            value.ino(),
            value.mode(),
            value.uid(),
            value.gid()
        ])
    });
    #[cfg(not(unix))]
    let metadata = metadata.map(|value| json!([value.len(), value.permissions().readonly()]));
    metadata
}
fn path_identity(path: &Path) -> Value {
    let spelling = |path: PathBuf| path.to_string_lossy().into_owned();
    json!({"path":path.to_string_lossy(),"resolved":std::fs::canonicalize(path).ok().map(spelling),
        "link":std::fs::read_link(path).ok().map(spelling),"metadata":metadata_identity(path),
        "parent":path.parent().map(|parent|json!({"resolved":std::fs::canonicalize(parent).ok().map(spelling),"metadata":metadata_identity(parent)}))})
}
fn document_binding(
    current: &Option<Vec<u8>>,
    legacy: &Option<Vec<u8>>,
    canonical: &Path,
    legacy_path: Option<&Path>,
) -> String {
    let value = json!([
        current,
        legacy,
        path_identity(canonical),
        legacy_path.map(path_identity)
    ]);
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

/// Interpret known fields strictly; opaque extra fields stay in the raw value.
/// Flattened serde defaults alone can hide broken placeholder-kind values.
pub(crate) fn validate_template(value: &Value) -> Result<PromptTemplate> {
    ensure!(value.is_object(), "A prompt template must be a JSON object");
    let template: PromptTemplate = serde_json::from_value(value.clone())?;
    if let Some(placeholders) = value.get("placeholders") {
        for field in placeholders
            .as_array()
            .context("Placeholder metadata must be an array")?
        {
            let mut kind = field.clone();
            let object = kind
                .as_object_mut()
                .context("A placeholder must be an object")?;
            object
                .entry("kind")
                .or_insert_with(|| Value::String("text".into()));
            serde_json::from_value::<PlaceholderKind>(kind)?;
            serde_json::from_value::<PromptPlaceholder>(field.clone())?;
        }
    }
    Ok(template)
}

impl Document {
    pub fn read(location: Location) -> Result<Self> {
        if let Location::Local(repo) = &location {
            ensure!(repo.is_dir(), "The prompt destination checkout was removed");
        }
        let canonical = location.canonical();
        ensure!(
            canonical.parent().is_some_and(Path::is_dir),
            "The prompt configuration directory was removed"
        );
        let legacy = location.legacy();
        let current_bytes = bytes(&canonical)?;
        let legacy_bytes = legacy
            .as_ref()
            .map(|path| bytes(path))
            .transpose()?
            .flatten();
        let active_path = if current_bytes.is_some() {
            canonical.clone()
        } else if legacy_bytes.is_some() {
            legacy.clone().unwrap()
        } else {
            canonical.clone()
        };
        let value: Value = match current_bytes.as_ref().or(legacy_bytes.as_ref()) {
            Some(bytes) => {
                serde_json::from_slice(bytes).context("Prompt config contains malformed JSON")?
            }
            None => json!({}),
        };
        ensure!(
            value.is_object(),
            "Prompt config must contain a JSON object"
        );
        match location {
            Location::Global(_) => {
                serde_json::from_value::<crate::app::AppConfig>(value.clone())?;
            }
            Location::Local(_) => {
                serde_json::from_value::<crate::extension::ExtensionConfig>(value.clone())?;
            }
        }
        let binding =
            document_binding(&current_bytes, &legacy_bytes, &canonical, legacy.as_deref());
        let document = Self {
            location,
            active_path,
            value,
            binding,
            legacy_bytes,
        };
        for template in document.rows()? {
            validate_template(template)?;
        }
        Ok(document)
    }
    pub fn rows(&self) -> Result<&[Value]> {
        let container = match self.location {
            Location::Global(_) => match self.value.get("extension") {
                Some(extension) => {
                    ensure!(extension.is_object(), "Global extension must be an object");
                    extension
                }
                None => return Ok(&[]),
            },
            Location::Local(_) => &self.value,
        };
        match container.get("prompt_templates") {
            Some(rows) => Ok(rows
                .as_array()
                .context("prompt_templates must be an array")?),
            None => Ok(&[]),
        }
    }
    fn replace_rows(&self, rows: Vec<Value>) -> Result<Value> {
        let mut value = self.value.clone();
        let container = match self.location {
            Location::Global(_) => value
                .as_object_mut()
                .unwrap()
                .entry("extension")
                .or_insert_with(|| json!({})),
            Location::Local(_) => &mut value,
        };
        container
            .as_object_mut()
            .context("Prompt config container must be an object")?
            .insert("prompt_templates".into(), rows.into());
        Ok(value)
    }
    fn current(&self) -> Result<Self> {
        let current = Self::read(self.location.clone())?;
        ensure!(
            current.binding == self.binding,
            "Prompt config or its path changed. Keep your draft and reload before saving."
        );
        Ok(current)
    }
    fn write(&self, value: &Value) -> Result<Self> {
        self.current()?;
        if *value == self.value {
            return Ok(self.clone());
        }
        let encoded = serde_json::to_vec_pretty(value)?;
        let committed_value = value.clone();
        let canonical = self.location.canonical();
        crate::app::setup::write_json_document(
            &self.location.canonical(),
            value,
            &self.active_path,
        )?;
        // Atomic rename succeeded. Return the document we committed instead
        // of making a fallible post-write reload look like a failed save.
        // A later external write is detected on the next read/save.
        let binding = document_binding(
            &Some(encoded),
            &self.legacy_bytes,
            &canonical,
            self.location.legacy().as_deref(),
        );
        Ok(Self {
            location: self.location.clone(),
            active_path: canonical,
            value: committed_value,
            binding,
            legacy_bytes: self.legacy_bytes.clone(),
        })
    }
}

struct Locks(Vec<std::fs::File>);
impl Drop for Locks {
    fn drop(&mut self) {
        // Explicit unlock also releases a lock briefly inherited across a
        // concurrent process spawn; waiting for the last OFD close is unsafe.
        for lock in &self.0 {
            let _ = std::fs::File::unlock(lock);
        }
    }
}
fn lock_documents(documents: &[&Document]) -> Result<Locks> {
    let mut paths: Vec<_> = documents
        .iter()
        .flat_map(|document| {
            let path = document.location.canonical();
            let logical = path
                .parent()
                .and_then(|parent| std::fs::canonicalize(parent).ok())
                .and_then(|parent| path.file_name().map(|name| parent.join(name)))
                .unwrap_or_else(|| path.clone());
            let mut paths = vec![logical];
            // Alias normalization avoids reacquiring our own lock or reversing
            // acquisition order for two paths to the same symlink target. The
            // logical lock also interoperates with ordinary settings writers.
            if let Ok(physical) = std::fs::canonicalize(path) {
                paths.push(physical);
            }
            paths
        })
        .collect();
    // lock_config derives a sidecar with with_extension("json.lock").
    // Distinct config names (including symlink targets) can share that exact
    // sidecar. Order and deduplicate the locks we acquire, not the input names.
    let mut by_lock = std::collections::BTreeMap::new();
    for path in paths.drain(..) {
        let sidecar = path.with_extension("json.lock");
        let identity = std::fs::canonicalize(&sidecar).unwrap_or(sidecar);
        by_lock.entry(identity).or_insert(path);
    }
    let paths = by_lock.into_values();
    let mut locks = Locks(Vec::new());
    for path in paths {
        std::fs::create_dir_all(path.parent().context("Missing config directory")?)?;
        locks.0.push(crate::app::setup::lock_config(&path)?);
    }
    Ok(locks)
}

#[derive(Clone, Debug)]
pub(crate) struct Edit {
    pub document: Document,
    pub ordinal: usize,
    pub raw: Value,
}
impl Edit {
    pub fn read(document: Document, ordinal: usize) -> Result<Self> {
        let raw = document
            .rows()?
            .get(ordinal)
            .context("This template was removed or reordered")?
            .clone();
        validate_template(&raw)?;
        Ok(Self {
            document,
            ordinal,
            raw,
        })
    }
    pub fn binding(&self) -> Result<String> {
        revision(&(&self.document.binding, self.ordinal, &self.raw))
    }
    pub fn reload(&self) -> Result<Self> {
        let fresh = Self::read(
            Document::read(self.document.location.clone())?,
            self.ordinal,
        )?;
        ensure!(
            fresh.raw.get("name") == self.raw.get("name")
                && fresh.raw.get("id") == self.raw.get("id"),
            "The source rows moved or were renamed. Keep your edits and select the current template again."
        );
        ensure!(
            fresh
                .document
                .rows()?
                .iter()
                .filter(|row| row.get("name") == fresh.raw.get("name"))
                .count()
                == 1,
            "The source name is ambiguous. Keep your edits and select the current template again."
        );
        Ok(fresh)
    }
    pub fn commit(
        &self,
        name: &str,
        body: &str,
        tags: &str,
        mut verify_context: impl FnMut() -> Result<()>,
    ) -> Result<Self> {
        let (name, body, tags) = validate_user_draft(name, body, tags)?;
        let _locks = lock_documents(&[&self.document])?;
        verify_context()?;
        let current = self.document.current()?;
        ensure!(
            current.rows()?.get(self.ordinal) == Some(&self.raw),
            "The source template changed or moved. Reload before saving."
        );
        let mut rows = current.rows()?.to_vec();
        ensure!(
            name == self.raw["name"]
                || !rows
                    .iter()
                    .enumerate()
                    .any(|(index, row)| index != self.ordinal && row["name"] == name),
            "Another template in this destination already has that name. Choose another name."
        );
        let row = rows[self.ordinal].as_object_mut().unwrap();
        row.insert("name".into(), name.into());
        row.insert("body".into(), body.into());
        row.insert("tags".into(), tags.into());
        let committed_raw = rows[self.ordinal].clone();
        let updated = current.replace_rows(rows)?;
        verify_context()?;
        Ok(Self {
            document: current.write(&updated)?,
            ordinal: self.ordinal,
            raw: committed_raw,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExportSource {
    pub raw: Value,
    pub revision: String,
    pub file: Option<Edit>,
}
impl ExportSource {
    pub fn from_file(edit: Edit) -> Result<Self> {
        Ok(Self {
            revision: edit.binding()?,
            raw: edit.raw.clone(),
            file: Some(edit),
        })
    }
    fn verify(&self) -> Result<()> {
        if let Some(edit) = &self.file {
            let current = Edit::read(edit.document.current()?, edit.ordinal)?;
            ensure!(
                current.raw == self.raw,
                "The source template changed before export"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Export {
    pub source: ExportSource,
    pub destination: Document,
    pub replacing: bool,
    pub no_op: bool,
    updated: Value,
}
impl Export {
    pub fn prepare(source: ExportSource, destination: Document) -> Result<Self> {
        let template = validate_template(&source.raw)?;
        let mut rows = destination.rows()?.to_vec();
        let matches: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row["name"] == template.name)
            .map(|(index, _)| index)
            .collect();
        ensure!(
            matches.len() <= 1,
            "Multiple destination templates have that name. Resolve the ambiguity before exporting."
        );
        let replacing = !matches.is_empty();
        if let Some(&index) = matches.first() {
            // Replace known template fields from the source, while retaining
            // destination-only opaque metadata and all source opaque metadata.
            let mut merged = rows[index].as_object().unwrap().clone();
            for key in KNOWN_FIELDS {
                merged.remove(*key);
            }
            merged.extend(source.raw.as_object().unwrap().clone());
            rows[index] = merged.into();
        } else {
            rows.push(source.raw.clone());
        }
        let updated = destination.replace_rows(rows)?;
        let no_op = updated == destination.value;
        Ok(Self {
            source,
            destination,
            replacing,
            no_op,
            updated,
        })
    }
    pub fn binding(&self) -> Result<String> {
        revision(&(
            &self.source.revision,
            &self.source.raw,
            &self.destination.binding,
            &self.updated,
        ))
    }
    pub fn commit(
        &self,
        replace_confirmed: bool,
        mut verify_context_and_source: impl FnMut() -> Result<()>,
    ) -> Result<Document> {
        ensure!(
            !self.replacing || replace_confirmed || self.no_op,
            "Confirm replacement of the same-name destination template before exporting"
        );
        let mut documents = vec![&self.destination];
        if let Some(edit) = &self.source.file {
            documents.push(&edit.document);
        }
        let _locks = lock_documents(&documents)?;
        verify_context_and_source()?;
        self.source.verify()?;
        let current = self.destination.current()?;
        verify_context_and_source()?;
        self.source.verify()?;
        current.write(&self.updated)
    }
}

#[cfg(test)]
#[path = "declarative_tests.rs"]
mod tests;

/// Stable workspace identity independent of frontend/TUI selection indices.
#[derive(Clone, Debug)]
pub(crate) struct WorkspaceContext {
    pub project_id: Option<String>,
    pub feature_id: Option<String>,
    repo: Option<PathBuf>,
    workdir: Option<PathBuf>,
    pub binding: String,
}
impl WorkspaceContext {
    pub fn capture(
        store: &crate::project::ProjectStore,
        project_id: Option<&str>,
        feature_id: Option<&str>,
    ) -> Result<Self> {
        let project = project_id
            .map(|id| {
                store
                    .projects
                    .iter()
                    .find(|project| project.id == id)
                    .context("The project was deleted")
            })
            .transpose()?;
        let feature = feature_id
            .map(|id| {
                project
                    .context("The feature has no project")?
                    .features
                    .iter()
                    .find(|feature| feature.id == id)
                    .context("The feature was deleted")
            })
            .transpose()?;
        let repo = project.map(|project| project.repo.clone());
        let workdir = feature.map(|feature| feature.workdir.clone());
        ensure!(
            repo.as_ref().is_none_or(|path| path.is_dir())
                && workdir.as_ref().is_none_or(|path| path.is_dir()),
            "The prompt checkout was removed"
        );
        let binding = revision(&(
            project_id,
            feature_id,
            &repo,
            &workdir,
            repo.as_deref().map(path_identity),
            workdir.as_deref().map(path_identity),
        ))?;
        Ok(Self {
            project_id: project_id.map(str::to_owned),
            feature_id: feature_id.map(str::to_owned),
            repo,
            workdir,
            binding,
        })
    }
    pub fn verify(&self, store: &crate::project::ProjectStore) -> Result<()> {
        let current = Self::capture(
            store,
            self.project_id.as_deref(),
            self.feature_id.as_deref(),
        )?;
        ensure!(
            current.binding == self.binding,
            "The project or feature checkout changed. Keep your draft and reselect its destination."
        );
        Ok(())
    }
    pub fn local_project(&self) -> Result<Location> {
        Ok(Location::Local(
            self.repo.clone().context("Choose a project destination")?,
        ))
    }
    pub fn local_worktree(&self) -> Result<Location> {
        let workdir = self
            .workdir
            .clone()
            .context("Choose a feature worktree destination")?;
        ensure!(
            self.repo.as_ref().is_none_or(
                |repo| std::fs::canonicalize(repo).ok() != std::fs::canonicalize(&workdir).ok()
            ),
            "This feature shares its project checkout; choose the project destination"
        );
        Ok(Location::Local(workdir))
    }
}
