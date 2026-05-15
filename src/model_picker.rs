use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{ModelOption, ModelSelection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickerState {
    options: Vec<ModelOption>,
    pub query: String,
    pub selected: usize,
    current_provider: String,
    current_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPickerAction {
    Continue,
    Selected(ModelSelection),
    Cancelled,
}

impl ModelPickerState {
    pub fn new(options: Vec<ModelOption>, current_provider: &str, current_model: &str) -> Self {
        let mut state = Self {
            options,
            query: String::new(),
            selected: 0,
            current_provider: current_provider.to_owned(),
            current_model: current_model.to_owned(),
        };
        if let Some(index) = state
            .filtered_options()
            .iter()
            .position(|option| option.provider == current_provider && option.model == current_model)
        {
            state.selected = index;
        }
        state
    }

    pub fn filtered_options(&self) -> Vec<&ModelOption> {
        let query = self.query.trim().to_lowercase();
        let mut items = self.options.iter().collect::<Vec<_>>();
        items.sort_by(|left, right| {
            (&left.provider, &left.model).cmp(&(&right.provider, &right.model))
        });
        if query.is_empty() {
            return items;
        }

        items
            .into_iter()
            .filter(|option| model_matches(option, &query))
            .collect()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelPickerAction {
        match key.code {
            KeyCode::Esc => ModelPickerAction::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ModelPickerAction::Cancelled
            }
            KeyCode::Enter => self
                .selected_option()
                .map(|option| {
                    ModelPickerAction::Selected(ModelSelection {
                        provider: option.provider.clone(),
                        model: option.model.clone(),
                    })
                })
                .unwrap_or(ModelPickerAction::Cancelled),
            KeyCode::Backspace => {
                self.query.pop();
                self.clamp_selected();
                ModelPickerAction::Continue
            }
            KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                ModelPickerAction::Continue
            }
            KeyCode::Down => {
                let len = self.filtered_options().len();
                if self.selected + 1 < len {
                    self.selected += 1;
                }
                ModelPickerAction::Continue
            }
            KeyCode::PageUp => {
                self.selected = self.selected.saturating_sub(10);
                ModelPickerAction::Continue
            }
            KeyCode::PageDown => {
                let len = self.filtered_options().len();
                self.selected = self.selected.saturating_add(10).min(len.saturating_sub(1));
                ModelPickerAction::Continue
            }
            KeyCode::Home => {
                self.selected = 0;
                ModelPickerAction::Continue
            }
            KeyCode::End => {
                let len = self.filtered_options().len();
                self.selected = len.saturating_sub(1);
                ModelPickerAction::Continue
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                self.clamp_selected();
                ModelPickerAction::Continue
            }
            _ => ModelPickerAction::Continue,
        }
    }

    pub fn selected_option(&self) -> Option<&ModelOption> {
        self.filtered_options().get(self.selected).copied()
    }

    pub fn current_label(&self) -> String {
        format!("{}/{}", self.current_provider, self.current_model)
    }

    fn clamp_selected(&mut self) {
        let len = self.filtered_options().len();
        self.selected = if len == 0 {
            0
        } else {
            self.selected.min(len.saturating_sub(1))
        };
    }
}

fn model_matches(option: &ModelOption, query: &str) -> bool {
    let mut haystacks = vec![
        option.provider.to_lowercase(),
        option.model.to_lowercase(),
        option.api.to_lowercase(),
        option.base_url.to_lowercase(),
        option.auth.to_lowercase(),
    ];
    if option.tools {
        haystacks.push("tools".to_owned());
    }
    if option.images {
        haystacks.push("images".to_owned());
    }
    if option.reasoning {
        haystacks.push("reasoning".to_owned());
    }

    haystacks.into_iter().any(|text| text.contains(query))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent};

    fn options() -> Vec<ModelOption> {
        vec![
            ModelOption {
                provider: "openai".to_owned(),
                model: "gpt-5".to_owned(),
                api: "openai-chat-completions".to_owned(),
                base_url: "https://api.openai.com/v1".to_owned(),
                auth: "OPENAI_API_KEY".to_owned(),
                tools: true,
                images: false,
                reasoning: true,
            },
            ModelOption {
                provider: "minimax".to_owned(),
                model: "minimax-2.7".to_owned(),
                api: "openai-chat-completions".to_owned(),
                base_url: "https://api.minimax.chat/v1".to_owned(),
                auth: "MINIMAX_API_KEY".to_owned(),
                tools: true,
                images: false,
                reasoning: false,
            },
        ]
    }

    #[test]
    fn picker_defaults_to_current_model() {
        let state = ModelPickerState::new(options(), "minimax", "minimax-2.7");

        assert_eq!(state.current_label(), "minimax/minimax-2.7");
        assert_eq!(state.selected_option().unwrap().model, "minimax-2.7");
    }

    #[test]
    fn picker_filters_by_model_name() {
        let mut state = ModelPickerState::new(options(), "openai", "gpt-5");
        state.query = "mini".to_owned();

        let filtered = state.filtered_options();

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider, "minimax");
    }

    #[test]
    fn picker_returns_selection_on_enter() {
        let mut state = ModelPickerState::new(options(), "openai", "gpt-5");

        let action = state.handle_key(KeyEvent::from(KeyCode::Enter));

        assert!(matches!(action, ModelPickerAction::Selected(_)));
    }
}
