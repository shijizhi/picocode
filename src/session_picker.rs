use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::session::SessionSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerState {
    summaries: Vec<SessionSummary>,
    pub query: String,
    pub selected: usize,
    pub show_paths: bool,
    pub newest_first: bool,
    pub prompt: Option<PickerPrompt>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPickerAction {
    Continue,
    Selected(String),
    Cancelled,
    RenameRequested { session_id: String, new_id: String },
    DeleteRequested { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerPrompt {
    Rename { session_id: String, input: String },
    DeleteConfirm { session_id: String },
}

impl SessionPickerState {
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
        let mut items = self.summaries.iter().collect::<Vec<_>>();
        if self.newest_first {
            items.sort_by(|left, right| {
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
            items.sort_by(|left, right| {
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

        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return items;
        }

        items
            .into_iter()
            .filter(|summary| session_matches(summary, &query))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SessionPickerAction {
        if let Some(prompt) = &mut self.prompt {
            return match prompt {
                PickerPrompt::Rename { session_id, input } => match key.code {
                    KeyCode::Esc => {
                        self.prompt = None;
                        SessionPickerAction::Continue
                    }
                    KeyCode::Enter => {
                        let session_id = session_id.clone();
                        let new_id = input.trim().to_owned();
                        self.prompt = None;
                        SessionPickerAction::RenameRequested { session_id, new_id }
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        SessionPickerAction::Continue
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        input.push(character);
                        SessionPickerAction::Continue
                    }
                    _ => SessionPickerAction::Continue,
                },
                PickerPrompt::DeleteConfirm { session_id } => match key.code {
                    KeyCode::Esc => {
                        self.prompt = None;
                        SessionPickerAction::Continue
                    }
                    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let session_id = session_id.clone();
                        self.prompt = None;
                        SessionPickerAction::DeleteRequested { session_id }
                    }
                    _ => SessionPickerAction::Continue,
                },
            };
        }

        match key.code {
            KeyCode::Esc => SessionPickerAction::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                SessionPickerAction::Cancelled
            }
            KeyCode::Enter => self
                .selected_session()
                .map(|session| SessionPickerAction::Selected(session.session.id.clone()))
                .unwrap_or(SessionPickerAction::Cancelled),
            KeyCode::Backspace => {
                self.query.pop();
                self.clamp_selected();
                SessionPickerAction::Continue
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                SessionPickerAction::Continue
            }
            KeyCode::Down => {
                let len = self.filtered_summaries().len();
                if self.selected + 1 < len {
                    self.selected += 1;
                }
                SessionPickerAction::Continue
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(10);
                SessionPickerAction::Continue
            }
            KeyCode::PageDown => {
                let len = self.filtered_summaries().len();
                self.selected = self.selected.saturating_add(10).min(len.saturating_sub(1));
                SessionPickerAction::Continue
            }
            KeyCode::Home => {
                self.selected = 0;
                SessionPickerAction::Continue
            }
            KeyCode::End => {
                let len = self.filtered_summaries().len();
                self.selected = len.saturating_sub(1);
                SessionPickerAction::Continue
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.show_paths = !self.show_paths;
                SessionPickerAction::Continue
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.newest_first = !self.newest_first;
                self.clamp_selected();
                SessionPickerAction::Continue
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(session) = self.selected_session() {
                    self.prompt = Some(PickerPrompt::Rename {
                        session_id: session.session.id.clone(),
                        input: session.session.id.clone(),
                    });
                }
                SessionPickerAction::Continue
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(session) = self.selected_session() {
                    self.prompt = Some(PickerPrompt::DeleteConfirm {
                        session_id: session.session.id.clone(),
                    });
                }
                SessionPickerAction::Continue
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                self.clamp_selected();
                SessionPickerAction::Continue
            }
            _ => SessionPickerAction::Continue,
        }
    }

    pub fn selected_session(&self) -> Option<&SessionSummary> {
        self.filtered_summaries().get(self.selected).copied()
    }

    pub fn prompt_label(&self) -> Option<String> {
        match &self.prompt {
            Some(PickerPrompt::Rename { input, .. }) => {
                Some(format!("rename: {}  (Enter save, Esc cancel)", input))
            }
            Some(PickerPrompt::DeleteConfirm { session_id }) => Some(format!(
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
}

fn session_matches(summary: &SessionSummary, query: &str) -> bool {
    summary.session.id.to_lowercase().contains(query)
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
        || summary.event_count.to_string().contains(query)
        || summary.stats.compact_label().to_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, SessionStats};

    fn summary(id: &str, compact_label: SessionStats) -> SessionSummary {
        SessionSummary {
            session: Session::new(id, std::path::Path::new("/tmp/project")),
            parent_session_id: None,
            cwd: Some("/tmp/project".to_owned()),
            app_version: Some("0.1.0".to_owned()),
            ai_provider: Some("minimax".to_owned()),
            ai_model: Some("MiniMax-M2.7".to_owned()),
            event_count: 1,
            stats: compact_label,
            first_timestamp: Some("1".to_owned()),
            last_timestamp: Some("1".to_owned()),
        }
    }

    #[test]
    fn picker_matches_stats_label() {
        let stats = SessionStats {
            command_runs: 2,
            tool_calls: 1,
            ..SessionStats::default()
        };
        let summary = summary("session-a", stats);

        assert!(session_matches(&summary, "cmd=2"));
        assert!(session_matches(&summary, "tool=1"));
        assert!(!session_matches(&summary, "cmd=9"));
    }
}
