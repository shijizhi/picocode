use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use crate::{
    event::{self, Event, EventMsg},
    model_picker::{ModelPickerAction, ModelPickerState},
    session::SessionSummary,
    session_picker::{SessionPickerAction, SessionPickerState},
    session_tree::{SessionTreeAction, SessionTreeState},
    submission::{LocalCommand, Op, Submission},
};

#[derive(Debug, Default)]
pub struct AppState {
    pub input: String,
    pub events: Vec<Event>,
    pub submissions: Vec<Submission>,
    pub pending_ai_requests: usize,
    pub runtime_status: RuntimeStatus,
    pub ai_profile: Option<AiProfile>,
    pub workspace_root: Option<String>,
    pub mode: AppMode,
    pub pending_resume_session: Option<String>,
    pub pending_new_session: bool,
    pending_submissions: Vec<Submission>,
    pub transcript_scroll: usize,
    next_event_seq: u64,
    next_submission_seq: u64,
    pub should_exit: bool,
}

impl AppState {
    pub fn new() -> Self {
        let mut state = Self::default();
        state.runtime_status = RuntimeStatus::idle();
        state
    }

    pub fn from_events(events: Vec<Event>) -> Self {
        let next_event_seq = next_event_seq(&events);
        Self {
            events,
            next_event_seq,
            runtime_status: RuntimeStatus::idle(),
            ai_profile: None,
            workspace_root: None,
            mode: AppMode::Chat,
            pending_resume_session: None,
            pending_new_session: false,
            ..Self::default()
        }
    }

    pub fn push_user_message(&mut self, content: impl Into<String>) {
        self.push_event(EventMsg::user(content));
    }

    pub fn push_error_message(&mut self, message: impl Into<String>) {
        self.push_event(EventMsg::error(message));
        self.scroll_to_bottom();
    }

    pub fn push_paste_text(&mut self, text: impl Into<String>) {
        self.input.push_str(&text.into());
    }

    pub fn push_event_msg(&mut self, msg: EventMsg) {
        self.push_event(msg);
        self.scroll_to_bottom();
    }

    pub fn enter_session_picker(&mut self, summaries: Vec<SessionSummary>) {
        self.mode = AppMode::SessionPicker(SessionPickerState::new(summaries));
        self.runtime_status = RuntimeStatus::with_detail("resume", "session picker");
    }

    pub fn enter_session_tree(&mut self, summaries: Vec<SessionSummary>) {
        self.mode = AppMode::SessionTree(SessionTreeState::new(summaries));
        self.runtime_status = RuntimeStatus::with_detail("tree", "session browser");
    }

    pub fn is_session_tree_active(&self) -> bool {
        matches!(self.mode, AppMode::SessionTree(_))
    }

    pub fn is_session_picker_active(&self) -> bool {
        matches!(self.mode, AppMode::SessionPicker(_))
    }

    pub fn enter_model_picker(
        &mut self,
        options: Vec<crate::config::ModelOption>,
        current_provider: &str,
        current_model: &str,
    ) {
        self.mode = AppMode::ModelPicker(ModelPickerState::new(
            options,
            current_provider,
            current_model,
        ));
        self.runtime_status = RuntimeStatus::with_detail("model", "selector");
    }

    pub fn is_model_picker_active(&self) -> bool {
        matches!(self.mode, AppMode::ModelPicker(_))
    }

    pub fn start_ai_request(&mut self) {
        self.pending_ai_requests = self.pending_ai_requests.saturating_add(1);
        self.runtime_status = RuntimeStatus::thinking();
    }

    pub fn finish_ai_request(&mut self) {
        self.pending_ai_requests = self.pending_ai_requests.saturating_sub(1);
        if self.pending_ai_requests == 0 {
            self.runtime_status = RuntimeStatus::idle();
        }
    }

    pub fn set_runtime_status(&mut self, status: RuntimeStatus) {
        self.runtime_status = status;
    }

    pub fn set_ai_profile(&mut self, profile: Option<AiProfile>) {
        self.ai_profile = profile;
    }

    pub fn set_workspace_root(&mut self, workspace_root: impl Into<String>) {
        self.workspace_root = Some(workspace_root.into());
    }

