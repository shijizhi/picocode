use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{ModelOption, PicocodeConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEditorState {
    options: Vec<ModelOption>,
    pub selected_model: usize,
    pub selected_field: ConfigField,
    pub prompt: Option<ConfigPrompt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigField {
    Provider,
    Model,
    BaseUrl,
    Api,
    Auth,
    Tools,
    Images,
    Reasoning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigPrompt {
    Edit {
        field: ConfigField,
        input: String,
        secret: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigEditorAction {
    Continue,
    Saved(PicocodeConfig),
    Cancelled,
}

impl ConfigEditorState {
    pub fn new(
        options: Vec<ModelOption>,
        current_provider: &str,
        current_model: &str,
        focus_field: Option<ConfigField>,
    ) -> Self {
        let options = if options.is_empty() {
            vec![PicocodeConfig::default().model]
        } else {
            options
        };
        let mut state = Self {
            options,
            selected_model: 0,
            selected_field: focus_field.unwrap_or(ConfigField::Provider),
            prompt: None,
        };
        if let Some(index) = state
            .options
            .iter()
            .position(|option| option.provider == current_provider && option.model == current_model)
        {
            state.selected_model = index;
        }
        state.clamp_selected_field();
        state
    }

    pub fn current_option(&self) -> Option<&ModelOption> {
        self.options.get(self.selected_model)
    }

    pub fn current_option_mut(&mut self) -> Option<&mut ModelOption> {
        self.options.get_mut(self.selected_model)
    }

    pub fn current_label(&self) -> String {
        self.current_option()
            .map(|option| format!("{}/{}", option.provider, option.model))
            .unwrap_or_else(|| "none".to_owned())
    }

    pub fn summary_label(&self) -> String {
        format!(
            "{} model(s)  ·  current: {}",
            self.options.len(),
            self.current_label()
        )
    }

    pub fn prompt_label(&self) -> Option<String> {
        match &self.prompt {
            Some(ConfigPrompt::Edit {
                field,
                input,
                secret: _,
            }) => {
                let value = if input.is_empty() {
                    "<empty>".to_owned()
                } else {
                    input.clone()
                };
                let label = if *field == ConfigField::Auth {
                    "auth (env var name or raw key)"
                } else {
                    field.label()
                };
                Some(format!("{label}: {value}  (Enter save, Esc cancel)"))
            }
            None => None,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigEditorAction {
        if let Some(prompt) = self.prompt.take() {
            let mut prompt = prompt;
            let action = match &mut prompt {
                ConfigPrompt::Edit { input, .. } => match key.code {
                    KeyCode::Esc => ConfigEditorAction::Continue,
                    KeyCode::Enter => {
                        let input = input.trim().to_owned();
                        self.apply_prompt_value(input);
                        ConfigEditorAction::Continue
                    }
                    KeyCode::Backspace => {
                        input.pop();
                        ConfigEditorAction::Continue
                    }
                    KeyCode::Char(character)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        input.push(character);
                        ConfigEditorAction::Continue
                    }
                    _ => ConfigEditorAction::Continue,
                },
            };

            if !matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                self.prompt = Some(prompt);
            }
            return action;
        }

        match key.code {
            KeyCode::Esc => ConfigEditorAction::Cancelled,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ConfigEditorAction::Cancelled
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ConfigEditorAction::Saved(self.to_config())
            }
            KeyCode::Left => {
                self.selected_model = self.selected_model.saturating_sub(1);
                self.clamp_selected_field();
                ConfigEditorAction::Continue
            }
            KeyCode::Right => {
                if self.selected_model + 1 < self.options.len() {
                    self.selected_model += 1;
                }
                self.clamp_selected_field();
                ConfigEditorAction::Continue
            }
            KeyCode::Up => {
                self.selected_field = self.selected_field.previous();
                ConfigEditorAction::Continue
            }
            KeyCode::Down => {
                self.selected_field = self.selected_field.next();
                ConfigEditorAction::Continue
            }
            KeyCode::Tab => {
                self.selected_field = self.selected_field.next();
                ConfigEditorAction::Continue
            }
            KeyCode::BackTab => {
                self.selected_field = self.selected_field.previous();
                ConfigEditorAction::Continue
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.duplicate_current_model();
                ConfigEditorAction::Continue
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.delete_current_model();
                ConfigEditorAction::Continue
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                self.start_editing_selected_field();
                ConfigEditorAction::Continue
            }
            KeyCode::Char(' ') => {
                if self.selected_field.is_toggle() {
                    self.toggle_selected_field();
                } else {
                    self.start_editing_selected_field();
                }
                ConfigEditorAction::Continue
            }
            _ => ConfigEditorAction::Continue,
        }
    }

    pub fn handle_paste(&mut self, text: &str) -> ConfigEditorAction {
        if let Some(ConfigPrompt::Edit { input, .. }) = &mut self.prompt {
            input.push_str(text);
        }
        ConfigEditorAction::Continue
    }

    fn start_editing_selected_field(&mut self) {
        let field = self.selected_field;
        if field.is_toggle() {
            self.toggle_selected_field();
            return;
        }

        let input = self
            .current_option()
            .map(|option| field.current_value(option))
            .unwrap_or_default();
        self.prompt = Some(ConfigPrompt::Edit {
            field,
            input,
            secret: false,
        });
    }

    fn apply_prompt_value(&mut self, value: String) {
        let field = match &self.prompt {
            Some(ConfigPrompt::Edit { field, .. }) => *field,
            None => return,
        };
        if let Some(option) = self.current_option_mut() {
            field.set_value(option, value);
        }
    }

    fn toggle_selected_field(&mut self) {
        let field = self.selected_field;
        if let Some(option) = self.current_option_mut() {
            field.toggle(option);
        }
    }

    fn duplicate_current_model(&mut self) {
        let Some(current) = self.current_option().cloned() else {
            return;
        };
        let mut clone = current;
        let existing = self
            .options
            .iter()
            .map(|option| (option.provider.clone(), option.model.clone()))
            .collect::<HashSet<_>>();
        clone.model = unique_model_name(&clone.model, &existing);
        self.options.insert(self.selected_model + 1, clone);
        self.selected_model += 1;
        self.selected_field = ConfigField::Provider;
    }

    fn delete_current_model(&mut self) {
        if self.options.len() <= 1 {
            return;
        }
        self.options.remove(self.selected_model);
        if self.selected_model >= self.options.len() {
            self.selected_model = self.options.len().saturating_sub(1);
        }
        self.clamp_selected_field();
    }

    fn clamp_selected_field(&mut self) {
        let fields = ConfigField::all();
        let index = fields
            .iter()
            .position(|field| *field == self.selected_field)
            .unwrap_or(0);
        self.selected_field = fields[index.min(fields.len().saturating_sub(1))];
    }

    fn to_config(&self) -> PicocodeConfig {
        if self.options.is_empty() {
            return PicocodeConfig::default();
        }

        let mut config = PicocodeConfig::default();
        config.model = self.options[self.selected_model].clone();
        let mut seen = HashSet::<(String, String)>::new();
        seen.insert((config.model.provider.clone(), config.model.model.clone()));
        config.models = self
            .options
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.selected_model)
            .filter_map(|(_, option)| {
                let key = (option.provider.clone(), option.model.clone());
                if seen.insert(key) {
                    Some(option.clone())
                } else {
                    None
                }
            })
            .collect();
        config
    }
}

impl ConfigField {
    pub const fn all() -> [Self; 8] {
        [
            Self::Provider,
            Self::Model,
            Self::BaseUrl,
            Self::Api,
            Self::Auth,
            Self::Tools,
            Self::Images,
            Self::Reasoning,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Model => "model",
            Self::BaseUrl => "base_url",
            Self::Api => "api",
            Self::Auth => "auth",
            Self::Tools => "tools",
            Self::Images => "images",
            Self::Reasoning => "reasoning",
        }
    }

    pub fn current_value(self, option: &ModelOption) -> String {
        match self {
            Self::Provider => option.provider.clone(),
            Self::Model => option.model.clone(),
            Self::BaseUrl => option.base_url.clone(),
            Self::Api => option.api.clone(),
            Self::Auth => option.auth.clone(),
            Self::Tools => option.tools.to_string(),
            Self::Images => option.images.to_string(),
            Self::Reasoning => option.reasoning.to_string(),
        }
    }

    pub fn set_value(self, option: &mut ModelOption, value: String) {
        match self {
            Self::Provider => option.provider = value,
            Self::Model => option.model = value,
            Self::BaseUrl => option.base_url = value,
            Self::Api => option.api = value,
            Self::Auth => option.auth = value,
            Self::Tools => option.tools = parse_bool(&value).unwrap_or(option.tools),
            Self::Images => option.images = parse_bool(&value).unwrap_or(option.images),
            Self::Reasoning => option.reasoning = parse_bool(&value).unwrap_or(option.reasoning),
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(self, Self::Tools | Self::Images | Self::Reasoning)
    }

    pub fn toggle(self, option: &mut ModelOption) {
        match self {
            Self::Tools => option.tools = !option.tools,
            Self::Images => option.images = !option.images,
            Self::Reasoning => option.reasoning = !option.reasoning,
            _ => {}
        }
    }

    pub fn next(self) -> Self {
        let fields = Self::all();
        let index = fields.iter().position(|field| *field == self).unwrap_or(0);
        fields[(index + 1) % fields.len()]
    }

    pub fn previous(self) -> Self {
        let fields = Self::all();
        let index = fields.iter().position(|field| *field == self).unwrap_or(0);
        fields[(index + fields.len() - 1) % fields.len()]
    }

    pub fn display_value(self, option: &ModelOption) -> String {
        match self {
            Self::Auth => option.auth.clone(),
            Self::Tools => bool_label(option.tools),
            Self::Images => bool_label(option.images),
            Self::Reasoning => bool_label(option.reasoning),
            _ => self.current_value(option),
        }
    }
}

fn bool_label(value: bool) -> String {
    if value {
        "on".to_owned()
    } else {
        "off".to_owned()
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn unique_model_name(base: &str, existing: &HashSet<(String, String)>) -> String {
    let mut suffix = 1usize;
    loop {
        let candidate = if suffix == 1 {
            format!("{base}-copy")
        } else {
            format!("{base}-copy-{suffix}")
        };
        if !existing
            .iter()
            .any(|(_, model)| model.as_str() == candidate.as_str())
        {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}
