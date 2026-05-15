use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct PicocodeConfig {
    #[serde(default = "default_model_option")]
    pub model: ModelOption,
    #[serde(default)]
    pub models: Vec<ModelOption>,
}

impl Default for PicocodeConfig {
    fn default() -> Self {
        Self {
            model: default_model_option(),
            models: Vec::new(),
        }
    }
}

impl PicocodeConfig {
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from_path(config_path())
    }

    pub fn load_from_path(path: PathBuf) -> Result<Self, ConfigError> {
        read_toml_or_default(&path)
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to_path(config_path())
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let serialized = toml::to_string_pretty(self)?;
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_path = path.with_file_name(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config"),
            unique
        ));

        fs::write(&temp_path, serialized)?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }

    pub fn selected_model(&self) -> &ModelOption {
        &self.model
    }

    pub fn selected_api_key(&self) -> Result<String, ConfigError> {
        resolve_secret(&self.model.auth)
    }

    pub fn model_options(&self) -> Vec<ModelOption> {
        let mut options = vec![self.model.clone()];
        for model in &self.models {
            if !options
                .iter()
                .any(|option| option.provider == model.provider && option.model == model.model)
            {
                options.push(model.clone());
            }
        }
        options.sort_by(|left, right| {
            (&left.provider, &left.model).cmp(&(&right.provider, &right.model))
        });
        options
    }

    pub fn find_model(&self, provider: &str, model: &str) -> Option<&ModelOption> {
        if self.model.provider == provider && self.model.model == model {
            return Some(&self.model);
        }
        self.models
            .iter()
            .find(|option| option.provider == provider && option.model == model)
    }

    pub fn with_model_selection(&self, provider: &str, model: &str) -> Self {
        let mut config = self.clone();
        if let Some(option) = self
            .model_options()
            .into_iter()
            .find(|option| option.provider == provider && option.model == model)
        {
            config.model = option;
        }
        config
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ModelOption {
    pub provider: String,
    pub model: String,
    pub api: String,
    pub base_url: String,
    pub auth: String,
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub images: bool,
    #[serde(default)]
    pub reasoning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSelection {
    pub provider: String,
    pub model: String,
}

fn default_model_option() -> ModelOption {
    ModelOption {
        provider: "openai".to_owned(),
        model: "gpt-5".to_owned(),
        api: "openai-chat-completions".to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        auth: "OPENAI_API_KEY".to_owned(),
        tools: false,
        images: false,
        reasoning: false,
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Toml(toml::de::Error),
    TomlSerialize(toml::ser::Error),
    MissingEnv(String),
    CommandFailed(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "config read failed: {error}"),
            Self::Toml(error) => write!(formatter, "config parse failed: {error}"),
            Self::TomlSerialize(error) => write!(formatter, "config serialize failed: {error}"),
            Self::MissingEnv(variable) => {
                write!(
                    formatter,
                    "secret environment variable '{variable}' is not set"
                )
            }
            Self::CommandFailed(command) => {
                write!(formatter, "secret command failed: {command}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        Self::Toml(error)
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(error: toml::ser::Error) -> Self {
        Self::TomlSerialize(error)
    }
}

fn read_toml_or_default<T>(path: &Path) -> Result<T, ConfigError>
where
    T: Default + for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(T::default());
    }

    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

fn resolve_secret(value: &str) -> Result<String, ConfigError> {
    if let Some(command) = value.strip_prefix('!') {
        let output = Command::new("sh").arg("-c").arg(command).output()?;
        if !output.status.success() {
            return Err(ConfigError::CommandFailed(command.to_owned()));
        }
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }

    if looks_like_env_var(value) {
        return env::var(value).map_err(|_| ConfigError::MissingEnv(value.to_owned()));
    }

    Ok(value.to_owned())
}

fn looks_like_env_var(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
        && value.chars().any(|character| character == '_')
}

fn config_path() -> PathBuf {
    env::var("PICOCODE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| picocode_home().join("config.toml"))
}

fn picocode_home() -> PathBuf {
    env::var("PICOCODE_HOME")
        .map(PathBuf::from)
        .or_else(|_| env::var("HOME").map(|home| PathBuf::from(home).join(".picocode")))
        .unwrap_or_else(|_| PathBuf::from(".picocode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_selects_openai_gpt5() {
        let config = PicocodeConfig::default();

        assert_eq!(config.model.provider, "openai");
        assert_eq!(config.model.model, "gpt-5");
    }

    #[test]
    fn default_config_exposes_model_options() {
        let config = PicocodeConfig::default();
        let options = config.model_options();

        assert_eq!(options.len(), 1);
        assert_eq!(options[0].provider, "openai");
        assert_eq!(options[0].model, "gpt-5");
    }

    #[test]
    fn config_can_change_model_selection() {
        let mut config = PicocodeConfig::default();
        config.models.push(ModelOption {
            provider: "minimax".to_owned(),
            model: "MiniMax-M2.7".to_owned(),
            api: "openai-chat-completions".to_owned(),
            base_url: "https://api.minimaxi.com/v1".to_owned(),
            auth: "MINIMAX_API_KEY".to_owned(),
            tools: false,
            images: false,
            reasoning: false,
        });
        let config = config.with_model_selection("minimax", "MiniMax-M2.7");

        assert_eq!(config.model.provider, "minimax");
        assert_eq!(config.model.model, "MiniMax-M2.7");
    }

    #[test]
    fn parses_flat_model_config_toml() {
        let config: PicocodeConfig = toml::from_str(
            r#"
            [model]
            provider = "doubao"
            model = "doubao-seed-2-0-code-preview-260215"
            api = "openai-chat-completions"
            base_url = "https://ark.cn-beijing.volces.com/api/v3"
            auth = "ARK_API_KEY"
            tools = true

            [[models]]
            provider = "minimax"
            model = "MiniMax-M2.7"
            api = "openai-chat-completions"
            base_url = "https://api.minimaxi.com/v1"
            auth = "MINIMAX_API_KEY"
            tools = false
            "#,
        )
        .unwrap();

        assert_eq!(config.model.provider, "doubao");
        assert_eq!(config.model.model, "doubao-seed-2-0-code-preview-260215");
        assert!(config.model.tools);
        assert_eq!(config.models.len(), 1);
        assert_eq!(config.models[0].provider, "minimax");
        assert_eq!(config.models[0].auth, "MINIMAX_API_KEY");
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut config = PicocodeConfig::default();
        config.model = ModelOption {
            provider: "minimax".to_owned(),
            model: "MiniMax-M2.7".to_owned(),
            api: "openai-chat-completions".to_owned(),
            base_url: "https://api.minimaxi.com/v1".to_owned(),
            auth: "MINIMAX_API_KEY".to_owned(),
            tools: false,
            images: false,
            reasoning: false,
        };
        config.models.push(ModelOption {
            provider: "doubao".to_owned(),
            model: "doubao-seed-2-0-code-preview-260215".to_owned(),
            api: "openai-chat-completions".to_owned(),
            base_url: "https://ark.cn-beijing.volces.com/api/v3".to_owned(),
            auth: "ARK_API_KEY".to_owned(),
            tools: true,
            images: true,
            reasoning: false,
        });

        let rendered = toml::to_string_pretty(&config).unwrap();
        let decoded: PicocodeConfig = toml::from_str(&rendered).unwrap();

        assert_eq!(decoded.model.provider, "minimax");
        assert_eq!(decoded.models.len(), 1);
        assert_eq!(decoded.models[0].provider, "doubao");
    }

    #[test]
    fn env_like_secret_is_detected() {
        assert!(looks_like_env_var("OPENAI_API_KEY"));
        assert!(!looks_like_env_var("sk-test"));
    }
}