    pub fn take_pending_submissions(&mut self) -> Vec<Submission> {
        std::mem::take(&mut self.pending_submissions)
    }

    fn submit_user_input(&mut self, content: impl Into<String>) {
        let content = content.into();
        let submission = if let Some(command) = parse_local_command(&content) {
            self.next_submission(Op::local_command(command))
        } else {
            self.next_submission(Op::user_input(content))
        };
        if let Op::UserInput { .. } = &submission.op {
            if event::should_auto_compact(&self.events) {
                let compacted = event::summarize_compaction(&self.events);
                self.push_event_msg(EventMsg::compaction(compacted));
                self.runtime_status = RuntimeStatus::with_detail("compact", "continue");
            }
        }
        self.apply_submission(&submission);
        self.pending_submissions.push(submission.clone());
        self.submissions.push(submission);
    }

    pub fn push_system_message(&mut self, content: impl Into<String>) {
        self.push_event(EventMsg::system(content));
    }

    fn next_submission(&mut self, op: Op) -> Submission {
        let id = format!("sub-{}", self.next_submission_seq);
        self.next_submission_seq = self.next_submission_seq.saturating_add(1);
        Submission::new(id, op)
    }

    fn apply_submission(&mut self, submission: &Submission) {
        match &submission.op {
            Op::UserInput { content } => {
                self.push_user_message(content);
                self.scroll_to_bottom();
            }
            Op::LocalCommand { command } => {
                let label = match command {
                    LocalCommand::Resume => "/resume",
                    LocalCommand::New => "/new",
                    LocalCommand::Continue => "/continue",
                    LocalCommand::Image { .. } => return,
                    LocalCommand::ImageClipboard => return,
                    LocalCommand::Compact => "/compact",
                    LocalCommand::Export => "/export",
                    LocalCommand::Share => "/share",
                    LocalCommand::Capabilities => "/capabilities",
                    LocalCommand::Capability { query } => {
                        return self.push_system_message(format!("/capability {query}"));
                    }
                    LocalCommand::CapabilityEnable { query } => {
                        return self.push_system_message(format!("/cap-enable {query}"));
                    }
                    LocalCommand::CapabilityDisable { query } => {
                        return self.push_system_message(format!("/cap-disable {query}"));
                    }
                    LocalCommand::Skill { query } => {
                        return self.push_system_message(format!("/skill {query}"));
                    }
                    LocalCommand::Tree => "/tree",
                    LocalCommand::Fork => "/fork",
                    LocalCommand::Session { id } => {
                        return self.push_system_message(format!("/session {id}"));
                    }
                    LocalCommand::Model => "/model",
                };
                self.push_system_message(label);
                self.scroll_to_bottom();
            }
        }
    }

    pub fn handle_picker_key(&mut self, key: KeyEvent) -> Option<SessionPickerAction> {
        match &mut self.mode {
            AppMode::SessionPicker(state) => {
                let action = state.handle_key(key);
                match &action {
                    SessionPickerAction::Selected(session_id) => {
                        self.pending_resume_session = Some(session_id.clone());
                        self.should_exit = true;
                    }
                    SessionPickerAction::Cancelled => {
                        self.mode = AppMode::Chat;
                        self.runtime_status = RuntimeStatus::idle();
                    }
                    SessionPickerAction::RenameRequested { .. }
                    | SessionPickerAction::DeleteRequested { .. }
                    | SessionPickerAction::Continue => {}
                }
                Some(action)
            }
            AppMode::Chat | AppMode::ModelPicker(_) | AppMode::SessionTree(_) => None,
        }
    }

    pub fn handle_model_key(&mut self, key: KeyEvent) -> Option<ModelPickerAction> {
        match &mut self.mode {
            AppMode::ModelPicker(state) => {
                let action = state.handle_key(key);
                match &action {
                    ModelPickerAction::Selected(_) => {
                        self.runtime_status = RuntimeStatus::with_detail("model", "selected");
                    }
                    ModelPickerAction::Cancelled => {
                        self.mode = AppMode::Chat;
                        self.runtime_status = RuntimeStatus::idle();
                    }
                    ModelPickerAction::Continue => {}
                }
                Some(action)
            }
            AppMode::Chat | AppMode::SessionPicker(_) | AppMode::SessionTree(_) => None,
        }
    }

