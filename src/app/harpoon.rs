use anyhow::Result;

use super::*;
use crate::project::{SessionBookmark, SessionKind};

const MAX_SESSION_BOOKMARKS: usize = 9;

impl App {
    pub fn open_bookmark_picker(
        &mut self,
        from_view: Option<ViewState>,
    ) {
        self.mode =
            AppMode::BookmarkPicker(BookmarkPickerState {
                selected: 0,
                from_view,
            });
    }

    pub fn bookmark_current_session(&mut self) -> Result<()> {
        let Some((pi, fi, si)) = self.current_session_indices() else {
            self.message = Some(
                "Select or view a session to bookmark".to_string(),
            );
            return Ok(());
        };

        let bookmark =
            self.make_bookmark_for_indices(pi, fi, si);

        if let Some(pos) =
            self.bookmark_position(&bookmark)
        {
            self.message = Some(format!(
                "Session already bookmarked in slot {}",
                pos + 1
            ));
            return Ok(());
        }

        let mut evicted = false;
        if self.store.session_bookmarks.len()
            >= MAX_SESSION_BOOKMARKS
        {
            self.store.session_bookmarks.remove(0);
            evicted = true;
        }

        self.store.session_bookmarks.push(bookmark);
        self.save()?;
        let slot = self.store.session_bookmarks.len();
        self.message = Some(if evicted {
            format!(
                "Bookmarked in slot {} (oldest slot evicted)",
                slot
            )
        } else {
            format!("Bookmarked in slot {}", slot)
        });
        Ok(())
    }

    pub fn unbookmark_current_session(
        &mut self,
    ) -> Result<()> {
        let Some((pi, fi, si)) = self.current_session_indices() else {
            self.message = Some(
                "Select or view a session to unbookmark".to_string(),
            );
            return Ok(());
        };

        let bookmark =
            self.make_bookmark_for_indices(pi, fi, si);
        if let Some(pos) =
            self.bookmark_position(&bookmark)
        {
            self.store.session_bookmarks.remove(pos);
            self.save()?;
            self.message = Some(format!(
                "Removed bookmark from slot {}",
                pos + 1
            ));
        } else {
            self.message =
                Some("Session is not bookmarked".to_string());
        }
        Ok(())
    }

    pub fn jump_to_bookmark(
        &mut self,
        slot: usize,
    ) -> Result<()> {
        if slot == 0 || slot > MAX_SESSION_BOOKMARKS {
            self.message = Some(
                "Bookmark slot must be 1-9".to_string(),
            );
            return Ok(());
        }

        let idx = slot - 1;
        let Some(bookmark) =
            self.store.session_bookmarks.get(idx).cloned()
        else {
            self.message = Some(format!(
                "Bookmark slot {} is empty",
                slot
            ));
            return Ok(());
        };

        let Some((pi, fi, si)) =
            self.resolve_bookmark_indices(&bookmark)
        else {
            self.store.session_bookmarks.remove(idx);
            self.save()?;
            self.message = Some(format!(
                "Bookmark slot {} was stale and got removed",
                slot
            ));
            return Ok(());
        };

        self.selection = Selection::Session(pi, fi, si);
        self.enter_view()?;
        self.message =
            Some(format!("Jumped to bookmark {}", slot));
        Ok(())
    }

    pub fn bookmark_status_labels(&self) -> Vec<String> {
        (0..MAX_SESSION_BOOKMARKS)
            .map(|idx| {
                let slot = idx + 1;
                let Some(bookmark) =
                    self.store.session_bookmarks.get(idx)
                else {
                    return format!("{}:-", slot);
                };

                if let Some((pi, fi, si)) =
                    self.resolve_bookmark_indices(bookmark)
                {
                    let project =
                        &self.store.projects[pi];
                    let feature =
                        &project.features[fi];
                    let session =
                        &feature.sessions[si];
                    format!(
                        "{}:{}/{}",
                        slot, feature.name, session.label
                    )
                } else {
                    format!("{}:?", slot)
                }
            })
            .collect()
    }

