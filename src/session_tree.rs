use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session::SessionSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeState {
    summaries: Vec<SessionSummary>,
    pub query: String,
    pub selected: usize,
    pub show_paths: bool,
    pub newest_first: bool,
    pub prompt: Option<TreePrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTreeAction {
    Continue,
    Selected(String),
    Cancelled,
    RenameRequested { session_id: String, new_id: String },
    DeleteRequested { session_id: String },
    ForkRequested { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreePrompt {
    Rename { session_id: String, input: String },
    DeleteConfirm { session_id: String },
}

#[derive(Debug, Clone)]
pub struct TreeRow<'a> {
    pub summary: &'a SessionSummary,
    pub prefix: String,
}

impl SessionTreeState {
    pub fn new(summaries: Vec<SessionSummary>) -> Self {
        Self {
            summaries,
            query: String::new(),
            selected: 0,
            show_paths: true,
            newest_first: true,
            prompt: None,
        }
    }

    pub fn set_summaries(&mut self, summaries: Vec<SessionSummary>) {
        self.summaries = summaries;
        self.clamp_selected();
    }

    pub fn filtered_summaries(&self) -> Vec<&SessionSummary> {
        self.tree_rows()
            .into_iter()
            .map(|row| row.summary)
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SessionTreeAction {
        if let Some(prompt) = &mut self.prompt {
            return match prompt {
                TreePrompt::Rename { session_id, input } => match key.code {
                    KeyCode::Esc => {
                        self.prompt = None;
                        SessionTreeAction::Continue
                    }
                    KeyCode::Enter => {
                        let session_id = session_id.clone();
                        let new_id = input.trim().to_owned();
                        self.prompt = None;
                        SessionTreeAction::RenameRequested { session_id, new_id }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        SessionTreeAction::Continue
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        input.push(character);
                        SessionTreeAction::Continue
                    }
                    _ => SessionTreeAction::Continue,
                },
                TreePrompt::DeleteConfirm { session_id } => match key.code {
                    KeyCode::Esc => {
                        self.prompt = None;
                        SessionTreeAction::Continue
                    }
                    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let session_id = session_id.clone();
                        self.prompt = None;
                        SessionTreeAction::DeleteRequested { session_id }
                    }
                    _ => SessionTreeAction::Continue,
                },
            };
        }

        match key.code {
            KeyCode::Esc => SessionTreeAction::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                SessionTreeAction::Cancelled
            }
            KeyCode::Enter => self
                .selected_session()
                .map(|session| SessionTreeAction::Selected(session.session.id.clone()))
                .unwrap_or(SessionTreeAction::Cancelled),
            KeyCode::Backspace => {
                self.query.pop();
                self.clamp_selected();
                SessionTreeAction::Continue
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                SessionTreeAction::Continue
            }
            KeyCode::Down => {
                let len = self.filtered_summaries().len();
                if self.selected + 1 < len {
                    self.selected += 1;
                }
                SessionTreeAction::Continue
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(10);
                SessionTreeAction::Continue
            }
            KeyCode::PageDown => {
                let len = self.filtered_summaries().len();
                self.selected = self.selected.saturating_add(10).min(len.saturating_sub(1));
                SessionTreeAction::Continue
            }
            KeyCode::Home => {
                self.selected = 0;
                SessionTreeAction::Continue
            }
            KeyCode::End => {
                let len = self.filtered_summaries().len();
                self.selected = len.saturating_sub(1);
                SessionTreeAction::Continue
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.show_paths = !self.show_paths;
                SessionTreeAction::Continue
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.newest_first = !self.newest_first;
                self.clamp_selected();
                SessionTreeAction::Continue
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => self
                .selected_session()
                .map(|session| SessionTreeAction::ForkRequested {
                    session_id: session.session.id.clone(),
                })
                .unwrap_or(SessionTreeAction::Continue),
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(session) = self.selected_session() {
                    self.prompt = Some(TreePrompt::Rename {
                        session_id: session.session.id.clone(),
                        input: session.session.id.clone(),
                    });
                }
                SessionTreeAction::Continue
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(session) = self.selected_session() {
                    self.prompt = Some(TreePrompt::DeleteConfirm {
                        session_id: session.session.id.clone(),
                    });
                }
                SessionTreeAction::Continue
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                self.clamp_selected();
                SessionTreeAction::Continue
            }
            _ => SessionTreeAction::Continue,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionSummary> {
        self.filtered_summaries().get(self.selected).copied()
    }

    pub fn prompt_label(&self) -> Option<String> {
        match &self.prompt {
            Some(TreePrompt::Rename { input, .. }) => {
                Some(format!("rename: {}  (Enter save, Esc cancel)", input))
            }
            Some(TreePrompt::DeleteConfirm { session_id }) => Some(format!(
                "delete {}?  (Enter/Y confirm, Esc cancel)",
                session_id
            )),
            None => None,
        }
    }

    fn clamp_selected(&mut self) {
        let len = self.filtered_summaries().len();
        self.selected = if len == 0 {
            0
        } else {
            self.selected.min(len.saturating_sub(1))
        };
    }

    fn tree_rows(&self) -> Vec<TreeRow<'_>> {
        let query = self.query.trim().to_lowercase();
        let known_ids = self
            .summaries
            .iter()
            .map(|summary| summary.session.id.as_str())
            .collect::<HashSet<_>>();
        let mut children_by_parent: HashMap<Option<String>, Vec<usize>> = HashMap::new();
        for (index, summary) in self.summaries.iter().enumerate() {
            let key = match summary.parent_session_id.as_deref() {
                Some(parent) if known_ids.contains(parent) => Some(parent.to_owned()),
                _ => None,
            };
            children_by_parent.entry(key).or_default().push(index);
        }

        for summaries in children_by_parent.values_mut() {
            sort_summaries(summaries, &self.summaries, self.newest_first);
        }

        let mut rows = Vec::new();
        if let Some(roots) = children_by_parent.get(&None) {
            for (index, root) in roots.iter().enumerate() {
                let is_last = index + 1 == roots.len();
                if let Some(block) = build_tree_rows(
                    &self.summaries,
                    *root,
                    &children_by_parent,
                    &query,
                    &[],
                    is_last,
                    self.newest_first,
                ) {
                    rows.extend(block);
                }
            }
        }

        rows
    }
}

fn build_tree_rows<'a>(
    summaries: &'a [SessionSummary],
    index: usize,
    children_by_parent: &HashMap<Option<String>, Vec<usize>>,
    query: &str,
    ancestor_has_more: &[bool],
    is_last: bool,
    newest_first: bool,
) -> Option<Vec<TreeRow<'a>>> {
    let summary = &summaries[index];
    let children = sorted_children(
        summary.session.id.as_str(),
        summaries,
        children_by_parent,
        newest_first,
    );
    let mut child_blocks = Vec::new();
    let mut any_child_visible = false;
    for (index, child) in children.iter().enumerate() {
        let child_is_last = index + 1 == children.len();
        let mut next_ancestors = ancestor_has_more.to_vec();
        next_ancestors.push(!is_last);
        if let Some(block) = build_tree_rows(
            summaries,
            *child,
            children_by_parent,
            query,
            &next_ancestors,
            child_is_last,
            newest_first,
        ) {
            any_child_visible = true;
            child_blocks.push(block);
        }
    }

    let self_matches = session_matches(summary, query);
    let visible = query.is_empty() || self_matches || any_child_visible;
    if !visible {
        return None;
    }

    let mut rows = vec![TreeRow {
        summary,
        prefix: tree_prefix(ancestor_has_more, is_last),
    }];
    for block in child_blocks {
        rows.extend(block);
    }
    Some(rows)
}