    pub fn handle_tree_key(&mut self, key: KeyEvent) -> Option<SessionTreeAction> {
        match &mut self.mode {
            AppMode::SessionTree(state) => {
                let action = state.handle_key(key);
                match &action {
                    SessionTreeAction::Selected(session_id) => {
                        self.pending_resume_session = Some(session_id.clone());
                        self.should_exit = true;
                    }
                    SessionTreeAction::Cancelled => {
                        self.mode = AppMode::Chat;
                        self.runtime_status = RuntimeStatus::idle();
                    }
                    SessionTreeAction::RenameRequested { .. }
                    | SessionTreeAction::DeleteRequested { .. }
                    | SessionTreeAction::ForkRequested { .. }
                    | SessionTreeAction::Continue => {}
                }
                Some(action)
            }
            AppMode::Chat | AppMode::SessionPicker(_) | AppMode::ModelPicker(_) => None,
        }
    }

    fn push_event(&mut self, msg: EventMsg) {
        let id = format!("evt-{}", self.next_event_seq);
        self.next_event_seq = self.next_event_seq.saturating_add(1);
        self.events.push(Event::new(id, msg));
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_exit = true;
            }
            KeyCode::Esc => {
                self.should_exit = true;
            }
            KeyCode::Enter => {
                self.submit_input();
            }
            KeyCode::Up => {
                self.scroll_transcript_up(1);
            }
            KeyCode::Down => {
                self.scroll_transcript_down(1);
            }
            KeyCode::PageUp => {
                self.scroll_transcript_up(10);
            }
            KeyCode::PageDown => {
                self.scroll_transcript_down(10);
            }
            KeyCode::Home => {
                self.scroll_to_top();
            }
            KeyCode::End => {
                self.scroll_to_bottom();
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.push(character);
            }
            _ => {}
        }
    }

    fn submit_input(&mut self) {
        let trimmed = self.input.trim();
        if should_quit(trimmed) {
            self.should_exit = true;
            self.input.clear();
            return;
        }

        if !trimmed.is_empty() {
            self.submit_user_input(trimmed.to_owned());
        }

        self.input.clear();
    }

    fn scroll_transcript_up(&mut self, amount: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_add(amount);
    }

    fn scroll_transcript_down(&mut self, amount: usize) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(amount);
    }

    fn scroll_to_top(&mut self) {
        self.transcript_scroll = usize::MAX;
    }

    fn scroll_to_bottom(&mut self) {
        self.transcript_scroll = 0;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    Chat,
    SessionPicker(SessionPickerState),
    SessionTree(SessionTreeState),
    ModelPicker(ModelPickerState),
}

impl Default for AppMode {
    fn default() -> Self {
        Self::Chat
    }
}

