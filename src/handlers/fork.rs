use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, ForkFeatureStep};
use crate::project::AgentKind;

pub fn handle_fork_feature_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    let step = match &app.mode {
        AppMode::ForkingFeature(s) => s.step.clone(),
        _ => return Ok(()),
    };

    match step {
        ForkFeatureStep::Branch => match key {
            KeyCode::Esc => {
                app.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                {
                    if state.new_branch.is_empty() {
                        app.message = Some(
                            "Branch name cannot be empty"
                                .into(),
                        );
                    } else {
                        state.step = ForkFeatureStep::Agent;
                    }
                }
            }
            KeyCode::Backspace => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                {
                    state.new_branch.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                {
                    state.new_branch.push(c);
                }
            }
            _ => {}
        },
        ForkFeatureStep::Agent => match key {
            KeyCode::Esc => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                {
                    state.step = ForkFeatureStep::Branch;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                    && state.agent_index > 0
                {
                    state.agent_index -= 1;
                    state.agent =
                        AgentKind::ALL[state.agent_index]
                            .clone();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                    && state.agent_index + 1
                        < AgentKind::ALL.len()
                {
                    state.agent_index += 1;
                    state.agent =
                        AgentKind::ALL[state.agent_index]
                            .clone();
                }
            }
            KeyCode::Tab => {
                if let AppMode::ForkingFeature(state) =
                    &mut app.mode
                {
                    state.include_context =
                        !state.include_context;
                }
            }
            KeyCode::Enter => {
                app.create_forked_feature()?;
            }
            _ => {}
        },
    }

    Ok(())
}
