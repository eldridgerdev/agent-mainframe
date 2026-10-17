//! User authoring uses targeted SQLite writes; declarative sources are read-only.
use super::{LibraryEntry, LibraryScope, load};
use crate::db::prompt_templates::{Mutation, Snapshot};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::prompt_library::{PromptTemplate, format_tags, validate_user_draft};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct UserPromptEditor {
    pub id: String,
    pub revision: String,
    pub name: String,
    pub body: String,
    pub tags: String,
    pub description: Option<String>,
    pub configured_fields: usize,
}
impl From<Snapshot> for UserPromptEditor {
    fn from(snapshot: Snapshot) -> Self {
        Self {
            id: snapshot.template.id,
            revision: snapshot.revision,
            name: snapshot.template.name,
            body: snapshot.template.body,
            tags: format_tags(&snapshot.template.tags),
            description: snapshot.template.description,
            configured_fields: snapshot.template.placeholders.len(),
        }
    }
}
#[derive(Debug, Deserialize)]
pub struct UserPromptDraft {
    pub id: Option<String>,
    pub revision: Option<String>,
    pub name: String,
    pub body: String,
    pub tags: String,
}

fn stale() -> GuiError {
    GuiError::conflict(
        "This User template changed or was deleted. Keep your draft, then reload the template before saving or deleting.",
    )
}
pub(super) fn selected(
    gui: &mut GuiHandle,
    scope: &LibraryScope,
    key: &str,
) -> GuiResult<LibraryEntry> {
    load(gui, scope, "")?
        .entries
        .into_iter()
        .find(|entry| entry.key == key)
        .ok_or_else(|| {
            GuiError::conflict(
                "This template or library scope changed. Refresh and select it again.",
            )
        })
}
fn user_snapshot(gui: &GuiHandle, entry: &LibraryEntry) -> GuiResult<Snapshot> {
    if entry.source != "User" {
        return Err(GuiError::conflict(
            "Only User templates can be edited or deleted here. Duplicate this source to User first.",
        ));
    }
    let snapshot = gui
        .db()?
        .prompt_template_snapshot(&entry.template.id)?
        .ok_or_else(stale)?;
    if snapshot.template != entry.template {
        return Err(stale());
    }
    Ok(snapshot)
}

pub fn edit(gui: &mut GuiHandle, scope: &LibraryScope, key: &str) -> GuiResult<UserPromptEditor> {
    let entry = selected(gui, scope, key)?;
    Ok(user_snapshot(gui, &entry)?.into())
}
pub fn reload(gui: &GuiHandle, id: &str) -> GuiResult<UserPromptEditor> {
    Ok(gui
        .db()?
        .prompt_template_snapshot(id)?
        .ok_or_else(stale)?
        .into())
}