fn parse_local_command(input: &str) -> Option<LocalCommand> {
    let trimmed = input.trim();
    if trimmed == "/resume" {
        return Some(LocalCommand::Resume);
    }
    if trimmed == "/new" {
        return Some(LocalCommand::New);
    }
    if trimmed == "/continue" || trimmed == "-c" {
        return Some(LocalCommand::Continue);
    }
    if let Some(rest) = trimmed.strip_prefix("/image ") {
        let path = rest.trim();
        if !path.is_empty() {
            if path.eq_ignore_ascii_case("clip") || path.eq_ignore_ascii_case("clipboard") {
                return Some(LocalCommand::ImageClipboard);
            }
            return Some(LocalCommand::Image {
                path: path.to_owned(),
            });
        }
    }
    if trimmed == "/compact" {
        return Some(LocalCommand::Compact);
    }
    if trimmed == "/export" {
        return Some(LocalCommand::Export);
    }
    if trimmed == "/share" {
        return Some(LocalCommand::Share);
    }
    if trimmed == "/capabilities" {
        return Some(LocalCommand::Capabilities);
    }
    if let Some(rest) = trimmed.strip_prefix("/capability ") {
        let query = rest.trim();
        if !query.is_empty() {
            return Some(LocalCommand::Capability {
                query: query.to_owned(),
            });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/cap-enable ") {
        let query = rest.trim();
        if !query.is_empty() {
            return Some(LocalCommand::CapabilityEnable {
                query: query.to_owned(),
            });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/cap-disable ") {
        let query = rest.trim();
        if !query.is_empty() {
            return Some(LocalCommand::CapabilityDisable {
                query: query.to_owned(),
            });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/skill ") {
        let query = rest.trim();
        if !query.is_empty() {
            return Some(LocalCommand::Skill {
                query: query.to_owned(),
            });
        }
    }
    if let Some(rest) = trimmed.strip_prefix("/session ") {
        let id = rest.trim();
        if !id.is_empty() {
            return Some(LocalCommand::Session { id: id.to_owned() });
        }
    }
    if trimmed == "/tree" {
        return Some(LocalCommand::Tree);
    }
    if trimmed == "/fork" {
        return Some(LocalCommand::Fork);
    }
    if trimmed == "/model" {
        return Some(LocalCommand::Model);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AiProfile {
    pub provider: String,
    pub model: String,
}

impl AiProfile {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }

    pub fn label(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub label: String,
    pub detail: Option<String>,
    pub is_active: bool,
    pub started_at: Option<Instant>,
}

impl RuntimeStatus {
    pub fn idle() -> Self {
        Self {
            label: "idle".to_owned(),
            detail: None,
            is_active: false,
            started_at: None,
        }
    }

    pub fn thinking() -> Self {
        Self {
            label: "thinking".to_owned(),
            detail: None,
            is_active: true,
            started_at: Some(Instant::now()),
        }
    }

    pub fn with_detail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: Some(detail.into()),
            is_active: true,
            started_at: Some(Instant::now()),
        }
    }
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self::idle()
    }
}

fn next_event_seq(events: &[Event]) -> u64 {
    events
        .iter()
        .filter_map(|event| event.id.strip_prefix("evt-")?.parse::<u64>().ok())
        .max()
        .map(|value| value.saturating_add(1))
        .unwrap_or(0)
}

pub fn should_quit(input: &str) -> bool {
    matches!(input.trim(), "/quit" | "/exit" | ":q")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_starts_with_intro_messages() {
        let app = AppState::new();

        assert!(app.events.is_empty());
    }

    #[test]
    fn quit_commands_are_recognized() {
        assert!(should_quit("/quit"));
        assert!(should_quit("/exit"));
        assert!(should_quit(":q"));
        assert!(!should_quit("hello"));
    }

    #[test]
    fn enter_records_user_input() {
        let mut app = AppState::new();
        app.input = "hello ratatui".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(app.events.last().unwrap().msg.content(), "hello ratatui");
        assert!(app.input.is_empty());
        assert!(!app.should_exit);
    }

    #[test]
    fn enter_with_quit_command_exits_without_recording_message() {
        let mut app = AppState::new();
        app.input = "/quit".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        assert!(app.events.is_empty());
        assert!(app.input.is_empty());
        assert!(app.should_exit);
    }

    #[test]
    fn control_character_input_is_ignored() {
        let mut app = AppState::new();

        app.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));

        assert!(app.input.is_empty());
        assert!(!app.should_exit);
    }

    #[test]
    fn transcript_scrolls_up_and_down() {
        let mut app = AppState::new();

        app.handle_key(KeyEvent::from(KeyCode::Up));
        app.handle_key(KeyEvent::from(KeyCode::Up));
        assert_eq!(app.transcript_scroll, 2);

        app.handle_key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.transcript_scroll, 1);
    }

    #[test]
    fn submitting_input_returns_to_bottom() {
        let mut app = AppState::new();
        app.handle_key(KeyEvent::from(KeyCode::PageUp));
        app.input = "new message".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(app.transcript_scroll, 0);
    }

    #[test]
    fn events_get_stable_sequential_ids() {
        let mut app = AppState::default();

        app.push_user_message("first");
        app.push_user_message("second");

        assert_eq!(app.events[0].id, "evt-0");
        assert_eq!(app.events[1].id, "evt-1");
    }

    #[test]
    fn submitted_input_creates_submission_and_event() {
        let mut app = AppState::new();
        app.input = "hello submission".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        assert_eq!(app.submissions.len(), 1);
        assert_eq!(app.submissions[0].id, "sub-0");
        assert!(matches!(app.submissions[0].op, Op::UserInput { .. }));
        assert_eq!(app.events.last().unwrap().msg.content(), "hello submission");
    }

    #[test]
    fn slash_resume_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/resume".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Resume
            }
        ));
    }

    #[test]
    fn slash_new_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/new".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::New
            }
        ));
    }

    #[test]
    fn slash_continue_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/continue".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Continue
            }
        ));
    }

    #[test]
    fn slash_image_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/image screenshot.png".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Image { .. }
            }
        ));
    }

    #[test]
    fn slash_image_clipboard_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/image clipboard".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::ImageClipboard
            }
        ));
    }

    #[test]
    fn slash_image_clip_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/image clip".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::ImageClipboard
            }
        ));
    }

    #[test]
    fn slash_model_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/model".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Model
            }
        ));
    }

    #[test]
    fn slash_capabilities_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/capabilities".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Capabilities
            }
        ));
    }

    #[test]
    fn slash_capability_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/capability usage".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Capability { .. }
            }
        ));
    }

    #[test]
    fn slash_cap_enable_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/cap-enable usage".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::CapabilityEnable { .. }
            }
        ));
    }

    #[test]
    fn slash_cap_disable_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/cap-disable usage".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::CapabilityDisable { .. }
            }
        ));
    }

    #[test]
    fn slash_skill_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/skill usage".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Skill { .. }
            }
        ));
    }

    #[test]
    fn slash_tree_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/tree".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Tree
            }
        ));
    }

    #[test]
    fn slash_fork_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/fork".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Fork
            }
        ));
    }

    #[test]
    fn slash_compact_creates_local_command_submission() {
        let mut app = AppState::new();
        app.input = "/compact".to_owned();

        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].op,
            Op::LocalCommand {
                command: LocalCommand::Compact
            }
        ));
    }

    #[test]
    fn pending_submissions_can_be_drained_once() {
        let mut app = AppState::new();
        app.input = "hello pending".to_owned();
        app.handle_key(KeyEvent::from(KeyCode::Enter));

        let pending = app.take_pending_submissions();

        assert_eq!(pending.len(), 1);
        assert!(app.take_pending_submissions().is_empty());
    }

    #[test]
    fn assistant_and_error_events_are_recorded() {
        let mut app = AppState::default();

        app.push_event_msg(EventMsg::assistant("hi"));
        app.push_error_message("oops");

        assert!(matches!(app.events[0].msg, EventMsg::AssistantMessage(_)));
        assert!(matches!(app.events[1].msg, EventMsg::Error(_)));
    }

    #[test]
    fn ai_request_count_tracks_pending_work() {
        let mut app = AppState::default();

        app.start_ai_request();
        app.start_ai_request();
        app.finish_ai_request();
        app.finish_ai_request();
        app.finish_ai_request();

        assert_eq!(app.pending_ai_requests, 0);
    }

    #[test]
    fn restored_app_continues_event_ids() {
        let events = vec![Event::new("evt-3", EventMsg::user("hello"))];
        let mut app = AppState::from_events(events);

        app.push_event_msg(EventMsg::assistant("hi"));

        assert_eq!(app.events.last().unwrap().id, "evt-4");
    }

    #[test]
    fn runtime_status_transitions_from_idle_to_thinking_and_back() {
        let mut app = AppState::new();

        assert!(!app.runtime_status.is_active);
        assert_eq!(app.runtime_status.label, "idle");

        app.start_ai_request();
        assert!(app.runtime_status.is_active);
        assert_eq!(app.runtime_status.label, "thinking");

        app.finish_ai_request();
        assert!(!app.runtime_status.is_active);
        assert_eq!(app.runtime_status.label, "idle");
    }

    #[test]
    fn runtime_status_can_include_detail() {
        let status = RuntimeStatus::with_detail("loading", "config");

        assert!(status.is_active);
        assert_eq!(status.label, "loading");
        assert_eq!(status.detail.as_deref(), Some("config"));
    }
}
