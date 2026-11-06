use std::sync::Mutex;

use super::super::{FeatureTarget, GuiError, pr_workspace_blocking};

// A single global lock: only one preview/confirmed integration runs at a time,
// because companions may share a source checkout or the GUI handle's Git work.
// Status reports `running` for any key so every dialog shows the real state.
static COMPANION_OPERATIONS: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();
static COMPANION_RESULTS: std::sync::OnceLock<
    Mutex<std::collections::HashMap<String, Result<String, GuiError>>>,
> = std::sync::OnceLock::new();
#[derive(serde::Serialize)]
pub(crate) struct CompanionOperationStatus {
    running: bool,
    result: Option<String>,
    error: Option<GuiError>,
}

#[tauri::command]
pub(crate) fn companion_integration_status(target: FeatureTarget) -> CompanionOperationStatus {
    let key = format!("{}:{}", target.project_id, target.feature_id);
    let operations = COMPANION_OPERATIONS
        .get_or_init(Default::default)
        .lock()
        .expect("operation mutex poisoned");
    let running = !operations.is_empty();
    let mut results = COMPANION_RESULTS
        .get_or_init(Default::default)
        .lock()
        .expect("result mutex poisoned");
    // Keep the operation lock until the outcome is read so a new operation
    // cannot start between checking `running` and consuming a finished result.
    // Only report finished outcomes while idle, and report each one once.
    let result = if running { None } else { results.remove(&key) };
    drop(results);
    drop(operations);
    CompanionOperationStatus {
        running,
        result: result.as_ref().and_then(|r| r.as_ref().ok().cloned()),
        error: result.and_then(Result::err),
    }
}
struct CompanionOperation(String);
impl CompanionOperation {
    fn acquire(target: &FeatureTarget) -> Result<Self, GuiError> {
        let key = format!("{}:{}", target.project_id, target.feature_id);
        let mut operations = COMPANION_OPERATIONS
            .get_or_init(Default::default)
            .lock()
            .expect("operation mutex poisoned");
        if !operations.is_empty() {
            return Err(GuiError {
                github: None,
                kind: agent_mainframe::gui_contract::GuiErrorKind::Conflict,
                message: "Another companion integration operation is already running".into(),
            });
        }
        operations.insert(key.clone());
        // Starting new work invalidates prior outcomes for every target, since
        // all dialogs share the running state.
        COMPANION_RESULTS
            .get_or_init(Default::default)
            .lock()
            .expect("result mutex poisoned")
            .clear();
        Ok(Self(key))
    }
}
impl Drop for CompanionOperation {
    fn drop(&mut self) {
        COMPANION_OPERATIONS
            .get_or_init(Default::default)
            .lock()
            .expect("operation mutex poisoned")
            .remove(&self.0);
    }
}

#[tauri::command]
pub(crate) async fn companion_integration_preview(
    app: tauri::AppHandle,
    target: FeatureTarget,
    choice: agent_mainframe::gui_companion_integration::IntegrationChoice,
) -> Result<agent_mainframe::gui_companion_integration::IntegrationPlan, GuiError> {
    let operation = CompanionOperation::acquire(&target)?;
    pr_workspace_blocking(app, move |gui| {
        let _operation = operation;
        let binding = agent_mainframe::gui_companion_integration::bind(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &target,
        )?;
        let plan = agent_mainframe::gui_companion_integration::plan(binding.clone(), choice)
            .map_err(GuiError::from)?;
        let current = agent_mainframe::gui_companion_integration::bind(
            &mut gui.lock().expect("gui handle mutex poisoned"),
            &target,
        )?;
        if current != binding {
            return Err(GuiError {
                github: None,
                kind: agent_mainframe::gui_contract::GuiErrorKind::Conflict,
                message: "Companion or source changed; refresh the preview".into(),
            });
        }
        Ok(plan)
    })
    .await?
}

