mod openai;

use crate::{
    config::{ConfigError, PicocodeConfig},
    event::{self, Event, EventMsg},
};

pub use openai::OpenAiCompatibleProvider;

#[derive(Debug)]
pub struct AiClient {
    provider: Box<dyn ApiProvider + Send + Sync>,
    model: Model,
}

impl AiClient {
    pub fn from_config(config: &PicocodeConfig) -> Result<Self, AiError> {
        let model_config = config.selected_model();
        let api_key = config.selected_api_key()?;

        match model_config.api.as_str() {
            "openai-chat-completions" => {
                let provider = OpenAiCompatibleProvider::new(openai::OpenAiConfig {
                    base_url: model_config.base_url.clone(),
                    api_key,
                    model_id: model_config.model.clone(),
                    capabilities: ModelCapabilities {
                        tools: model_config.tools,
                        images: model_config.images,
                        reasoning: model_config.reasoning,
                    },
                });
                let model = provider.model().clone();
                Ok(Self::new(Box::new(provider), model))
            }
            other => Err(AiError::UnsupportedApi(other.to_owned())),
        }
    }

    pub fn new(provider: Box<dyn ApiProvider + Send + Sync>, model: Model) -> Self {
        Self { provider, model }
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }

    pub fn model_id(&self) -> &str {
        &self.model.id
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        self.model.capabilities
    }

    pub fn complete(&self, context: &AiContext) -> Result<AssistantOutput, AiError> {
        if context_uses_images(context) && !self.model.capabilities.images {
            return Err(AiError::ProviderFailed(format!(
                "selected model '{}' does not support image input. Switch to a vision-capable model.",
                self.model.id
            )));
        }
        self.provider.complete(&self.model, context)
    }
}

pub trait ApiProvider: std::fmt::Debug + Send + Sync {
    fn name(&self) -> &str;
    fn model(&self) -> &Model;
    fn complete(&self, model: &Model, context: &AiContext) -> Result<AssistantOutput, AiError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub provider: String,
    pub api: String,
    pub capabilities: ModelCapabilities,
}

