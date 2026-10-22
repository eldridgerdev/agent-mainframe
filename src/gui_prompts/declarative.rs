//! Declarative prompt editing/export without driving the TUI's AppMode.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{LibraryEntry, LibraryScope, authoring::selected};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult};
use crate::prompt_library::{
    declarative::{
        self as files, Document, Edit, Export, ExportSource, Location, WorkspaceContext,
    },
    format_tags,
};

fn conflict(error: anyhow::Error) -> GuiError {
    GuiError::conflict(error.to_string())
}
fn context(gui: &mut GuiHandle, scope: &LibraryScope) -> GuiResult<WorkspaceContext> {
    gui.refresh_store()?;
    let (project, feature) = match scope {
        LibraryScope::Global => (None, None),
        LibraryScope::Project { project_id } => (Some(project_id.as_str()), None),
        LibraryScope::Feature {
            project_id,
            feature_id,
        } => (Some(project_id.as_str()), Some(feature_id.as_str())),
    };
    WorkspaceContext::capture(&gui.app_for_workflow().store, project, feature).map_err(conflict)
}
fn verify_context(gui: &mut GuiHandle, contexts: &[&WorkspaceContext]) -> Result<()> {
    gui.refresh_store()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    for context in contexts {
        context.verify(&gui.app_for_workflow().store)?;
    }
    Ok(())
}
fn global() -> Location {
    Location::Global(crate::project::amf_config_dir().join("config.json"))
}
fn location(context: &WorkspaceContext, source: &str) -> Result<Location> {
    match source {
        "Global" => Ok(global()),
        "Project" => context.local_project(),
        "Worktree" => context.local_worktree(),
        _ => anyhow::bail!("Choose a declarative template; User templates have their own editor"),
    }
}
fn matches_entry(edit: &Edit, entry: &LibraryEntry) -> Result<()> {
    let template = files::validate_template(&edit.raw)?;
    ensure!(
        template.name == entry.template.name
            && template.body == entry.template.body
            && template.tags == entry.template.tags
            && template.description == entry.template.description
            && template.placeholders == entry.template.placeholders,
        "The source template changed or moved. Refresh the library and select it again."
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FileSelection {
    pub scope: LibraryScope,
    pub source: String,
    pub ordinal: usize,
}
#[derive(Serialize)]
pub struct DeclarativeEditor {
    pub selection: FileSelection,
    pub context_revision: String,
    pub revision: String,
    pub name: String,
    pub body: String,
    pub tags: String,
    pub description: Option<String>,
    pub configured_fields: usize,
    pub original_id: Option<String>,
    pub original_name: String,
    pub source_path: String,
    pub destination_path: String,
}
fn view(
    selection: FileSelection,
    context: &WorkspaceContext,
    edit: &Edit,
) -> Result<DeclarativeEditor> {
    let template = files::validate_template(&edit.raw)?;
    Ok(DeclarativeEditor {
        selection,
        context_revision: context.binding.clone(),
        revision: edit.binding()?,
        original_id: edit
            .raw
            .get("id")
            .and_then(|id| id.as_str())
            .map(str::to_owned),
        original_name: template.name.clone(),
        name: template.name,
        body: template.body,
        tags: format_tags(&template.tags),
        description: template.description,
        configured_fields: template.placeholders.len(),
        source_path: edit.document.active_path.display().to_string(),
        destination_path: edit.document.location.canonical().display().to_string(),
    })
}
pub fn edit(gui: &mut GuiHandle, scope: LibraryScope, key: &str) -> GuiResult<DeclarativeEditor> {
    let entry = selected(gui, &scope, key)?;
    let context = context(gui, &scope)?;
    let document =
        Document::read(location(&context, entry.source).map_err(conflict)?).map_err(conflict)?;
    let edit = Edit::read(document, entry.source_index).map_err(conflict)?;
    matches_entry(&edit, &entry).map_err(conflict)?;
    view(
        FileSelection {
            scope,
            source: entry.source.into(),
            ordinal: entry.source_index,
        },
        &context,
        &edit,
    )
    .map_err(conflict)
}
#[derive(Deserialize)]
pub struct ReloadRequest {
    pub selection: FileSelection,
    pub context_revision: String,
    pub original_name: String,
    pub original_id: Option<String>,
}
pub fn reload(gui: &mut GuiHandle, request: ReloadRequest) -> GuiResult<DeclarativeEditor> {
    let context = context(gui, &request.selection.scope)?;
    if context.binding != request.context_revision {
        return Err(GuiError::conflict(
            "The library checkout changed. Keep your draft and reselect the source.",
        ));
    }
    let document = Document::read(location(&context, &request.selection.source).map_err(conflict)?)
        .map_err(conflict)?;
    let edit = Edit::read(document, request.selection.ordinal).map_err(conflict)?;
    let id = edit
        .raw
        .get("id")
        .and_then(|id| id.as_str())
        .map(str::to_owned);
    if edit.raw["name"] != request.original_name
        || id != request.original_id
        || edit
            .document
            .rows()
            .map_err(conflict)?
            .iter()
            .filter(|row| row["name"] == request.original_name)
            .count()
            != 1
    {
        return Err(GuiError::conflict(
            "Source rows moved, were renamed or are ambiguous. Keep your edits and select the current template again.",
        ));
    }
    view(request.selection, &context, &edit).map_err(conflict)
}
#[derive(Deserialize)]
pub struct SaveRequest {
    pub selection: FileSelection,
    pub context_revision: String,
    pub revision: String,
    pub name: String,
    pub body: String,
    pub tags: String,
}
pub fn save(gui: &mut GuiHandle, request: SaveRequest) -> GuiResult<()> {
    let context = context(gui, &request.selection.scope)?;
    if context.binding != request.context_revision {
        return Err(GuiError::conflict(
            "The library checkout changed. Keep your draft and reselect the source.",
        ));
    }
    let document = Document::read(location(&context, &request.selection.source).map_err(conflict)?)
        .map_err(conflict)?;
    let edit = Edit::read(document, request.selection.ordinal).map_err(conflict)?;
    if edit.binding().map_err(conflict)? != request.revision {
        return Err(GuiError::conflict(
            "Prompt config changed. Keep your draft and reload before saving.",
        ));
    }
    edit.commit(&request.name, &request.body, &request.tags, || {
        verify_context(gui, &[&context])
    })
    .map_err(conflict)?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportDestination {
    Global,
    Project {
        project_id: String,
    },
    Worktree {
        project_id: String,
        feature_id: String,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ExportRequest {
    pub scope: LibraryScope,
    pub entry_key: String,
    pub destination: ExportDestination,
}
#[derive(Serialize)]
pub struct ExportPreview {
    pub revision: String,
    pub name: String,
    pub body: String,
    pub description: Option<String>,
    pub configured_fields: usize,
    pub destination_path: String,
    pub source_path: Option<String>,
    pub replacing: bool,
    pub no_op: bool,
}
struct Prepared {
    plan: Export,
    source_context: WorkspaceContext,
    destination_context: WorkspaceContext,
    user_source: Option<(String, String)>,
    revision: String,
}
fn prepare(gui: &mut GuiHandle, request: &ExportRequest) -> GuiResult<Prepared> {
    let entry = selected(gui, &request.scope, &request.entry_key)?;
    let source_context = context(gui, &request.scope)?;
    let (source, user_source) = if entry.source == "User" {
        let snapshot = gui
            .db()?
            .prompt_template_snapshot(&entry.template.id)?
            .ok_or_else(|| GuiError::conflict("The User template was deleted"))?;
        if snapshot.template != entry.template {
            return Err(GuiError::conflict(
                "The User template changed. Refresh before exporting.",
            ));
        }
        let user_source = Some((snapshot.template.id.clone(), snapshot.revision.clone()));
        (
            ExportSource {
                raw: snapshot.raw_template()?,
                revision: snapshot.revision,
                file: None,
            },
            user_source,
        )
    } else {
        let edit = Edit::read(
            Document::read(location(&source_context, entry.source).map_err(conflict)?)
                .map_err(conflict)?,
            entry.source_index,
        )
        .map_err(conflict)?;
        matches_entry(&edit, &entry).map_err(conflict)?;
        (ExportSource::from_file(edit).map_err(conflict)?, None)
    };
    let destination_scope = match &request.destination {
        ExportDestination::Global => LibraryScope::Global,
        ExportDestination::Project { project_id } => LibraryScope::Project {
            project_id: project_id.clone(),
        },
        ExportDestination::Worktree {
            project_id,
            feature_id,
        } => LibraryScope::Feature {
            project_id: project_id.clone(),
            feature_id: feature_id.clone(),
        },
    };
    let destination_context = context(gui, &destination_scope)?;
    let dest_location = match request.destination {
        ExportDestination::Global => global(),
        ExportDestination::Project { .. } => {
            destination_context.local_project().map_err(conflict)?
        }
        ExportDestination::Worktree { .. } => {
            destination_context.local_worktree().map_err(conflict)?
        }
    };
    let plan = Export::prepare(source, Document::read(dest_location).map_err(conflict)?)
        .map_err(conflict)?;
    let revision = files::revision(&(
        plan.binding().map_err(conflict)?,
        &source_context.binding,
        &destination_context.binding,
        request,
    ))
    .map_err(conflict)?;
    Ok(Prepared {
        plan,
        source_context,
        destination_context,
        user_source,
        revision,
    })
}
pub fn export_preview(gui: &mut GuiHandle, request: ExportRequest) -> GuiResult<ExportPreview> {
    let prepared = prepare(gui, &request)?;
    let template = files::validate_template(&prepared.plan.source.raw).map_err(conflict)?;
    Ok(ExportPreview {
        revision: prepared.revision,
        name: template.name,
        body: template.body,
        description: template.description,
        configured_fields: template.placeholders.len(),
        destination_path: prepared
            .plan
            .destination
            .location
            .canonical()
            .display()
            .to_string(),
        source_path: prepared
            .plan
            .source
            .file
            .as_ref()
            .map(|edit| edit.document.active_path.display().to_string()),
        replacing: prepared.plan.replacing,
        no_op: prepared.plan.no_op,
    })
}
#[derive(Deserialize)]
pub struct ExportCommit {
    pub request: ExportRequest,
    pub revision: String,
    pub replace_confirmed: bool,
}
pub fn export_commit(gui: &mut GuiHandle, request: ExportCommit) -> GuiResult<()> {
    let prepared = prepare(gui, &request.request)?;
    if prepared.revision != request.revision {
        return Err(GuiError::conflict(
            "The source, destination or checkout changed. Refresh the export preview before confirming.",
        ));
    }
    prepared
        .plan
        .commit(request.replace_confirmed, || {
            verify_context(
                gui,
                &[&prepared.source_context, &prepared.destination_context],
            )?;
            if let Some((id, revision)) = &prepared.user_source {
                let current = gui
                    .db()
                    .map_err(|error| anyhow::anyhow!(error.message))?
                    .prompt_template_snapshot(id)?
                    .context("The User source was deleted")?;
                ensure!(
                    &current.revision == revision,
                    "The User source changed before export. Refresh the preview."
                );
            }
            Ok(())
        })
        .map_err(conflict)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn project_path(gui: &mut GuiHandle) -> std::path::PathBuf {
        gui.app_for_workflow().store.projects[0]
            .repo
            .join("amf.json")
    }
    fn seed(gui: &mut GuiHandle) -> Value {
        let value = json!({"opaque_root":{"keep":7},"prompt_templates":[{"name":"Source","body":"Use {{area}}","description":"Keep description","tags":["bug"],"opaque":{"version":5},"placeholders":[{"key":"unused","kind":"text","default":"saved","opaque":true}]},{"name":"Sibling","body":"untouched"}]});
        std::fs::write(project_path(gui), serde_json::to_vec(&value).unwrap()).unwrap();
        value
    }
    fn key(gui: &mut GuiHandle, scope: &LibraryScope, name: &str) -> String {
        super::super::load(gui, scope, "")
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.source == "Project" && e.name == name)
            .unwrap()
            .key
    }
    fn draft(editor: DeclarativeEditor) -> SaveRequest {
        SaveRequest {
            selection: editor.selection,
            context_revision: editor.context_revision,
            revision: editor.revision,
            name: "Renamed".into(),
            body: "Edited".into(),
            tags: "bug #BUG new".into(),
        }
    }
    #[test]
    fn desktop_edit_preserves_raw_fields_and_rejects_delayed_source_mutations() {
        let (_dir, mut gui, scope, _) = super::super::tests::fixture();
        let original = seed(&mut gui);
        let selected = key(&mut gui, &scope, "Source");
        let editor = edit(&mut gui, scope.clone(), &selected).unwrap();
        assert_eq!(editor.configured_fields, 1);
        let mut external = original.clone();
        external["prompt_templates"][0]["placeholders"][0]["default"] = "changed".into();
        std::fs::write(
            project_path(&mut gui),
            serde_json::to_vec(&external).unwrap(),
        )
        .unwrap();
        assert!(save(&mut gui, draft(editor)).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(project_path(&mut gui)).unwrap())
                .unwrap(),
            external
        );
        let selected = key(&mut gui, &scope, "Source");
        let editor = edit(&mut gui, scope.clone(), &selected).unwrap();
        save(&mut gui, draft(editor)).unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(project_path(&mut gui)).unwrap()).unwrap();
        assert_eq!(value["opaque_root"], original["opaque_root"]);
        assert_eq!(
            value["prompt_templates"][0]["opaque"],
            original["prompt_templates"][0]["opaque"]
        );
        assert_eq!(
            value["prompt_templates"][0]["placeholders"],
            external["prompt_templates"][0]["placeholders"]
        );
        assert_eq!(
            value["prompt_templates"][1],
            original["prompt_templates"][1]
        );
    }
    #[test]
    fn reload_refuses_source_reorder_and_context_retarget_keeps_files_unchanged() {
        let (dir, mut gui, scope, _) = super::super::tests::fixture();
        let original = seed(&mut gui);
        let selected = key(&mut gui, &scope, "Source");
        let editor = edit(&mut gui, scope.clone(), &selected).unwrap();
        let mut reordered = original.clone();
        reordered["prompt_templates"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        std::fs::write(
            project_path(&mut gui),
            serde_json::to_vec(&reordered).unwrap(),
        )
        .unwrap();
        assert!(
            reload(
                &mut gui,
                ReloadRequest {
                    selection: editor.selection.clone(),
                    context_revision: editor.context_revision.clone(),
                    original_name: editor.original_name.clone(),
                    original_id: editor.original_id.clone()
                }
            )
            .is_err()
        );
        let mut store = gui.db().unwrap().load_store().unwrap();
        store.projects[0].repo = dir.path().join("replacement");
        std::fs::create_dir(&store.projects[0].repo).unwrap();
        gui.db().unwrap().save_store(&store).unwrap();
        assert!(save(&mut gui, draft(editor)).is_err());
        assert!(!store.projects[0].repo.join("amf.json").exists());
    }
    #[test]
    fn export_preview_confirms_replacement_and_binds_destination_and_user_raw_source() {
        let (_dir, mut gui, scope, target) = super::super::tests::fixture();
        seed(&mut gui);
        let mut user =
            crate::prompt_library::PromptTemplate::new("Source".into(), "User body".into());
        user.description = Some("User metadata".into());
        gui.db().unwrap().insert_prompt_template(&user).unwrap();
        let user_key = super::super::load(&mut gui, &scope, "")
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.source == "User")
            .unwrap()
            .key;
        let request = ExportRequest {
            scope: scope.clone(),
            entry_key: user_key,
            destination: ExportDestination::Project {
                project_id: target.project_id,
            },
        };
        let preview = export_preview(&mut gui, request.clone()).unwrap();
        assert!(preview.replacing);
        assert!(!preview.no_op);
        assert!(
            export_commit(
                &mut gui,
                ExportCommit {
                    request: request.clone(),
                    revision: preview.revision.clone(),
                    replace_confirmed: false
                }
            )
            .is_err()
        );
        user.body = "Changed outside".into();
        gui.db().unwrap().update_prompt_template(&user).unwrap();
        assert!(
            export_commit(
                &mut gui,
                ExportCommit {
                    request: request.clone(),
                    revision: preview.revision,
                    replace_confirmed: true
                }
            )
            .is_err()
        );
        let key = super::super::load(&mut gui, &scope, "")
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.source == "User")
            .unwrap()
            .key;
        let request = ExportRequest {
            entry_key: key,
            ..request
        };
        let preview = export_preview(&mut gui, request.clone()).unwrap();
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(project_path(&mut gui)).unwrap()).unwrap();
        value["external"] = true.into();
        std::fs::write(project_path(&mut gui), serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            export_commit(
                &mut gui,
                ExportCommit {
                    request: request.clone(),
                    revision: preview.revision,
                    replace_confirmed: true
                }
            )
            .is_err()
        );
        let preview = export_preview(&mut gui, request.clone()).unwrap();
        export_commit(
            &mut gui,
            ExportCommit {
                request,
                revision: preview.revision,
                replace_confirmed: true,
            },
        )
        .unwrap();
        let value: Value =
            serde_json::from_slice(&std::fs::read(project_path(&mut gui)).unwrap()).unwrap();
        assert_eq!(value["prompt_templates"][0]["body"], "Changed outside");
        assert_eq!(value["prompt_templates"][0]["opaque"], json!({"version":5}));
        assert_eq!(value["prompt_templates"][1]["name"], "Sibling");
    }
    #[test]
    fn same_source_export_is_explicit_noop_and_deleted_destination_is_refused() {
        let (_dir, mut gui, scope, target) = super::super::tests::fixture();
        seed(&mut gui);
        let selected = key(&mut gui, &scope, "Source");
        let request = ExportRequest {
            scope: scope.clone(),
            entry_key: selected,
            destination: ExportDestination::Project {
                project_id: target.project_id.clone(),
            },
        };
        let before = std::fs::read(project_path(&mut gui)).unwrap();
        let preview = export_preview(&mut gui, request.clone()).unwrap();
        assert!(preview.no_op);
        export_commit(
            &mut gui,
            ExportCommit {
                request,
                revision: preview.revision,
                replace_confirmed: false,
            },
        )
        .unwrap();
        assert_eq!(std::fs::read(project_path(&mut gui)).unwrap(), before);
        let selected = key(&mut gui, &scope, "Source");
        let request = ExportRequest {
            scope,
            entry_key: selected,
            destination: ExportDestination::Worktree {
                project_id: target.project_id,
                feature_id: target.feature_id,
            },
        };
        let preview = export_preview(&mut gui, request.clone()).unwrap();
        let workdir = gui.app_for_workflow().store.projects[0].features[0]
            .workdir
            .clone();
        std::fs::remove_dir(&workdir).unwrap();
        assert!(
            export_commit(
                &mut gui,
                ExportCommit {
                    request,
                    revision: preview.revision,
                    replace_confirmed: false
                }
            )
            .is_err()
        );
        assert!(!workdir.exists());
    }
}
