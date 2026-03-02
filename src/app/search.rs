use super::*;
use super::util::shorten_path;

impl App {
    pub fn start_search(&mut self) {
        self.mode = AppMode::Searching(SearchState {
            query: String::new(),
            matches: Vec::new(),
            selected_match: 0,
        });
        self.message = None;
    }

    pub fn perform_search(&mut self) {
        let query = match &self.mode {
            AppMode::Searching(state) => state.query.to_lowercase(),
            _ => return,
        };

        let mut matches = Vec::new();

        for (pi, project) in self.store.projects.iter().enumerate() {
            if project.name.to_lowercase().contains(&query) {
                matches.push(SearchMatch {
                    item: VisibleItem::Project(pi),
                    label: project.name.clone(),
                    context: shorten_path(&project.repo),
                });
            }

            for (fi, feature) in project.features.iter().enumerate() {
                if feature.name.to_lowercase().contains(&query) {
                    matches.push(SearchMatch {
                        item: VisibleItem::Feature(pi, fi),
                        label: feature.name.clone(),
                        context: format!("{} / {}", project.name, shorten_path(&feature.workdir)),
                    });
                }

                for (si, session) in feature.sessions.iter().enumerate() {
                    if session.label.to_lowercase().contains(&query) {
                        matches.push(SearchMatch {
                            item: VisibleItem::Session(pi, fi, si),
                            label: session.label.clone(),
                            context: format!("{} / {}", project.name, feature.name),
                        });
                    }
                }
            }
        }

        if let AppMode::Searching(state) = &mut self.mode {
            state.matches = matches;
            if state.selected_match >= state.matches.len() {
                state.selected_match = 0;
            }
        }
    }

    pub fn jump_to_search_match(&mut self) {
        let match_item = match &self.mode {
            AppMode::Searching(state) => {
                state.matches.get(state.selected_match).cloned()
            }
            _ => return,
        };

        if let Some(m) = match_item {
            self.selection = match m.item {
                VisibleItem::Project(pi) => Selection::Project(pi),
                VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
            };
            self.mode = AppMode::Normal;
            self.message = None;
        }
    }

    pub fn cancel_search(&mut self) {
        self.mode = AppMode::Normal;
        self.message = None;
    }

    pub fn select_next_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode
            && !state.matches.is_empty()
        {
            state.selected_match = (state.selected_match + 1) % state.matches.len();
        }
    }

    pub fn select_prev_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode
            && !state.matches.is_empty()
        {
            state.selected_match = if state.selected_match == 0 {
                state.matches.len() - 1
            } else {
                state.selected_match - 1
            };
        }
    }
}