impl Model {
    pub fn new(
        id: impl Into<String>,
        provider: impl Into<String>,
        api: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            api: api.into(),
            capabilities,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelCapabilities {
    pub tools: bool,
    pub images: bool,
    pub reasoning: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiContext {
    pub system_prompt: Option<String>,
    pub messages: Vec<AiMessage>,
    pub tools: Vec<ToolSpec>,
}

impl AiContext {
    pub fn new(system_prompt: Option<String>, messages: Vec<AiMessage>) -> Self {
        Self {
            system_prompt,
            messages,
            tools: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AiMessage {
    User(UserMessage),
    Assistant(AssistantMessage),
    ToolResult(ToolResultMessage),
}

impl AiMessage {
    pub fn user_text(content: impl Into<String>) -> Self {
        Self::User(UserMessage {
            content: vec![ContentBlock::text(content)],
        })
    }

    pub fn user_content(content: Vec<ContentBlock>) -> Self {
        Self::User(UserMessage { content })
    }

    pub fn assistant_text(content: impl Into<String>) -> Self {
        Self::Assistant(AssistantMessage {
            content: vec![ContentBlock::text(content)],
            stop_reason: None,
            usage: None,
        })
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult(ToolResultMessage {
            tool_call_id: tool_call_id.into(),
            content: vec![ContentBlock::text(content)],
            is_error,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage {
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMessage {
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultMessage {
    pub tool_call_id: String,
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ContentBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    ToolCall(ToolCallContent),
    Image(ImageContent),
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(TextContent { text: text.into() })
    }

    pub fn thinking(text: impl Into<String>) -> Self {
        Self::Thinking(ThinkingContent { text: text.into() })
    }

    pub fn tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self::ToolCall(ToolCallContent {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        })
    }

    pub fn image(
        source_path: impl Into<String>,
        file_name: impl Into<String>,
        mime_type: impl Into<String>,
        data_url: impl Into<String>,
    ) -> Self {
        Self::Image(ImageContent {
            source_path: source_path.into(),
            file_name: file_name.into(),
            mime_type: mime_type.into(),
            data_url: data_url.into(),
        })
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(content) => Some(content.text.as_str()),
            Self::Thinking(_) | Self::ToolCall(_) | Self::Image(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextContent {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingContent {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallContent {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageContent {
    pub source_path: String,
    pub file_name: String,
    pub mime_type: String,
    pub data_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantOutput {
    pub message: AssistantMessage,
    pub raw_response_id: Option<String>,
}

impl AssistantOutput {
    pub fn from_provider_content(content: impl Into<String>) -> Self {
        let content = content.into();
        Self {
            message: AssistantMessage {
                content: provider_content_blocks(&content),
                stop_reason: Some(StopReason::Stop),
                usage: None,
            },
            raw_response_id: None,
        }
    }

    pub fn text_content(&self) -> String {
        content_blocks_to_text(&self.message.content)
            .trim()
            .to_owned()
    }

    pub fn tool_calls(&self) -> Vec<ToolCallContent> {
        self.message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolCall(content) => Some(content.clone()),
                ContentBlock::Text(_) | ContentBlock::Thinking(_) | ContentBlock::Image(_) => None,
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug)]
pub enum AiError {
    UnsupportedProvider(String),
    UnsupportedApi(String),
    MissingModel(String),
    Config(ConfigError),
    Io(std::io::Error),
    ProviderFailed(String),
}

impl std::fmt::Display for AiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedProvider(provider) => {
                write!(formatter, "Unsupported AI provider '{provider}'")
            }
            Self::UnsupportedApi(api) => write!(formatter, "Unsupported AI API '{api}'"),
            Self::MissingModel(model) => write!(formatter, "AI model '{model}' is not configured"),
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Io(error) => write!(formatter, "AI request failed: {error}"),
            Self::ProviderFailed(error) => write!(formatter, "AI request failed: {error}"),
        }
    }
}

impl std::error::Error for AiError {}

impl From<ConfigError> for AiError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

pub fn context_from_events(events: &[Event]) -> AiContext {
    let mut system_prompts = vec![
        "You are picocode, a concise local coding assistant running inside a terminal UI."
            .to_owned(),
    ];
    let mut messages = Vec::new();
    let mut start_index = 0;
    let mut pending_images = Vec::new();

    if let Some(index) = event::latest_summary_event_index(events) {
        match &events[index].msg {
            EventMsg::Compaction(compaction) => {
                system_prompts.push(format!(
                    "Session compaction summary:\n{}",
                    compaction.summary
                ));
            }
            EventMsg::BranchSummary(summary) => {
                system_prompts.push(format!("Session branch summary:\n{}", summary.summary));
            }
            _ => {}
        }
        start_index = index.saturating_add(1);
    }

    for event in &events[start_index..] {
        match &event.msg {
            EventMsg::SystemMessage(event) => system_prompts.push(event.content.clone()),
            EventMsg::UserMessage(event) => {
                let mut content = pending_images.drain(..).collect::<Vec<ContentBlock>>();
                content.push(ContentBlock::text(event.content.clone()));
                messages.push(AiMessage::user_content(content));
            }
            EventMsg::AssistantMessage(event) => {
                messages.push(AiMessage::assistant_text(event.content.clone()));
            }
            EventMsg::ImageAttachment(event) => {
                pending_images.push(ContentBlock::image(
                    event.source_path.clone(),
                    event.file_name.clone(),
                    event.mime_type.clone(),
                    event.data_url.clone(),
                ));
            }
            EventMsg::ToolCall(event) => {
                messages.push(AiMessage::Assistant(AssistantMessage {
                    content: vec![ContentBlock::tool_call(
                        event.call_id.clone(),
                        event.name.clone(),
                        event.arguments.clone(),
                    )],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: None,
                }));
            }
            EventMsg::ToolResult(event) => {
                messages.push(AiMessage::tool_result(
                    event.call_id.clone(),
                    event.content.clone(),
                    event.status.as_str() == "error" || event.status.as_str() == "denied",
                ));
            }
            EventMsg::CommandRun(_)
            | EventMsg::CommandOutput(_)
            | EventMsg::Compaction(_)
            | EventMsg::BranchSummary(_)
            | EventMsg::FileEdit(_)
            | EventMsg::Error(_)
            | EventMsg::Final(_) => {}
        }
    }

    AiContext::new(Some(system_prompts.join("\n\n")), messages)
}

pub fn content_blocks_to_text(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("")
}

pub fn context_uses_images(context: &AiContext) -> bool {
    context.messages.iter().any(|message| match message {
        AiMessage::User(message) => message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Image(_))),
        AiMessage::Assistant(_) | AiMessage::ToolResult(_) => false,
    })
}

fn provider_content_blocks(content: &str) -> Vec<ContentBlock> {
    const THINK_START: &str = "<think>";
    const THINK_END: &str = "</think>";

    let mut blocks = Vec::new();
    let mut remaining = content;
    let mut saw_thinking = false;

    while let Some(start) = remaining.find(THINK_START) {
        push_visible_text(&mut blocks, &remaining[..start], saw_thinking);
        let after_start = &remaining[start + THINK_START.len()..];

        let Some(end) = after_start.find(THINK_END) else {
            push_visible_text(&mut blocks, remaining, saw_thinking);
            return blocks;
        };

        let thinking = after_start[..end].trim();
        if !thinking.is_empty() {
            blocks.push(ContentBlock::thinking(thinking));
            saw_thinking = true;
        }
        remaining = &after_start[end + THINK_END.len()..];
    }

    push_visible_text(&mut blocks, remaining, saw_thinking);

    if blocks.is_empty() {
        blocks.push(ContentBlock::text(""));
    }

    blocks
}

fn push_visible_text(blocks: &mut Vec<ContentBlock>, text: &str, trim_start: bool) {
    let text = if trim_start { text.trim_start() } else { text };
    if !text.is_empty() {
        blocks.push(ContentBlock::text(text));
    }
}

pub(crate) fn json_escape(value: &str) -> String {
    let mut escaped = String::new();
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventMsg;
    use crate::tool::ToolResultStatus;

    #[test]
    fn context_from_events_collects_system_prompt_and_messages() {
        let events = vec![
            Event::new("evt-0", EventMsg::system("project rules")),
            Event::new("evt-1", EventMsg::user("hello")),
            Event::new("evt-2", EventMsg::assistant("hi")),
        ];

        let context = context_from_events(&events);

        assert!(context
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("project rules"));
        assert_eq!(context.messages.len(), 2);
        assert!(matches!(context.messages[0], AiMessage::User(_)));
        assert!(matches!(context.messages[1], AiMessage::Assistant(_)));
    }

    #[test]
    fn context_from_events_maps_tool_events_to_tool_messages() {
        let events = vec![
            Event::new(
                "evt-0",
                EventMsg::ToolCall(crate::event::ToolCallEvent::new(
                    "call-0",
                    "read",
                    "path=Cargo.toml",
                )),
            ),
            Event::new(
                "evt-1",
                EventMsg::ToolResult(crate::event::ToolResultEvent {
                    call_id: "call-0".to_owned(),
                    status: ToolResultStatus::Success,
                    content: "path: Cargo.toml".to_owned(),
                    truncated: false,
                    next_offset: None,
                    edits: Vec::new(),
                }),
            ),
        ];

        let context = context_from_events(&events);

        assert_eq!(context.messages.len(), 2);
        match &context.messages[0] {
            AiMessage::Assistant(message) => {
                assert!(matches!(message.content[0], ContentBlock::ToolCall(_)));
            }
            _ => panic!("expected assistant tool call message"),
        }
        match &context.messages[1] {
            AiMessage::ToolResult(message) => {
                assert_eq!(message.tool_call_id, "call-0");
                assert_eq!(content_blocks_to_text(&message.content), "path: Cargo.toml");
            }
            _ => panic!("expected tool result message"),
        }
    }

    #[test]
    fn context_from_events_attaches_images_to_the_next_user_message() {
        let events = vec![
            Event::new(
                "evt-0",
                EventMsg::ImageAttachment(crate::event::ImageAttachmentEvent::new(
                    "./shot.png",
                    "shot.png",
                    "image/png",
                    12,
                    "data:image/png;base64,AAAA",
                )),
            ),
            Event::new("evt-1", EventMsg::user("what do you see?")),
        ];

        let context = context_from_events(&events);

        assert_eq!(context.messages.len(), 1);
        match &context.messages[0] {
            AiMessage::User(message) => {
                assert!(matches!(message.content[0], ContentBlock::Image(_)));
                assert_eq!(content_blocks_to_text(&message.content), "what do you see?");
            }
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn context_from_events_uses_latest_compaction_summary_and_skips_earlier_events() {
        let events = vec![
            Event::new("evt-0", EventMsg::user("old user")),
            Event::new(
                "evt-1",
                EventMsg::Compaction(crate::event::CompactionEvent {
                    summary: "Goal: keep the active thread focused.".to_owned(),
                    folded_event_count: 1,
                }),
            ),
            Event::new("evt-2", EventMsg::system("project rules")),
            Event::new("evt-3", EventMsg::user("new user")),
        ];

        let context = context_from_events(&events);

        assert!(context
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("Session compaction summary"));
        assert!(context
            .system_prompt
            .as_ref()
            .unwrap()
            .contains("Goal: keep the active thread focused."));
        assert_eq!(context.messages.len(), 1);
        match &context.messages[0] {
            AiMessage::User(message) => {
                assert_eq!(content_blocks_to_text(&message.content), "new user");
            }
            _ => panic!("expected user message"),
        }
    }

    #[test]
    fn text_output_concatenates_text_blocks() {
        let output = AssistantOutput {
            message: AssistantMessage {
                content: vec![ContentBlock::text("hello"), ContentBlock::text(" world")],
                stop_reason: Some(StopReason::Stop),
                usage: None,
            },
            raw_response_id: None,
        };

        assert_eq!(output.text_content(), "hello world");
    }

    #[test]
    fn provider_content_preserves_plain_text() {
        let output = AssistantOutput::from_provider_content("hello world");

        assert_eq!(output.text_content(), "hello world");
        assert_eq!(
            output.message.content,
            vec![ContentBlock::text("hello world")]
        );
    }

    #[test]
    fn provider_content_moves_think_tags_out_of_visible_text() {
        let output = AssistantOutput::from_provider_content(
            "<think>The user is asking who I am.</think>\n我是 picocode。",
        );

        assert_eq!(output.text_content(), "我是 picocode。");
        assert_eq!(
            output.message.content,
            vec![
                ContentBlock::thinking("The user is asking who I am."),
                ContentBlock::text("我是 picocode。"),
            ]
        );
    }

    #[test]
    fn provider_content_handles_multiple_think_blocks() {
        let output = AssistantOutput::from_provider_content(
            "A<think>hidden 1</think>B<think>hidden 2</think>C",
        );

        assert_eq!(output.text_content(), "ABC");
        assert_eq!(
            output.message.content,
            vec![
                ContentBlock::text("A"),
                ContentBlock::thinking("hidden 1"),
                ContentBlock::text("B"),
                ContentBlock::thinking("hidden 2"),
                ContentBlock::text("C"),
            ]
        );
    }

    #[test]
    fn json_escape_handles_quotes_and_newlines() {
        assert_eq!(json_escape("a \"quote\"\n"), "a \\\"quote\\\"\\n");
    }
}