    pub fn remove_bookmark_slot(
        &mut self,
        slot: usize,
    ) -> Result<()> {
        if slot == 0 || slot > MAX_SESSION_BOOKMARKS {
            self.message = Some(
                "Bookmark slot must be 1-9".to_string(),
            );
            return Ok(());
        }
        let idx = slot - 1;
        if idx < self.store.session_bookmarks.len() {
            self.store.session_bookmarks.remove(idx);
            self.save()?;
            self.message = Some(format!(
                "Cleared bookmark slot {}",
                slot
            ));
        } else {
            self.message = Some(format!(
                "Bookmark slot {} is already empty",
                slot
            ));
        }
        Ok(())
    }

    pub fn bookmark_picker_rows(&self) -> Vec<String> {
        (0..self.store.session_bookmarks.len())
            .map(|idx| {
                let slot = idx + 1;
                let bookmark =
                    &self.store.session_bookmarks[idx];

                if let Some((pi, fi, si)) =
                    self.resolve_bookmark_indices(bookmark)
                {
                    let project =
                        &self.store.projects[pi];
                    let feature =
                        &project.features[fi];
                    let session =
                        &feature.sessions[si];
                    format!(
                        "{}  {}/{} ({})",
                        slot,
                        project.name,
                        feature.name,
                        session.label
                    )
                } else {
                    format!("{}  [stale]", slot)
                }
            })
            .collect()
    }

    fn current_session_indices(
        &self,
    ) -> Option<(usize, usize, usize)> {
        if let Some(pos) =
            self.current_view_session_indices()
        {
            return Some(pos);
        }

        match self.selection {
            Selection::Session(pi, fi, si) => Some((pi, fi, si)),
            Selection::Feature(pi, fi) => {
                let feature = self
                    .store
                    .projects
                    .get(pi)?
                    .features
                    .get(fi)?;
                let si = feature
                    .sessions
                    .iter()
                    .position(|s| {
                        s.kind == SessionKind::Claude
                    })
                    .unwrap_or(0);
                feature.sessions.get(si)?;
                Some((pi, fi, si))
            }
            Selection::Project(_) => None,
        }
    }

    fn current_view_session_indices(
        &self,
    ) -> Option<(usize, usize, usize)> {
        let AppMode::Viewing(view) = &self.mode else {
            return None;
        };

        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == view.project_name)?;
        let fi = self.store.projects[pi]
            .features
            .iter()
            .position(|f| f.name == view.feature_name)?;
        let si = self.store.projects[pi].features[fi]
            .sessions
            .iter()
            .position(|s| s.tmux_window == view.window)?;
        Some((pi, fi, si))
    }

    fn make_bookmark_for_indices(
        &self,
        pi: usize,
        fi: usize,
        si: usize,
    ) -> SessionBookmark {
        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let session = &feature.sessions[si];
        SessionBookmark {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            session_id: session.id.clone(),
        }
    }

    fn bookmark_position(
        &self,
        target: &SessionBookmark,
    ) -> Option<usize> {
        self.store
            .session_bookmarks
            .iter()
            .position(|bookmark| {
                bookmark.project_id == target.project_id
                    && bookmark.feature_id == target.feature_id
                    && bookmark.session_id == target.session_id
            })
    }

    fn resolve_bookmark_indices(
        &self,
        bookmark: &SessionBookmark,
    ) -> Option<(usize, usize, usize)> {
        self.store
            .projects
            .iter()
            .enumerate()
            .find(|(_, project)| {
                project.id == bookmark.project_id
            })
            .and_then(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .find(|(_, feature)| {
                        feature.id
                            == bookmark.feature_id
                    })
                    .and_then(|(fi, feature)| {
                        feature
                            .sessions
                            .iter()
                            .enumerate()
                            .find(|(_, session)| {
                                session.id
                                    == bookmark.session_id
                            })
                            .map(|(si, _)| (pi, fi, si))
                    })
            })
    }
}