pub fn save(gui: &mut GuiHandle, draft: UserPromptDraft) -> GuiResult<String> {
    let (name, body, tags) = validate_user_draft(&draft.name, &draft.body, &draft.tags)?;
    match (draft.id, draft.revision) {
        (None, None) => {
            let mut template = PromptTemplate::new(name, body);
            template.tags = tags;
            gui.db()?.insert_prompt_template(&template)?;
            Ok(template.id)
        }
        (Some(id), Some(revision)) => gui
            .db()?
            .mutate_prompt_template(&id, &revision, Mutation::Edit { name, body, tags })?
            .ok_or_else(stale),
        _ => Err(GuiError::conflict(
            "The User template identity is incomplete; reload before saving.",
        )),
    }
}
pub fn delete(gui: &mut GuiHandle, id: &str, revision: &str) -> GuiResult<()> {
    gui.db()?
        .mutate_prompt_template(id, revision, Mutation::Delete)?
        .ok_or_else(stale)?;
    Ok(())
}
pub fn duplicate(gui: &mut GuiHandle, scope: &LibraryScope, key: &str) -> GuiResult<String> {
    let entry = selected(gui, scope, key)?;
    if entry.source == "User" {
        let snapshot = user_snapshot(gui, &entry)?;
        return gui
            .db()?
            .mutate_prompt_template(
                &snapshot.template.id,
                &snapshot.revision,
                Mutation::Duplicate,
            )?
            .ok_or_else(stale);
    }
    // Browsing is intentionally tolerant. Duplication must instead check the
    // actual authored metadata, refusing fields a User row cannot preserve.
    let raw = declarative_row(gui, scope, &entry)?;
    validate_declarative_metadata(&raw)?;
    let parsed: PromptTemplate = serde_json::from_value(raw).map_err(anyhow::Error::from)?;
    if parsed.name != entry.template.name
        || parsed.body != entry.template.body
        || parsed.description != entry.template.description
        || parsed.tags != entry.template.tags
        || parsed.placeholders != entry.template.placeholders
    {
        return Err(GuiError::conflict(
            "The source template changed while copying. Refresh and retry.",
        ));
    }
    let copy = entry.template.user_copy();
    gui.db()?.insert_prompt_template(&copy)?;
    Ok(copy.id)
}
fn declarative_row(
    gui: &mut GuiHandle,
    scope: &LibraryScope,
    entry: &LibraryEntry,
) -> GuiResult<serde_json::Value> {
    let app = gui.app_for_workflow();
    #[cfg(test)]
    if entry.source == "Global" && app.store_path.as_os_str().is_empty() {
        return serde_json::to_value(&entry.template)
            .map_err(anyhow::Error::from)
            .map_err(Into::into);
    }
    let repo = match scope {
        LibraryScope::Global => None,
        LibraryScope::Project { project_id } | LibraryScope::Feature { project_id, .. } => app
            .store
            .projects
            .iter()
            .find(|project| &project.id == project_id)
            .map(|project| &project.repo),
    };
    let path = match entry.source {
        "Global" => crate::project::amf_config_dir().join("config.json"),
        "Project" => repo
            .and_then(|repo| crate::extension::resolve_project_config_path(repo))
            .ok_or_else(|| GuiError::conflict("Project config was removed"))?,
        "Worktree" => {
            let LibraryScope::Feature {
                project_id,
                feature_id,
            } = scope
            else {
                return Err(GuiError::conflict("Feature scope was removed"));
            };
            let (pi, fi) = app
                .store
                .locate_feature_by_id(Some(project_id), feature_id)
                .ok_or_else(|| GuiError::not_found("Feature was deleted"))?;
            crate::extension::resolve_project_config_path(
                &app.store.projects[pi].features[fi].workdir,
            )
            .ok_or_else(|| GuiError::conflict("Worktree config was removed"))?
        }
        _ => return Err(GuiError::conflict("Unknown prompt source")),
    };
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(anyhow::Error::from)?)
            .map_err(anyhow::Error::from)?;
    let templates = if entry.source == "Global" {
        &value["extension"]["prompt_templates"]
    } else {
        &value["prompt_templates"]
    };
    templates
        .as_array()
        .and_then(|rows| rows.get(entry.source_index))
        .cloned()
        .ok_or_else(|| GuiError::conflict("Source template was removed or reordered while copying"))
}
fn validate_declarative_metadata(value: &serde_json::Value) -> GuiResult<()> {
    let fail = || {
        GuiError::conflict(
            "This template contains unsupported metadata; copying it to User could discard fields. Edit its source before duplicating.",
        )
    };
    let object = value.as_object().ok_or_else(fail)?;
    if object.keys().any(|key| {
        ![
            "id",
            "name",
            "description",
            "body",
            "tags",
            "placeholders",
            "created_at",
            "updated_at",
        ]
        .contains(&key.as_str())
    }) {
        return Err(fail());
    }
    if let Some(placeholders) = object.get("placeholders") {
        for placeholder in placeholders.as_array().ok_or_else(fail)? {
            let fields = placeholder.as_object().ok_or_else(fail)?;
            let kind = fields
                .get("kind")
                .and_then(|kind| kind.as_str())
                .unwrap_or("text");
            let allowed = match kind {
                "text" | "multi_line" => vec!["key", "label", "kind", "required", "default"],
                "select" => vec!["key", "label", "kind", "required", "options"],
                _ => return Err(fail()),
            };
            if fields.keys().any(|key| !allowed.contains(&key.as_str())) {
                return Err(fail());
            }
            let mut kind_value = placeholder.clone();
            kind_value
                .as_object_mut()
                .ok_or_else(fail)?
                .entry("kind")
                .or_insert_with(|| serde_json::Value::String("text".into()));
            serde_json::from_value::<crate::prompt_library::PlaceholderKind>(kind_value)
                .map_err(|_| fail())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "authoring_tests.rs"]
mod tests;