#[tauri::command]
pub(crate) async fn companion_integration_confirm(
    app: tauri::AppHandle,
    target: FeatureTarget,
    choice: agent_mainframe::gui_companion_integration::IntegrationChoice,
    revision: String,
) -> Result<String, GuiError> {
    let operation = CompanionOperation::acquire(&target)?;
    let result_key = operation.0.clone();
    let returned = pr_workspace_blocking(app, move |gui| {
        let _operation = operation;
        let outcome = (|| {
            let binding = agent_mainframe::gui_companion_integration::bind(
                &mut gui.lock().expect("gui handle mutex poisoned"),
                &target,
            )?;
            let plan = agent_mainframe::gui_companion_integration::plan(binding.clone(), choice)
                .map_err(GuiError::from)?;
            if plan.revision != revision {
                return Err(GuiError {
                    github: None,
                    kind: agent_mainframe::gui_contract::GuiErrorKind::Conflict,
                    message: "Integration changed; refresh the preview".into(),
                });
            }
            let current = agent_mainframe::gui_companion_integration::bind(
                &mut gui.lock().expect("gui handle mutex poisoned"),
                &target,
            )?;
            if current != binding {
                return Err(GuiError {
                    github: None,
                    kind: agent_mainframe::gui_contract::GuiErrorKind::Conflict,
                    message: "Companion or source changed; refresh the preview".into(),
                });
            }
            agent_mainframe::gui_companion_integration::execute(&plan).map_err(GuiError::from)
        })();
        COMPANION_RESULTS
            .get_or_init(Default::default)
            .lock()
            .expect("result mutex poisoned")
            .insert(_operation.0.clone(), outcome.clone());
        outcome
    })
    .await;
    // The caller receives the outcome directly; drop the polled copy so a
    // later dialog does not present it as current.
    COMPANION_RESULTS
        .get_or_init(Default::default)
        .lock()
        .expect("result mutex poisoned")
        .remove(&result_key);
    returned?
}

#[cfg(test)]
mod tests {
    #[test]
    fn companion_operations_are_exclusive_and_report_a_triage_result_once() {
        use super::{
            COMPANION_RESULTS, CompanionOperation, FeatureTarget, companion_integration_status,
        };
        let target = FeatureTarget {
            project_id: "project".into(),
            feature_id: "triage-companion".into(),
        };
        let other = FeatureTarget {
            project_id: "project".into(),
            feature_id: "other".into(),
        };
        let held = CompanionOperation::acquire(&target).expect("first operation starts");
        assert!(companion_integration_status(target.clone()).running);
        // Duplicate and cross-feature starts are refused while one is running.
        assert!(CompanionOperation::acquire(&target).is_err());
        assert!(CompanionOperation::acquire(&other).is_err());
        COMPANION_RESULTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(
                held.0.clone(),
                Ok("Pushed 1 companion commit(s) to origin/pr for PR #7".into()),
            );
        // An outcome can be stored just before the operation guard is dropped.
        // Polling must keep it hidden until the operation finishes.
        let finishing = companion_integration_status(target.clone());
        assert!(finishing.running);
        assert!(finishing.result.is_none());
        assert!(finishing.error.is_none());
        drop(held);
        let done = companion_integration_status(target.clone());
        assert!(!done.running);
        assert!(done.result.unwrap().contains("PR #7"));
        // Reported once: reopening later cannot show it as current.
        assert!(
            companion_integration_status(target.clone())
                .result
                .is_none()
        );

        // A later operation invalidates both success and failure outcomes,
        // including those belonging to another target.
        {
            let mut results = COMPANION_RESULTS
                .get_or_init(Default::default)
                .lock()
                .unwrap();
            results.insert(target_key(&target), Ok("Earlier success".into()));
            results.insert(
                target_key(&other),
                Err(super::GuiError {
                    github: None,
                    kind: agent_mainframe::gui_contract::GuiErrorKind::Conflict,
                    message: "Earlier failure".into(),
                }),
            );
        }
        let held = CompanionOperation::acquire(&other).expect("released after drop");
        for polled in [&target, &other] {
            let status = companion_integration_status(polled.clone());
            assert!(status.running);
            assert!(status.result.is_none());
            assert!(status.error.is_none());
        }
        drop(held);
        for polled in [target, other] {
            let status = companion_integration_status(polled);
            assert!(!status.running);
            assert!(status.result.is_none());
            assert!(status.error.is_none());
        }
    }

    fn target_key(target: &super::FeatureTarget) -> String {
        format!("{}:{}", target.project_id, target.feature_id)
    }
}
