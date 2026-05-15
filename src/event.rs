use crate::{
    tool::{ToolCall, ToolResult, ToolResultStatus},
    workspace::{EditApplyResult, EditCheckpoint, EditRewindResult},
};

pub type EventId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: EventId,
    pub msg: EventMsg,
}

impl Event {
    pub fn new(id: impl Into<EventId>, msg: EventMsg) -> Self {
        Self { id: id.into(), msg }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventMsg {
    SystemMessage(SystemMessageEvent),
    UserMessage(UserMessageEvent),
    AssistantMessage(AssistantMessageEvent),
    ImageAttachment(ImageAttachmentEvent),
    ToolCall(ToolCallEvent),
    ToolResult(ToolResultEvent),
    CommandRun(CommandRunEvent),
    CommandOutput(CommandOutputEvent),
    Compaction(CompactionEvent),
    BranchSummary(BranchSummaryEvent),
    FileEdit(FileEditEvent),
    Error(ErrorEvent),
    Final(FinalEvent),
}

impl EventMsg {
    pub fn system(content: impl Into<String>) -> Self {
        Self::SystemMessage(SystemMessageEvent {
            content: content.into(),
        })
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::UserMessage(UserMessageEvent {
            content: content.into(),
        })
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::AssistantMessage(AssistantMessageEvent {
            content: content.into(),
        })
    }

    pub fn image_attachment(
        source_path: impl Into<String>,
        file_name: impl Into<String>,
        mime_type: impl Into<String>,
        byte_len: usize,
        data_url: impl Into<String>,
    ) -> Self {
        Self::ImageAttachment(ImageAttachmentEvent::new(
            source_path,
            file_name,
            mime_type,
            byte_len,
            data_url,
        ))
    }

    pub fn tool_call(call: ToolCall) -> Self {
        Self::ToolCall(ToolCallEvent::new(call.id, call.name, call.arguments))
    }

    pub fn tool_result(result: ToolResult) -> Self {
        Self::ToolResult(ToolResultEvent {
            call_id: result.call_id,
            status: result.status,
            content: result.content,
            truncated: result.truncated,
            next_offset: result.next_offset,
            edits: result.edits,
        })
    }

    pub fn command_run(event: CommandRunEvent) -> Self {
        Self::CommandRun(event)
    }

    pub fn command_output(event: CommandOutputEvent) -> Self {
        Self::CommandOutput(event)
    }

    pub fn compaction(event: CompactionEvent) -> Self {
        Self::Compaction(event)
    }

    pub fn branch_summary(event: BranchSummaryEvent) -> Self {
        Self::BranchSummary(event)
    }

    pub fn file_edit(edit: FileEditEvent) -> Self {
        Self::FileEdit(edit)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error(ErrorEvent {
            message: message.into(),
        })
    }

    #[allow(dead_code)]
    pub fn final_summary(summary: impl Into<String>) -> Self {
        Self::Final(FinalEvent {
            summary: summary.into(),
        })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::SystemMessage(_) => "system",
            Self::UserMessage(_) => "you",
            Self::AssistantMessage(_) => "assistant",
            Self::ImageAttachment(_) => "image",
            Self::ToolCall(_) => "tool",
            Self::ToolResult(_) => "tool result",
            Self::CommandRun(_) => "command",
            Self::CommandOutput(_) => "command output",
            Self::Compaction(_) => "compaction",
            Self::BranchSummary(_) => "branch summary",
            Self::FileEdit(_) => "file edit",
            Self::Error(_) => "error",
            Self::Final(_) => "final",
        }
    }

    pub fn content(&self) -> &str {
        match self {
            Self::SystemMessage(event) => event.content.as_str(),
            Self::UserMessage(event) => event.content.as_str(),
            Self::AssistantMessage(event) => event.content.as_str(),
            Self::ImageAttachment(event) => event.summary.as_str(),
            Self::ToolCall(event) => event.summary.as_str(),
            Self::ToolResult(event) => event.content.as_str(),
            Self::CommandRun(event) => event.summary.as_str(),
            Self::CommandOutput(event) => event.summary.as_str(),
            Self::Compaction(event) => event.summary.as_str(),
            Self::BranchSummary(event) => event.summary.as_str(),
            Self::FileEdit(event) => event.summary.as_str(),
            Self::Error(event) => event.message.as_str(),
            Self::Final(event) => event.summary.as_str(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemMessageEvent {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessageEvent {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantMessageEvent {
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageAttachmentEvent {
    pub source_path: String,
    pub file_name: String,
    pub mime_type: String,
    pub byte_len: usize,
    pub data_url: String,
    pub summary: String,
}

impl ImageAttachmentEvent {
    pub fn new(
        source_path: impl Into<String>,
        file_name: impl Into<String>,
        mime_type: impl Into<String>,
        byte_len: usize,
        data_url: impl Into<String>,
    ) -> Self {
        let source_path = source_path.into();
        let file_name = file_name.into();
        let mime_type = mime_type.into();
        let data_url = data_url.into();
        let summary = format!(
            "attached image: {} ({}, {})",
            file_name,
            mime_type,
            format_bytes(byte_len)
        );
        Self {
            source_path,
            file_name,
            mime_type,
            byte_len,
            data_url,
            summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallEvent {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub summary: String,
}

impl ToolCallEvent {
    pub fn new(
        call_id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        let call_id = call_id.into();
        let name = name.into();
        let arguments = arguments.into();
        let summary = if arguments.trim().is_empty() {
            name.clone()
        } else {
            format!("{} {}", name, arguments.replace('\n', " "))
        };
        Self {
            call_id,
            name,
            arguments,
            summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultEvent {
    pub call_id: String,
    pub status: ToolResultStatus,
    pub content: String,
    pub truncated: bool,
    pub next_offset: Option<usize>,
    pub edits: Vec<FileEditEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRunEvent {
    pub call_id: String,
    pub command: String,
    pub cwd: String,
    pub timeout_seconds: u64,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutputEvent {
    pub call_id: String,
    pub stream: String,
    pub content: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionEvent {
    pub summary: String,
    pub folded_event_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSummaryEvent {
    pub source_session_id: String,
    pub summary: String,
    pub folded_event_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEditEvent {
    pub action: FileEditAction,
    pub checkpoint: EditCheckpoint,
    pub rewound_checkpoint_id: Option<String>,
    pub summary: String,
}

impl FileEditEvent {
    pub fn from_edit_apply_result(result: EditApplyResult) -> Self {
        let EditApplyResult { checkpoint } = result;
        let summary = format!(
            "applied edit to {} ({})",
            checkpoint.path, checkpoint.checkpoint_id
        );
        Self {
            action: FileEditAction::Applied,
            checkpoint,
            rewound_checkpoint_id: None,
            summary,
        }
    }

    pub fn from_edit_rewind_result(result: EditRewindResult) -> Self {
        let EditRewindResult {
            rewound_checkpoint_id,
            checkpoint,
        } = result;
        let summary = format!(
            "rewound checkpoint {} on {}",
            rewound_checkpoint_id, checkpoint.path
        );
        Self {
            action: FileEditAction::RolledBack,
            checkpoint,
            rewound_checkpoint_id: Some(rewound_checkpoint_id),
            summary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileEditAction {
    Applied,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorEvent {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalEvent {
    pub summary: String,
}

fn format_bytes(byte_len: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    if byte_len >= MB {
        format!("{:.1} MB", byte_len as f64 / MB as f64)
    } else if byte_len >= KB {
        format!("{:.1} KB", byte_len as f64 / KB as f64)
    } else {
        format!("{byte_len} bytes")
    }
}

pub const AUTO_COMPACTION_TRIGGER_EVENTS: usize = 36;
const COMPACTION_RECENT_EVENT_WINDOW: usize = 12;
const COMPACTION_RECENT_DETAIL_WINDOW: usize = 6;

pub fn should_auto_compact(events: &[Event]) -> bool {
    count_active_events_since_compaction(events) >= AUTO_COMPACTION_TRIGGER_EVENTS
}

pub fn count_active_events_since_compaction(events: &[Event]) -> usize {
    count_active_events_since_summary(events)
}

pub fn summarize_compaction(events: &[Event]) -> CompactionEvent {
    let start_index = latest_summary_event_index(events)
        .map(|index| index.saturating_add(1))
        .unwrap_or(0);
    let active = &events[start_index..];
    let total = active.len();
    let folded_event_count = total.saturating_sub(COMPACTION_RECENT_EVENT_WINDOW);
    let recent = active
        .iter()
        .rev()
        .take(COMPACTION_RECENT_DETAIL_WINDOW)
        .collect::<Vec<_>>();

    let last_user = recent.iter().find_map(|event| match &event.msg {
        EventMsg::UserMessage(message) => Some(message.content.as_str()),
        _ => None,
    });
    let last_assistant = recent.iter().find_map(|event| match &event.msg {
        EventMsg::AssistantMessage(message) => Some(message.content.as_str()),
        _ => None,
    });
    let recent_tools = recent
        .iter()
        .filter_map(|event| match &event.msg {
            EventMsg::ToolCall(call) => Some(call.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let recent_commands = recent
        .iter()
        .filter_map(|event| match &event.msg {
            EventMsg::CommandRun(command) => Some(command.command.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut summary = vec![
        format!("Goal: keep the thread compact after folding {folded_event_count} older event(s)."),
        format!(
            "Progress: total events={total}, recent window kept={COMPACTION_RECENT_EVENT_WINDOW}."
        ),
    ];
    if let Some(user) = last_user {
        summary.push(format!("Recent user request: {user}"));
    }
    if let Some(answer) = last_assistant {
        summary.push(format!("Recent assistant reply: {answer}"));
    }
    if !recent_tools.is_empty() {
        summary.push(format!(
            "Recent tools: {}",
            recent_tools
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !recent_commands.is_empty() {
        summary.push(format!(
            "Recent commands: {}",
            recent_commands
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    summary.push(
        "Next Steps: continue from the compacted tail; use /resume to browse history.".to_owned(),
    );
    summary.push("Critical Context: raw history remains in the session log.".to_owned());

    CompactionEvent {
        summary: summary.join("\n"),
        folded_event_count,
    }
}

pub fn summarize_branch_summary(
    source_session_id: impl Into<String>,
    folded_event_count: usize,
    events: &[Event],
) -> BranchSummaryEvent {
    let source_session_id = source_session_id.into();
    let start_index = latest_summary_event_index(events)
        .map(|index| index.saturating_add(1))
        .unwrap_or(0);
    let active = &events[start_index..];
    let total = active.len();
    let recent = active
        .iter()
        .rev()
        .take(COMPACTION_RECENT_DETAIL_WINDOW)
        .collect::<Vec<_>>();

    let last_user = recent.iter().find_map(|event| match &event.msg {
        EventMsg::UserMessage(message) => Some(message.content.as_str()),
        _ => None,
    });
    let last_assistant = recent.iter().find_map(|event| match &event.msg {
        EventMsg::AssistantMessage(message) => Some(message.content.as_str()),
        _ => None,
    });
    let recent_tools = recent
        .iter()
        .filter_map(|event| match &event.msg {
            EventMsg::ToolCall(call) => Some(call.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let recent_commands = recent
        .iter()
        .filter_map(|event| match &event.msg {
            EventMsg::CommandRun(command) => Some(command.command.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut summary = vec![
        format!(
            "Goal: continue the branch from {source_session_id} after folding {folded_event_count} older event(s)."
        ),
        format!("Progress: source tail events={total}, recent window kept={COMPACTION_RECENT_EVENT_WINDOW}."),
    ];
    if let Some(user) = last_user {
        summary.push(format!("Recent user request: {user}"));
    }
    if let Some(answer) = last_assistant {
        summary.push(format!("Recent assistant reply: {answer}"));
    }
    if !recent_tools.is_empty() {
        summary.push(format!(
            "Recent tools: {}",
            recent_tools
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !recent_commands.is_empty() {
        summary.push(format!(
            "Recent commands: {}",
            recent_commands
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    summary.push("Next Steps: continue from this branch summary and its active tail.".to_owned());
    summary.push(
        "Critical Context: the source branch history remains in the parent session log.".to_owned(),
    );

    BranchSummaryEvent {
        source_session_id,
        summary: summary.join("\n"),
        folded_event_count,
    }
}

pub fn count_active_events_since_summary(events: &[Event]) -> usize {
    let start_index = latest_summary_event_index(events)
        .map(|index| index.saturating_add(1))
        .unwrap_or(0);

    events[start_index..]
        .iter()
        .filter(|event| {
            !matches!(
                event.msg,
                EventMsg::SystemMessage(_) | EventMsg::Compaction(_) | EventMsg::BranchSummary(_)
            )
        })
        .count()
}

pub fn latest_summary_event_index(events: &[Event]) -> Option<usize> {
    events.iter().rposition(|event| is_summary_event(event))
}

pub fn is_summary_event(event: &Event) -> bool {
    matches!(
        event.msg,
        EventMsg::Compaction(_) | EventMsg::BranchSummary(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_wraps_id_and_payload() {
        let event = Event::new("turn-1", EventMsg::user("hello"));

        assert_eq!(event.id, "turn-1");
        assert_eq!(event.msg.label(), "you");
        assert_eq!(event.msg.content(), "hello");
    }

    #[test]
    fn error_event_exposes_message_content() {
        let msg = EventMsg::error("something failed");

        assert_eq!(msg.label(), "error");
        assert_eq!(msg.content(), "something failed");
    }

    #[test]
    fn assistant_event_exposes_label_and_content() {
        let msg = EventMsg::assistant("hello");

        assert_eq!(msg.label(), "assistant");
        assert_eq!(msg.content(), "hello");
    }

    #[test]
    fn image_attachment_event_exposes_summary() {
        let msg = EventMsg::image_attachment(
            "./shot.png",
            "shot.png",
            "image/png",
            12,
            "data:image/png;base64,AAAA",
        );

        assert_eq!(msg.label(), "image");
        assert!(msg.content().contains("attached image: shot.png"));
        assert!(msg.content().contains("image/png"));
    }

    #[test]
    fn tool_call_event_exposes_summary() {
        let msg = EventMsg::tool_call(ToolCall::new("call-0", "read", "path=Cargo.toml"));

        assert_eq!(msg.label(), "tool");
        assert_eq!(msg.content(), "read path=Cargo.toml");
    }

    #[test]
    fn tool_result_event_exposes_content() {
        let msg = EventMsg::tool_result(ToolResult {
            call_id: "call-0".to_owned(),
            status: ToolResultStatus::Success,
            content: "path: Cargo.toml".to_owned(),
            truncated: false,
            next_offset: None,
            edits: Vec::new(),
        });

        assert_eq!(msg.label(), "tool result");
        assert_eq!(msg.content(), "path: Cargo.toml");
    }

    #[test]
    fn file_edit_event_exposes_summary() {
        let msg = EventMsg::file_edit(FileEditEvent {
            action: FileEditAction::Applied,
            checkpoint: EditCheckpoint {
                checkpoint_id: "checkpoint-1".to_owned(),
                path: "src/main.rs".to_owned(),
                base_hash: "base".to_owned(),
                result_hash: "result".to_owned(),
                before_content: "old".to_owned(),
                after_content: "new".to_owned(),
                diff: "--- a/src/main.rs\n+++ b/src/main.rs".to_owned(),
            },
            rewound_checkpoint_id: None,
            summary: "applied edit to src/main.rs".to_owned(),
        });

        assert_eq!(msg.label(), "file edit");
        assert!(msg.content().contains("applied edit"));
    }
}