fn tree_prefix(ancestor_has_more: &[bool], is_last: bool) -> String {
    let mut prefix = String::new();
    for has_more in ancestor_has_more {
        if *has_more {
            prefix.push_str("│  ");
        } else {
            prefix.push_str("   ");
        }
    }
    if !ancestor_has_more.is_empty() {
        if is_last {
            prefix.push_str("└─ ");
        } else {
            prefix.push_str("├─ ");
        }
    }
    prefix
}

fn sorted_children<'a>(
    parent_id: &str,
    summaries: &'a [SessionSummary],
    children_by_parent: &HashMap<Option<String>, Vec<usize>>,
    newest_first: bool,
) -> Vec<usize> {
    let mut children = children_by_parent
        .get(&Some(parent_id.to_owned()))
        .cloned()
        .unwrap_or_default();
    sort_summaries(&mut children, summaries, newest_first);
    children
}

fn sort_summaries(indices: &mut Vec<usize>, summaries: &[SessionSummary], newest_first: bool) {
    if newest_first {
        indices.sort_by(|left, right| {
            let left = &summaries[*left];
            let right = &summaries[*right];
            (
                right.last_timestamp.as_deref().unwrap_or(""),
                right.session.id.as_str(),
            )
                .cmp(&(
                    left.last_timestamp.as_deref().unwrap_or(""),
                    left.session.id.as_str(),
                ))
        });
    } else {
        indices.sort_by(|left, right| {
            let left = &summaries[*left];
            let right = &summaries[*right];
            (
                left.last_timestamp.as_deref().unwrap_or(""),
                left.session.id.as_str(),
            )
                .cmp(&(
                    right.last_timestamp.as_deref().unwrap_or(""),
                    right.session.id.as_str(),
                ))
        });
    }
}

