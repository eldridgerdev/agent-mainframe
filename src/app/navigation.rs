use super::*;

impl App {
    pub fn visible_items(&self) -> Vec<VisibleItem> {
        let mut items = Vec::new();
        for (pi, project) in self.store.projects.iter().enumerate() {
            items.push(VisibleItem::Project(pi));
            if !project.collapsed {
                let mut feature_indices: Vec<usize> = (0..project.features.len()).collect();
                feature_indices.sort_by(|&a, &b| {
                    project.features[b]
                        .created_at
                        .cmp(&project.features[a].created_at)
                });
                for fi in feature_indices {
                    let feature = &project.features[fi];
                    items.push(VisibleItem::Feature(pi, fi));
                    if !feature.collapsed {
                        for (si, session) in feature.sessions.iter().enumerate() {
                            if self.session_matches_filter(session) {
                                items.push(VisibleItem::Session(pi, fi, si));
                            }
                        }
                    }
                }
            }
        }
        items
    }

    fn session_matches_filter(&self, session: &FeatureSession) -> bool {
        use crate::project::SessionKind;
        match &self.session_filter {
            SessionFilter::All => true,
            SessionFilter::Claude => session.kind == SessionKind::Claude,
            SessionFilter::Opencode => session.kind == SessionKind::Opencode,
            SessionFilter::Codex => session.kind == SessionKind::Codex,
            SessionFilter::Terminal => session.kind == SessionKind::Terminal,
            SessionFilter::Nvim => session.kind == SessionKind::Nvim && session.label != "Memo",
            SessionFilter::Memo => session.kind == SessionKind::Nvim && session.label == "Memo",
            SessionFilter::Vscode => session.kind == SessionKind::Vscode,
        }
    }

    pub(crate) fn selection_index(&self) -> Option<usize> {
        let items = self.visible_items();
        items.iter().position(|item| match (&self.selection, item) {
            (Selection::Project(a), VisibleItem::Project(b)) => a == b,
            (Selection::Feature(a1, a2), VisibleItem::Feature(b1, b2)) => a1 == b1 && a2 == b2,
            (Selection::Session(a1, a2, a3), VisibleItem::Session(b1, b2, b3)) => {
                a1 == b1 && a2 == b2 && a3 == b3
            }
            _ => false,
        })
    }

    pub fn select_next(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let next = (current + 1) % items.len();
        self.selection = match items[next] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
            VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
        };
        self.reload_extension_config();
    }

    pub fn select_prev(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let prev = if current == 0 {
            items.len() - 1
        } else {
            current - 1
        };
        self.selection = match items[prev] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
            VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
        };
        self.reload_extension_config();
    }

    pub fn ensure_selection_visible(&mut self, visible_height: usize) {
        let items = self.visible_items();
        if items.is_empty() || visible_height == 0 {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        if current < self.scroll_offset {
            self.scroll_offset = current;
        } else if current >= self.scroll_offset + visible_height {
            self.scroll_offset = current - visible_height + 1;
        }
    }

    pub fn select_next_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = (current + offset) % items.len();
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                    _ => unreachable!(),
                };
                self.reload_extension_config();
                return;
            }
        }
    }

    pub fn select_prev_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = if current >= offset {
                current - offset
            } else {
                items.len() - (offset - current)
            };
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                    _ => unreachable!(),
                };
                self.reload_extension_config();
                return;
            }
        }
    }

    pub fn selected_project(&self) -> Option<&Project> {
        match &self.selection {
            Selection::Project(pi) | Selection::Feature(pi, _) | Selection::Session(pi, _, _) => {
                self.store.projects.get(*pi)
            }
        }
    }

    pub fn selected_feature(&self) -> Option<(&Project, &Feature)> {
        match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                Some((project, feature))
            }
            _ => None,
        }
    }

    pub fn selected_session(&self) -> Option<(&Project, &Feature, &FeatureSession)> {
        match &self.selection {
            Selection::Session(pi, fi, si) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                let session = feature.sessions.get(*si)?;
                Some((project, feature, session))
            }
            _ => None,
        }
    }
}