fn session_matches(summary: &SessionSummary, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }

    summary.session.id.to_lowercase().contains(query)
        || summary
            .parent_session_id
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(query)
        || summary
            .cwd
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(query)
        || summary
            .app_version
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(query)
        || summary
            .ai_provider
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(query)
        || summary
            .ai_model
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains(query)
        || summary.event_count.to_string().contains(query)
        || summary.stats.compact_label().to_lowercase().contains(query)
}

pub fn tree_window(state: &SessionTreeState, area_height: usize) -> (usize, Vec<TreeRow<'_>>) {
    let rows = state.tree_rows();
    if rows.is_empty() || area_height == 0 {
        return (0, Vec::new());
    }

    let visible = picker_visible_window(rows.len(), area_height, state.selected);
    (
        visible.start,
        rows.into_iter()
            .skip(visible.start)
            .take(visible.end.saturating_sub(visible.start))
            .collect(),
    )
}

fn picker_visible_window(total: usize, area_height: usize, selected: usize) -> PickerWindow {
    if total == 0 || area_height == 0 {
        return PickerWindow { start: 0, end: 0 };
    }

    let height = area_height.min(total);
    let mut start = selected.saturating_sub(height / 2);
    if start + height > total {
        start = total.saturating_sub(height);
    }

    PickerWindow {
        start,
        end: start + height,
    }
}

#[derive(Debug, Clone, Copy)]
struct PickerWindow {
    start: usize,
    end: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, parent_session_id: Option<&str>, last_timestamp: &str) -> SessionSummary {
        SessionSummary {
            session: crate::session::Session::new(id, std::path::Path::new("/tmp/project")),
            parent_session_id: parent_session_id.map(str::to_owned),
            cwd: Some("/tmp/project".to_owned()),
            app_version: Some("0.1.0".to_owned()),
            ai_provider: Some("minimax".to_owned()),
            ai_model: Some("MiniMax-M2.7".to_owned()),
            event_count: 1,
            stats: crate::session::SessionStats::default(),
            first_timestamp: Some(last_timestamp.to_owned()),
            last_timestamp: Some(last_timestamp.to_owned()),
        }
    }

    #[test]
    fn tree_rows_include_descendants() {
        let state = SessionTreeState::new(vec![
            summary("root", None, "1"),
            summary("child-a", Some("root"), "2"),
            summary("child-b", Some("root"), "3"),
        ]);

        let rows = state.tree_rows();

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].summary.session.id, "root");
        assert_eq!(rows[1].summary.session.id, "child-b");
        assert!(rows[1].prefix.contains("└─") || rows[1].prefix.contains("├─"));
    }

    #[test]
    fn tree_rows_filter_on_descendant_keeps_ancestor() {
        let mut state = SessionTreeState::new(vec![
            summary("root", None, "1"),
            summary("child-a", Some("root"), "2"),
            summary("child-b", Some("root"), "3"),
        ]);
        state.query = "child-b".to_owned();

        let rows = state.tree_rows();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].summary.session.id, "root");
        assert_eq!(rows[1].summary.session.id, "child-b");
    }

    #[test]
    fn tree_rows_match_stats_label() {
        let mut root = summary("root", None, "1");
        root.stats.command_runs = 3;
        let state = SessionTreeState::new(vec![root]);

        let mut state = state;
        state.query = "cmd=3".to_owned();

        let rows = state.tree_rows();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].summary.session.id, "root");
    }
}
