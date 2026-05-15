use crate::{
    ai::ToolSpec,
    event::{CommandOutputEvent, CommandRunEvent, EventMsg, FileEditEvent},
    instruction::{CommandApproval, ProjectCommandConfig},
    workspace::{
        EditBatchPreviewResult, EditCheckpoint, EditPreviewResult, EditRequest, FindResult,
        GrepResult, ListResult, ReadResult, Workspace, WorkspaceError,
    },
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: ToolInputSchema,
    pub permission: PermissionKind,
}

impl ToolDefinition {
    pub fn to_ai_spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema_json: self.input_schema.to_json(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInputSchema {
    pub properties: Vec<ToolInputProperty>,
    pub required: Vec<String>,
}

impl ToolInputSchema {
    pub fn object(properties: Vec<ToolInputProperty>, required: Vec<&str>) -> Self {
        Self {
            properties,
            required: required.into_iter().map(str::to_owned).collect(),
        }
    }

    pub fn to_json(&self) -> String {
        let properties = self
            .properties
            .iter()
            .map(ToolInputProperty::to_json_entry)
            .collect::<Vec<_>>()
            .join(",");
        let required = self
            .required
            .iter()
            .map(|field| format!("\"{}\"", json_escape(field)))
            .collect::<Vec<_>>()
            .join(",");

        format!(
            "{{\"type\":\"object\",\"properties\":{{{properties}}},\"required\":[{required}],\"additionalProperties\":false}}"
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInputProperty {
    pub name: String,
    pub kind: ToolInputKind,
    pub description: String,
}

impl ToolInputProperty {
    pub fn string(name: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: ToolInputKind::String,
            description: description.to_owned(),
        }
    }

    pub fn number(name: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: ToolInputKind::Number,
            description: description.to_owned(),
        }
    }

    pub fn boolean(name: &str, description: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind: ToolInputKind::Boolean,
            description: description.to_owned(),
        }
    }

    fn to_json_entry(&self) -> String {
        format!(
            "\"{}\":{{\"type\":\"{}\",\"description\":\"{}\"}}",
            json_escape(&self.name),
            self.kind.as_json_type(),
            json_escape(&self.description)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToolInputKind {
    String,
    Number,
    Boolean,
}

impl ToolInputKind {
    fn as_json_type(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum PermissionKind {
    Read,
    Write,
    Execute,
    Network,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl ToolCall {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
        }
    }

    pub fn summary(&self) -> String {
        let args = self
            .arguments
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if args.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.name, args)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub status: ToolResultStatus,
    pub content: String,
    pub truncated: bool,
    pub next_offset: Option<usize>,
    pub edits: Vec<FileEditEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRuntimeOptions {
    pub command: CommandRuntimeOptions,
}

impl Default for ToolRuntimeOptions {
    fn default() -> Self {
        Self {
            command: CommandRuntimeOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRuntimeOptions {
    pub approval: CommandApproval,
    pub timeout_seconds: u64,
}

impl Default for CommandRuntimeOptions {
    fn default() -> Self {
        Self {
            approval: CommandApproval::OnRisky,
            timeout_seconds: 120,
        }
    }
}

impl ToolResult {
    fn success(call_id: impl Into<String>, content: String, truncated: bool) -> Self {
        Self {
            call_id: call_id.into(),
            status: if truncated {
                ToolResultStatus::Truncated
            } else {
                ToolResultStatus::Success
            },
            content,
            truncated,
            next_offset: None,
            edits: Vec::new(),
        }
    }

    fn read_success(call_id: impl Into<String>, result: ReadResult) -> Self {
        Self {
            call_id: call_id.into(),
            status: if result.truncated {
                ToolResultStatus::Truncated
            } else {
                ToolResultStatus::Success
            },
            content: format_read_result(&result),
            truncated: result.truncated,
            next_offset: result.next_offset,
            edits: Vec::new(),
        }
    }

    fn error(call_id: impl Into<String>, error: impl ToString) -> Self {
        Self {
            call_id: call_id.into(),
            status: ToolResultStatus::Error,
            content: error.to_string(),
            truncated: false,
            next_offset: None,
            edits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ToolResultStatus {
    Success,
    Error,
    Denied,
    Truncated,
}

impl ToolResultStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Error => "error",
            Self::Denied => "denied",
            Self::Truncated => "truncated",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "success" => Some(Self::Success),
            "error" => Some(Self::Error),
            "denied" => Some(Self::Denied),
            "truncated" => Some(Self::Truncated),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EditJournal {
    checkpoints_by_path: HashMap<String, Vec<EditCheckpoint>>,
}

impl EditJournal {
    fn from_events(events: &[crate::event::Event]) -> Self {
        let mut journal = Self::default();
        for event in events {
            match &event.msg {
                crate::event::EventMsg::FileEdit(edit) => journal.record(edit),
                crate::event::EventMsg::ToolResult(result) => {
                    for edit in &result.edits {
                        journal.record(edit);
                    }
                }
                _ => {}
            }
        }
        journal
    }

    fn record(&mut self, edit: &FileEditEvent) {
        match edit.action {
            crate::event::FileEditAction::Applied => {
                if checkpoint_is_complete(&edit.checkpoint) {
                    let stack = self
                        .checkpoints_by_path
                        .entry(edit.checkpoint.path.clone())
                        .or_default();
                    if !stack
                        .iter()
                        .any(|checkpoint| checkpoint.checkpoint_id == edit.checkpoint.checkpoint_id)
                    {
                        stack.push(edit.checkpoint.clone());
                    }
                }
            }
            crate::event::FileEditAction::RolledBack => {
                if let Some(target_id) = &edit.rewound_checkpoint_id {
                    if let Some(stack) = self.checkpoints_by_path.get_mut(&edit.checkpoint.path) {
                        if let Some(position) = stack
                            .iter()
                            .rposition(|checkpoint| &checkpoint.checkpoint_id == target_id)
                        {
                            stack.remove(position);
                        }
                    }
                }
            }
        }
    }

    fn latest_checkpoint_for_path(&self, path: &str) -> Option<EditCheckpoint> {
        self.checkpoints_by_path
            .get(path)
            .and_then(|stack| stack.last().cloned())
    }
}

fn checkpoint_is_complete(checkpoint: &EditCheckpoint) -> bool {
    !checkpoint.checkpoint_id.is_empty()
        && !checkpoint.path.is_empty()
        && !checkpoint.base_hash.is_empty()
        && !checkpoint.result_hash.is_empty()
}

#[derive(Debug, Clone)]
pub struct ToolRuntime {
    workspace: Workspace,
    edit_journal: Arc<Mutex<EditJournal>>,
    options: ToolRuntimeOptions,
    command_event_tx: Option<mpsc::Sender<EventMsg>>,
}

impl ToolRuntime {
    #[allow(dead_code)]
    pub fn new(workspace: Workspace) -> Self {
        Self::with_options(workspace, ToolRuntimeOptions::default())
    }

    pub fn with_options(workspace: Workspace, options: ToolRuntimeOptions) -> Self {
        Self {
            workspace,
            edit_journal: Arc::new(Mutex::new(EditJournal::default())),
            options,
            command_event_tx: None,
        }
    }

    pub fn with_event_sender(
        workspace: Workspace,
        options: ToolRuntimeOptions,
        command_event_tx: mpsc::Sender<EventMsg>,
    ) -> Self {
        Self {
            workspace,
            edit_journal: Arc::new(Mutex::new(EditJournal::default())),
            options,
            command_event_tx: Some(command_event_tx),
        }
    }

    pub fn with_project_config(
        workspace: Workspace,
        project_config: Option<ProjectCommandConfig>,
    ) -> Self {
        let mut options = ToolRuntimeOptions::default();
        if let Some(config) = project_config {
            if let Some(approval) = config.approval {
                options.command.approval = approval;
            }
            if let Some(timeout_seconds) = config.timeout {
                options.command.timeout_seconds = timeout_seconds.max(1);
            }
        }
        Self::with_options(workspace, options)
    }

    #[allow(dead_code)]
    pub fn from_events(workspace: Workspace, events: &[crate::event::Event]) -> Self {
        let runtime = Self::new(workspace);
        runtime.seed_edit_journal(events);
        runtime
    }

    #[allow(dead_code)]
    pub fn from_events_with_project_config(
        workspace: Workspace,
        events: &[crate::event::Event],
        project_config: Option<ProjectCommandConfig>,
    ) -> Self {
        let runtime = Self::with_project_config(workspace, project_config);
        runtime.seed_edit_journal(events);
        runtime
    }

    pub fn from_events_with_project_config_and_sender(
        workspace: Workspace,
        events: &[crate::event::Event],
        project_config: Option<ProjectCommandConfig>,
        command_event_tx: mpsc::Sender<EventMsg>,
    ) -> Self {
        let runtime = Self::with_event_sender(
            workspace,
            ToolRuntimeOptions {
                command: project_config
                    .as_ref()
                    .map(|config| CommandRuntimeOptions {
                        approval: config.approval.clone().unwrap_or(CommandApproval::OnRisky),
                        timeout_seconds: config.timeout.unwrap_or(120).max(1),
                    })
                    .unwrap_or_default(),
            },
            command_event_tx,
        );
        runtime.seed_edit_journal(events);
        runtime
    }

    #[allow(dead_code)]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ls_definition(),
            read_definition(),
            find_files_definition(),
            search_text_definition(),
            propose_edit_definition(),
            propose_edit_batch_definition(),
            apply_patch_definition(),
            apply_patch_batch_definition(),
            rewind_edit_definition(),
            run_command_definition(),
        ]
    }

    pub fn tool_specs(&self) -> Vec<ToolSpec> {
        self.definitions()
            .into_iter()
            .map(|definition| definition.to_ai_spec())
            .collect()
    }

    pub fn execute(&self, call: ToolCall) -> ToolResult {
        match call.name.as_str() {
            "ls" => self.execute_ls(call),
            "read" => self.execute_read(call),
            "find_files" | "find" => self.execute_find_files(call),
            "search_text" | "grep" => self.execute_search_text(call),
            "propose_edit" => self.execute_propose_edit(call),
            "propose_edit_batch" => self.execute_propose_edit_batch(call),
            "apply_patch" => self.execute_apply_patch(call),
            "apply_patch_batch" => self.execute_apply_patch_batch(call),
            "rewind_edit" | "rollback_edit" => self.execute_rewind_edit(call),
            "run_command" => self.execute_run_command(call),
            _ => ToolResult::error(call.id, format!("unknown tool: {}", call.name)),
        }
    }

    fn execute_ls(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let path = args.value("path").unwrap_or(".");
        let limit = args.usize("limit").unwrap_or(200);

        match self.workspace.list(path, limit) {
            Ok(result) => {
                ToolResult::success(call.id, format_list_result(&result), result.truncated)
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_read(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(path) = args.value("path") else {
            return ToolResult::error(call.id, "missing required argument: path");
        };
        let offset = args.usize("offset").unwrap_or(0);
        let limit = args.usize("limit").unwrap_or(4000);

        match self.workspace.read(path, offset, limit) {
            Ok(result) => ToolResult::read_success(call.id, result),
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_find_files(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(query) = args.value("query") else {
            return ToolResult::error(call.id, "missing required argument: query");
        };
        let path = args.value("path").unwrap_or(".");
        let limit = args.usize("limit").unwrap_or(100);

        match self.workspace.find(query, path, limit) {
            Ok(result) => {
                ToolResult::success(call.id, format_find_result(&result), result.truncated)
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_search_text(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(query) = args.value("query") else {
            return ToolResult::error(call.id, "missing required argument: query");
        };
        if query.trim().is_empty() {
            return ToolResult::error(call.id, "query must not be empty");
        }
        let path = args.value("path").unwrap_or(".");
        let limit = args.usize("limit").unwrap_or(100);
        let ignore_case = args.bool("ignore_case").unwrap_or(false);

        match self.workspace.grep(query, path, limit, ignore_case) {
            Ok(result) => {
                ToolResult::success(call.id, format_grep_result(&result), result.truncated)
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_propose_edit(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(path) = args.value("path") else {
            return ToolResult::error(call.id, "missing required argument: path");
        };
        let Some(find) = args.value("find") else {
            return ToolResult::error(call.id, "missing required argument: find");
        };
        let Some(replace) = args.value("replace") else {
            return ToolResult::error(call.id, "missing required argument: replace");
        };

        match self.workspace.propose_edit(path, find, replace) {
            Ok(result) => ToolResult::success(call.id, format_edit_preview_result(&result), false),
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_propose_edit_batch(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(edits_json) = args.value("edits_json") else {
            return ToolResult::error(call.id, "missing required argument: edits_json");
        };
        let edits = match parse_edit_requests_json(edits_json) {
            Ok(edits) => edits,
            Err(error) => return ToolResult::error(call.id, error),
        };

        match self.workspace.propose_edits(&edits) {
            Ok(result) => {
                ToolResult::success(call.id, format_edit_batch_preview_result(&result), false)
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_apply_patch(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(path) = args.value("path") else {
            return ToolResult::error(call.id, "missing required argument: path");
        };
        let Some(find) = args.value("find") else {
            return ToolResult::error(call.id, "missing required argument: find");
        };
        let Some(replace) = args.value("replace") else {
            return ToolResult::error(call.id, "missing required argument: replace");
        };

        match self.workspace.apply_edit(path, find, replace) {
            Ok(result) => {
                let edit = FileEditEvent::from_edit_apply_result(result);
                self.record_edit(&edit);
                ToolResult {
                    call_id: call.id,
                    status: ToolResultStatus::Success,
                    content: format_edit_apply_result(&edit),
                    truncated: false,
                    next_offset: None,
                    edits: vec![edit],
                }
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_apply_patch_batch(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(edits_json) = args.value("edits_json") else {
            return ToolResult::error(call.id, "missing required argument: edits_json");
        };
        let edits = match parse_edit_requests_json(edits_json) {
            Ok(edits) => edits,
            Err(error) => return ToolResult::error(call.id, error),
        };

        match self.workspace.apply_edits(&edits) {
            Ok(result) => {
                let file_edits = result
                    .edits
                    .into_iter()
                    .map(|checkpoint| {
                        FileEditEvent::from_edit_apply_result(crate::workspace::EditApplyResult {
                            checkpoint,
                        })
                    })
                    .collect::<Vec<_>>();
                for edit in &file_edits {
                    self.record_edit(edit);
                }
                ToolResult {
                    call_id: call.id,
                    status: ToolResultStatus::Success,
                    content: format_edit_batch_apply_result(&file_edits),
                    truncated: false,
                    next_offset: None,
                    edits: file_edits,
                }
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_rewind_edit(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(path) = args.value("path") else {
            return ToolResult::error(call.id, "missing required argument: path");
        };

        let Some(checkpoint) = self.latest_checkpoint_for_path(path) else {
            return ToolResult::error(call.id, format!("no checkpoint available for path: {path}"));
        };

        match self.workspace.rewind_edit(&checkpoint) {
            Ok(result) => {
                let edit = FileEditEvent::from_edit_rewind_result(result);
                self.record_edit(&edit);
                ToolResult {
                    call_id: call.id,
                    status: ToolResultStatus::Success,
                    content: format_edit_rewind_result(&edit),
                    truncated: false,
                    next_offset: None,
                    edits: vec![edit],
                }
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn execute_run_command(&self, call: ToolCall) -> ToolResult {
        let args = ToolArgs::parse(&call.arguments);
        let Some(command) = args.value("command") else {
            return ToolResult::error(call.id, "missing required argument: command");
        };
        let cwd = args.value("cwd").unwrap_or(".");
        let timeout_seconds = args
            .usize("timeout_seconds")
            .map(|value| value.max(1) as u64)
            .unwrap_or(self.options.command.timeout_seconds.max(1));

        if let Err(reason) = self.command_is_allowed(command) {
            return ToolResult::error(call.id, reason);
        }

        let cwd = match self.resolve_command_cwd(cwd) {
            Ok(path) => path,
            Err(error) => return ToolResult::error(call.id, error),
        };

        match execute_shell_command(
            command,
            &cwd,
            Duration::from_secs(timeout_seconds),
            self.command_event_tx.clone(),
            call.id.clone(),
        ) {
            Ok(result) => {
                let status = if result.timed_out || result.exit_code != Some(0) {
                    ToolResultStatus::Error
                } else if result.truncated {
                    ToolResultStatus::Truncated
                } else {
                    ToolResultStatus::Success
                };
                ToolResult {
                    call_id: call.id,
                    status,
                    content: format_command_result(&result),
                    truncated: result.truncated,
                    next_offset: None,
                    edits: Vec::new(),
                }
            }
            Err(error) => ToolResult::error(call.id, error),
        }
    }

    fn seed_edit_journal(&self, events: &[crate::event::Event]) {
        let mut journal = self
            .edit_journal
            .lock()
            .expect("edit journal mutex should not be poisoned");
        *journal = EditJournal::from_events(events);
    }

    fn record_edit(&self, edit: &FileEditEvent) {
        let mut journal = self
            .edit_journal
            .lock()
            .expect("edit journal mutex should not be poisoned");
        journal.record(edit);
    }

    fn latest_checkpoint_for_path(&self, path: &str) -> Option<EditCheckpoint> {
        let journal = self
            .edit_journal
            .lock()
            .expect("edit journal mutex should not be poisoned");
        journal.latest_checkpoint_for_path(path)
    }

    fn command_is_allowed(&self, command: &str) -> Result<(), String> {
        match &self.options.command.approval {
            CommandApproval::Always => Ok(()),
            CommandApproval::Never => {
                Err("command execution is disabled by project config".to_owned())
            }
            CommandApproval::OnRisky => {
                if let Some(reason) = command_risk_reason(command) {
                    Err(format!("command requires approval: {reason}"))
                } else {
                    Ok(())
                }
            }
        }
    }

    fn resolve_command_cwd(&self, cwd: &str) -> Result<PathBuf, String> {
        let requested = if cwd.trim().is_empty() || cwd == "." {
            self.workspace.root().to_path_buf()
        } else {
            let candidate = self.workspace.root().join(cwd);
            candidate.canonicalize().map_err(|error| {
                format!(
                    "failed to resolve command cwd '{}': {}",
                    candidate.display(),
                    error
                )
            })?
        };

        if !requested.starts_with(self.workspace.root()) {
            return Err(format!(
                "command cwd must stay inside workspace root: {}",
                requested.display()
            ));
        }

        Ok(requested)
    }
}

fn ls_definition() -> ToolDefinition {
    ToolDefinition {
        name: "ls".to_owned(),
        description: "List entries in a workspace directory.".to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("path", "Workspace directory to list."),
                ToolInputProperty::number("limit", "Maximum number of entries to return."),
            ],
            vec!["path"],
        ),
        permission: PermissionKind::Read,
    }
}

fn read_definition() -> ToolDefinition {
    ToolDefinition {
        name: "read".to_owned(),
        description: "Read a UTF-8 text file from the workspace.".to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("path", "Workspace file to read."),
                ToolInputProperty::number("offset", "Character offset to start reading from."),
                ToolInputProperty::number("limit", "Maximum number of characters to return."),
            ],
            vec!["path"],
        ),
        permission: PermissionKind::Read,
    }
}

fn find_files_definition() -> ToolDefinition {
    ToolDefinition {
        name: "find_files".to_owned(),
        description: "Recursively find workspace files and directories by path substring."
            .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string(
                    "query",
                    "Case-insensitive substring to match against workspace-relative paths.",
                ),
                ToolInputProperty::string("path", "Workspace directory to search from."),
                ToolInputProperty::number("limit", "Maximum number of matches to return."),
            ],
            vec!["query"],
        ),
        permission: PermissionKind::Read,
    }
}

fn search_text_definition() -> ToolDefinition {
    ToolDefinition {
        name: "search_text".to_owned(),
        description: "Search UTF-8 workspace files for literal text and return matching lines."
            .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("query", "Literal text to search for."),
                ToolInputProperty::string("path", "Workspace file or directory to search from."),
                ToolInputProperty::number("limit", "Maximum number of matching lines to return."),
                ToolInputProperty::boolean(
                    "ignore_case",
                    "Whether to match text case-insensitively.",
                ),
            ],
            vec!["query"],
        ),
        permission: PermissionKind::Read,
    }
}

fn propose_edit_definition() -> ToolDefinition {
    ToolDefinition {
        name: "propose_edit".to_owned(),
        description:
            "Preview a deterministic text edit as a diff without changing workspace files."
                .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("path", "Workspace file to preview for editing."),
                ToolInputProperty::string(
                    "find",
                    "Exact text to search for in the current file content.",
                ),
                ToolInputProperty::string("replace", "Replacement text to preview."),
            ],
            vec!["path", "find", "replace"],
        ),
        permission: PermissionKind::Read,
    }
}

fn propose_edit_batch_definition() -> ToolDefinition {
    ToolDefinition {
        name: "propose_edit_batch".to_owned(),
        description:
            "Preview a batch of deterministic text edits as diffs without changing workspace files. The preview includes a base_hash for each file."
                .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![ToolInputProperty::string(
                "edits_json",
                "JSON array string containing batch edit objects with path, find, and replace.",
            )],
            vec!["edits_json"],
        ),
        permission: PermissionKind::Read,
    }
}

fn apply_patch_definition() -> ToolDefinition {
    ToolDefinition {
        name: "apply_patch".to_owned(),
        description:
            "Apply a deterministic text edit to a workspace file and record a reversible checkpoint."
                .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("path", "Workspace file to edit."),
                ToolInputProperty::string(
                    "find",
                    "Exact text to replace in the current file content.",
                ),
                ToolInputProperty::string("replace", "Replacement text to write into the file."),
            ],
            vec!["path", "find", "replace"],
        ),
        permission: PermissionKind::Write,
    }
}

fn apply_patch_batch_definition() -> ToolDefinition {
    ToolDefinition {
        name: "apply_patch_batch".to_owned(),
        description:
            "Apply a batch of deterministic text edits and record reversible checkpoints for each file. Optional expected_hash values enable conflict detection against the previewed base_hash."
                .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![ToolInputProperty::string(
                "edits_json",
                "JSON array string containing batch edit objects with path, find, and replace.",
            )],
            vec!["edits_json"],
        ),
        permission: PermissionKind::Write,
    }
}

fn rewind_edit_definition() -> ToolDefinition {
    ToolDefinition {
        name: "rewind_edit".to_owned(),
        description: "Rewind the latest reversible checkpoint for a workspace file.".to_owned(),
        input_schema: ToolInputSchema::object(
            vec![ToolInputProperty::string(
                "path",
                "Workspace file whose latest reversible checkpoint should be rewound.",
            )],
            vec!["path"],
        ),
        permission: PermissionKind::Write,
    }
}

fn run_command_definition() -> ToolDefinition {
    ToolDefinition {
        name: "run_command".to_owned(),
        description:
            "Run a shell command in the workspace root, capture stdout/stderr, and return the exit status."
                .to_owned(),
        input_schema: ToolInputSchema::object(
            vec![
                ToolInputProperty::string("command", "Shell command to run."),
                ToolInputProperty::string(
                    "cwd",
                    "Optional workspace-relative working directory for the command.",
                ),
                ToolInputProperty::number(
                    "timeout_seconds",
                    "Optional timeout in seconds; falls back to project command.timeout when omitted.",
                ),
            ],
            vec!["command"],
        ),
        permission: PermissionKind::Execute,
    }
}

fn format_list_result(result: &ListResult) -> String {
    let header = if result.path.is_empty() {
        ".".to_owned()
    } else {
        result.path.clone()
    };
    let mut lines = vec![format!("path: {header}")];
    lines.extend(result.entries.iter().map(|entry| entry.display_name()));
    if result.truncated {
        lines.push(format!("truncated: entry_limit={}", result.entry_limit));
    }
    lines.join("\n")
}

fn format_read_result(result: &ReadResult) -> String {
    let mut lines = vec![format!("path: {}", result.path), result.content.clone()];
    if let Some(next_offset) = result.next_offset {
        lines.push(format!("truncated: next_offset={next_offset}"));
    }
    lines.join("\n")
}

fn format_find_result(result: &FindResult) -> String {
    let header = if result.path.is_empty() {
        ".".to_owned()
    } else {
        result.path.clone()
    };
    let mut lines = vec![format!("path: {header}")];
    lines.extend(result.matches.iter().map(|entry| {
        if entry.is_dir {
            format!("{}/", entry.path)
        } else {
            entry.path.clone()
        }
    }));
    if result.truncated {
        lines.push(format!("truncated: match_limit={}", result.match_limit));
    }
    lines.join("\n")
}

fn format_grep_result(result: &GrepResult) -> String {
    let header = if result.path.is_empty() {
        ".".to_owned()
    } else {
        result.path.clone()
    };
    let mut lines = vec![format!("path: {header}")];
    lines.extend(
        result
            .matches
            .iter()
            .map(|hit| format!("{}:{}:{}", hit.path, hit.line_number, hit.line)),
    );
    if result.truncated {
        lines.push(format!("truncated: match_limit={}", result.match_limit));
    }
    lines.join("\n")
}

fn format_edit_preview_result(result: &EditPreviewResult) -> String {
    let digest = edit_diff_digest(result.diff.as_str());
    let mut lines = vec![
        format!("path: {}", result.path),
        format!("base_hash: {}", result.base_hash),
        format!("summary: {}", digest.summary_line()),
    ];
    lines.extend(
        digest
            .preview_lines
            .into_iter()
            .map(|line| format!("  {line}")),
    );
    lines.join("\n")
}

fn format_edit_batch_preview_result(result: &EditBatchPreviewResult) -> String {
    let mut lines = Vec::new();
    for preview in &result.previews {
        let digest = edit_diff_digest(preview.diff.as_str());
        lines.push(format!(
            "--- {} base_hash={} ---",
            preview.path, preview.base_hash
        ));
        lines.push(format!("  summary: {}", digest.summary_line()));
        lines.extend(
            digest
                .preview_lines
                .into_iter()
                .map(|line| format!("  {line}")),
        );
        lines.push(String::new());
    }
    if lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn format_edit_apply_result(edit: &FileEditEvent) -> String {
    let digest = edit_diff_digest(edit.checkpoint.diff.as_str());
    format!(
        "modified: {}\ncheckpoint: {}\nsummary: {}",
        edit.checkpoint.path,
        edit.checkpoint.checkpoint_id,
        digest.summary_line()
    )
}

fn format_edit_batch_apply_result(edits: &[FileEditEvent]) -> String {
    let mut lines = vec![format!("modified {} files", edits.len())];
    for edit in edits {
        let digest = edit_diff_digest(edit.checkpoint.diff.as_str());
        lines.push(format!(
            "- {} ({}) {}",
            edit.checkpoint.path,
            edit.checkpoint.checkpoint_id,
            digest.summary_line()
        ));
    }
    lines.join("\n")
}

fn format_edit_rewind_result(edit: &FileEditEvent) -> String {
    let digest = edit_diff_digest(edit.checkpoint.diff.as_str());
    format!(
        "rewound: {}\ncheckpoint: {}\nsummary: {}",
        edit.checkpoint.path,
        edit.rewound_checkpoint_id.as_deref().unwrap_or("unknown"),
        digest.summary_line()
    )
}

struct EditDiffDigest {
    added: usize,
    removed: usize,
    hunk_count: usize,
    match_line: Option<String>,
    preview_lines: Vec<String>,
}

impl EditDiffDigest {
    fn summary_line(&self) -> String {
        let mut parts = vec![format!("{} hunk(s)", self.hunk_count)];
        parts.push(format!("+{}", self.added));
        parts.push(format!("-{}", self.removed));
        if let Some(match_line) = &self.match_line {
            parts.push(match_line.clone());
        }
        parts.join(" ")
    }
}

fn edit_diff_digest(diff: &str) -> EditDiffDigest {
    let mut added = 0;
    let mut removed = 0;
    let mut hunk_count = 0;
    let mut match_line = None;
    let mut preview_lines = Vec::new();

    for line in diff.lines() {
        if line.starts_with("@@") {
            hunk_count += 1;
        }
        if line.starts_with("match: ") {
            match_line = Some(line.trim_start_matches("match: ").to_owned());
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
        if preview_lines.len() < 6 {
            preview_lines.push(line.to_owned());
        }
    }

    if diff.lines().count() > preview_lines.len() {
        preview_lines.push("... (collapsed)".to_owned());
    }

    EditDiffDigest {
        added,
        removed,
        hunk_count: hunk_count.max(1),
        match_line,
        preview_lines,
    }
}

fn format_command_result(result: &CommandExecutionResult) -> String {
    let mut lines = vec![
        format!("command: {}", result.command),
        format!("cwd: {}", result.cwd),
        format!(
            "status: {}",
            if result.timed_out {
                "timeout"
            } else if result.exit_code == Some(0) {
                "success"
            } else {
                "error"
            }
        ),
        format!(
            "exit_code: {}",
            result
                .exit_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_owned())
        ),
    ];

    if result.exit_code != Some(0) || result.timed_out {
        let stderr_preview = preview_lines(&result.stderr, 5);
        if !stderr_preview.is_empty() {
            lines.push("stderr_preview:".to_owned());
            lines.extend(stderr_preview.into_iter().map(|line| format!("  {line}")));
        } else if result.stdout.lines().next().is_some() {
            let stdout_preview = preview_lines(&result.stdout, 3);
            if !stdout_preview.is_empty() {
                lines.push("stdout_preview:".to_owned());
                lines.extend(stdout_preview.into_iter().map(|line| format!("  {line}")));
            }
        }
        if result.timed_out {
            lines.push("note: command timed out".to_owned());
        }
    } else {
        lines.push(format!("duration_ms: {}", result.duration_ms));
        lines.push(format!("stdout_lines: {}", count_lines(&result.stdout)));
        lines.push(format!("stderr_lines: {}", count_lines(&result.stderr)));
    }

    if result.truncated {
        lines.push("note: output was truncated for display".to_owned());
    }

    lines.join("\n")
}

fn preview_lines(content: &str, limit: usize) -> Vec<String> {
    let mut lines = content.lines().map(str::to_owned).collect::<Vec<String>>();
    if lines.is_empty() {
        return Vec::new();
    }
    if lines.len() > limit {
        lines.truncate(limit);
        lines.push("... (collapsed)".to_owned());
    }
    lines
}

fn parse_edit_requests_json(input: &str) -> Result<Vec<EditRequest>, String> {
    let value: serde_json::Value =
        serde_json::from_str(input).map_err(|error| format!("invalid edits_json: {error}"))?;
    let Some(items) = value.as_array() else {
        return Err("edits_json must be a JSON array".to_owned());
    };

    items
        .iter()
        .map(|item| {
            let path = item
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "batch edit item missing path".to_owned())?;
            let find = item
                .get("find")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("batch edit for {path} missing find"))?;
            let replace = item
                .get("replace")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| format!("batch edit for {path} missing replace"))?;
            Ok(EditRequest {
                path: path.to_owned(),
                find: find.to_owned(),
                replace: replace.to_owned(),
                expected_hash: item
                    .get("expected_hash")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        })
        .collect()
}

fn execute_shell_command(
    command: &str,
    cwd: &Path,
    timeout: Duration,
    command_event_tx: Option<mpsc::Sender<EventMsg>>,
    call_id: String,
) -> Result<CommandExecutionResult, String> {
    let mut child = shell_command(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start command: {error}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to capture command stdout".to_owned())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "failed to capture command stderr".to_owned())?;

    if let Some(sender) = &command_event_tx {
        let _ = sender.send(EventMsg::command_run(CommandRunEvent {
            call_id: call_id.clone(),
            command: command.to_owned(),
            cwd: cwd.display().to_string(),
            timeout_seconds: timeout.as_secs(),
            summary: format!("$ {} (cwd={})", command, cwd.display()),
        }));
    }

    let stdout_handle = spawn_command_stream_reader(
        stdout,
        "stdout".to_owned(),
        call_id.clone(),
        command_event_tx.clone(),
    );
    let stderr_handle = spawn_command_stream_reader(
        stderr,
        "stderr".to_owned(),
        call_id.clone(),
        command_event_tx,
    );

    let started_at = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started_at.elapsed() >= timeout {
                    timed_out = true;
                    let _ = child.kill();
                    match child.wait() {
                        Ok(status) => break status,
                        Err(error) => {
                            return Err(format!("failed to wait for timed out command: {error}"));
                        }
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(format!("failed to monitor command: {error}")),
        }
    };

    let stdout = stdout_handle
        .join()
        .map_err(|_| "stdout reader thread panicked".to_owned())?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| "stderr reader thread panicked".to_owned())?;

    let (stdout, stdout_truncated) = truncate_output(stdout, COMMAND_OUTPUT_CHAR_LIMIT);
    let (stderr, stderr_truncated) = truncate_output(stderr, COMMAND_OUTPUT_CHAR_LIMIT);

    Ok(CommandExecutionResult {
        command: command.to_owned(),
        cwd: cwd.display().to_string(),
        exit_code: status.code(),
        timed_out,
        duration_ms: started_at.elapsed().as_millis(),
        stdout,
        stderr,
        truncated: stdout_truncated || stderr_truncated || timed_out,
    })
}

fn count_lines(content: &str) -> usize {
    if content.trim().is_empty() {
        0
    } else {
        content.lines().count()
    }
}

fn spawn_command_stream_reader(
    stream: impl std::io::Read + Send + 'static,
    stream_name: String,
    call_id: String,
    command_event_tx: Option<mpsc::Sender<EventMsg>>,
) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut buffer = String::new();
        let mut collected = String::new();

        loop {
            buffer.clear();
            let read = reader.read_line(&mut buffer).unwrap_or(0);
            if read == 0 {
                break;
            }
            collected.push_str(&buffer);
            if let Some(sender) = &command_event_tx {
                let line = buffer.trim_end_matches(['\r', '\n']);
                let summary = if line.is_empty() {
                    format!("{}:", stream_name)
                } else {
                    format!("{}: {}", stream_name, line)
                };
                let _ = sender.send(EventMsg::command_output(CommandOutputEvent {
                    call_id: call_id.clone(),
                    stream: stream_name.clone(),
                    content: line.to_owned(),
                    summary,
                }));
            }
        }

        collected
    })
}

fn truncate_output(content: String, limit: usize) -> (String, bool) {
    let char_count = content.chars().count();
    if char_count <= limit {
        return (content, false);
    }

    (content.chars().take(limit).collect(), true)
}

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut shell = Command::new("cmd");
        shell.arg("/C").arg(command);
        shell
    } else {
        let mut shell = Command::new("sh");
        shell.arg("-lc").arg(command);
        shell
    }
}

fn command_risk_reason(command: &str) -> Option<String> {
    let trimmed = command.trim();
    let first_token = trimmed.split_whitespace().next().unwrap_or_default();
    let lowered = trimmed.to_lowercase();

    if trimmed.is_empty() {
        return Some("empty command".to_owned());
    }

    if matches!(
        first_token,
        "rm" | "sudo" | "mkfs" | "dd" | "shutdown" | "reboot" | "halt" | "poweroff"
    ) {
        return Some(format!("destructive command '{}'", first_token));
    }

    if lowered.contains("rm -rf") || lowered.contains("rm -fr") {
        return Some("recursive deletion".to_owned());
    }

    if lowered.contains("git reset --hard") || lowered.contains("git clean -fd") {
        return Some("destructive git operation".to_owned());
    }

    if lowered.contains("curl") && (lowered.contains("| sh") || lowered.contains("| bash")) {
        return Some("piping network output into a shell".to_owned());
    }

    None
}

const COMMAND_OUTPUT_CHAR_LIMIT: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandExecutionResult {
    command: String,
    cwd: String,
    exit_code: Option<i32>,
    timed_out: bool,
    duration_ms: u128,
    stdout: String,
    stderr: String,
    truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolArgs {
    values: ToolArgValues,
}

impl ToolArgs {
    fn parse(input: &str) -> Self {
        let trimmed = input.trim();
        if trimmed.starts_with('{') {
            if let Ok(serde_json::Value::Object(values)) = serde_json::from_str(trimmed) {
                return Self {
                    values: ToolArgValues::Json(values),
                };
            }
        }

        let entries = input.lines().filter_map(parse_arg_line).collect::<Vec<_>>();
        Self {
            values: ToolArgValues::Lines(entries),
        }
    }

    fn value(&self, key: &str) -> Option<&str> {
        match &self.values {
            ToolArgValues::Lines(entries) => entries
                .iter()
                .find_map(|(candidate, value)| (candidate == key).then_some(value.as_str())),
            ToolArgValues::Json(values) => values.get(key)?.as_str(),
        }
    }

    fn usize(&self, key: &str) -> Option<usize> {
        match &self.values {
            ToolArgValues::Lines(_) => self.value(key)?.parse::<usize>().ok(),
            ToolArgValues::Json(values) => values
                .get(key)
                .and_then(|value| value.as_u64())
                .map(|value| value as usize)
                .or_else(|| self.value(key)?.parse::<usize>().ok()),
        }
    }

    fn bool(&self, key: &str) -> Option<bool> {
        match &self.values {
            ToolArgValues::Lines(_) => match self.value(key)? {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
            ToolArgValues::Json(values) => values
                .get(key)
                .and_then(|value| value.as_bool())
                .or_else(|| match self.value(key)? {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolArgValues {
    Lines(Vec<(String, String)>),
    Json(serde_json::Map<String, serde_json::Value>),
}

fn parse_arg_line(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim().to_owned(), value.trim().to_owned()))
}

fn json_escape(value: &str) -> String {
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

impl From<WorkspaceError> for ToolResult {
    fn from(error: WorkspaceError) -> Self {
        Self::error("", error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, create_dir_all, write},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn runtime_exposes_read_and_write_definitions() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let definitions = runtime.definitions();

        assert_eq!(
            definitions
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "ls",
                "read",
                "find_files",
                "search_text",
                "propose_edit",
                "propose_edit_batch",
                "apply_patch",
                "apply_patch_batch",
                "rewind_edit",
                "run_command",
            ]
        );
        assert!(definitions
            .iter()
            .any(|definition| definition.permission == PermissionKind::Write));
    }

    #[test]
    fn ls_tool_returns_directory_listing() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src")).unwrap();
        write(temp.path("README.md"), "hello").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new("call-0", "ls", "path=.\nlimit=20"));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("README.md"));
        assert!(result.content.contains("src/"));
    }

    #[test]
    fn read_tool_returns_content_and_truncation() {
        let temp = TempWorkspace::new();
        write(temp.path("README.md"), "hello world").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "read",
            "path=README.md\noffset=0\nlimit=5",
        ));

        assert_eq!(result.status, ToolResultStatus::Truncated);
        assert!(result.content.contains("hello"));
        assert_eq!(result.next_offset, Some(5));
    }

    #[test]
    fn find_tool_returns_recursive_matches() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src/ui")).unwrap();
        write(temp.path("src/ui.rs"), "ui").unwrap();
        write(temp.path("src/ui/mod.rs"), "ui").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "find_files",
            "query=ui\npath=.\nlimit=20",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("src/ui/"));
        assert!(result.content.contains("src/ui.rs"));
        assert!(result.content.contains("src/ui/mod.rs"));
    }

    #[test]
    fn find_alias_returns_recursive_matches() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src/ui")).unwrap();
        write(temp.path("src/ui.rs"), "ui").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "find",
            "query=ui\npath=.\nlimit=20",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("src/ui.rs"));
    }

    #[test]
    fn grep_tool_returns_matching_lines() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src")).unwrap();
        write(temp.path("src/app.rs"), "Picocode\npicocode\n").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "search_text",
            "query=pico\npath=.\nlimit=20\nignore_case=true",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("src/app.rs:1:Picocode"));
        assert!(result.content.contains("src/app.rs:2:picocode"));
    }

    #[test]
    fn grep_alias_returns_matching_lines() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src")).unwrap();
        write(temp.path("src/app.rs"), "Picocode\npicocode\n").unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "grep",
            "query=pico\npath=.\nlimit=20\nignore_case=true",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("src/app.rs:1:Picocode"));
    }

    #[test]
    fn propose_edit_returns_diff_preview() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "propose_edit",
            "path=main.rs\nfind=println!(\"hello\")\nreplace=println!(\"hello world\")",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("--- a/main.rs"));
        assert!(result.content.contains("+++ b/main.rs"));
        assert!(result.content.contains("println!(\"hello world\")"));
    }

    #[test]
    fn apply_patch_batch_writes_multiple_files() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        write(
            temp.path("lib.rs"),
            "pub fn greet() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());
        let edits_json = r#"[{"path":"main.rs","find":"println!(\"hello\")","replace":"println!(\"hello world\")"},{"path":"lib.rs","find":"println!(\"hi\")","replace":"println!(\"hi there\")"}]"#;

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "apply_patch_batch",
            format!("edits_json={edits_json}"),
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(result.edits.len(), 2);
        assert!(result.content.contains("modified 2 files"));
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello world\");\n}\n"
        );
        assert_eq!(
            fs::read_to_string(temp.path("lib.rs")).unwrap(),
            "pub fn greet() {\n    println!(\"hi there\");\n}\n"
        );
    }

    #[test]
    fn apply_patch_batch_detects_conflict_before_writing() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        write(
            temp.path("lib.rs"),
            "pub fn greet() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());
        let edits_json = r#"[{"path":"main.rs","find":"println!(\"hello\")","replace":"println!(\"hello world\")"},{"path":"lib.rs","find":"println!(\"hi\")","replace":"println!(\"hi there\")"}]"#;

        let preview = runtime.execute(ToolCall::new(
            "call-0",
            "propose_edit_batch",
            format!("edits_json={edits_json}"),
        ));
        assert_eq!(preview.status, ToolResultStatus::Success);

        write(
            temp.path("lib.rs"),
            "pub fn greet() {\n    println!(\"changed\");\n}\n",
        )
        .unwrap();

        let result = runtime.execute(ToolCall::new(
            "call-1",
            "apply_patch_batch",
            format!("edits_json={edits_json}"),
        ));

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(
            result.content.contains("edit checkpoint mismatch")
                || result.content.contains("edit preview target not found")
        );
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
    }

    #[test]
    fn apply_patch_batch_rejects_duplicate_file_edits() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());
        let edits_json = r#"[{"path":"main.rs","find":"println!(\"hello\")","replace":"println!(\"hello world\")"},{"path":"main.rs","find":"println!(\"hello\")","replace":"println!(\"hello again\")"}]"#;

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "apply_patch_batch",
            format!("edits_json={edits_json}"),
        ));

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.content.contains("duplicate edit"));
    }

    #[test]
    fn apply_patch_writes_file_and_records_checkpoint() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "apply_patch",
            "path=main.rs\nfind=println!(\"hello\")\nreplace=println!(\"hello world\")",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("modified: main.rs"));
        assert!(result.content.contains("checkpoint:"));
        assert!(result.content.contains("summary:"));
        assert!(result.content.contains("1 hunk(s)"));
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello world\");\n}\n"
        );
        assert!(!result.content.contains(".picocode/edits"));
    }

    #[test]
    fn rewind_edit_restores_latest_checkpoint() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let applied = runtime.execute(ToolCall::new(
            "call-0",
            "apply_patch",
            "path=main.rs\nfind=println!(\"hello\")\nreplace=println!(\"hello world\")",
        ));
        assert_eq!(applied.status, ToolResultStatus::Success);

        let rolled_back = runtime.execute(ToolCall::new("call-1", "rewind_edit", "path=main.rs"));

        assert_eq!(rolled_back.status, ToolResultStatus::Success);
        assert!(rolled_back.content.contains("rewound: main.rs"));
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
    }

    #[test]
    fn run_command_returns_stdout_and_exit_status() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "run_command",
            "command=printf hello",
        ));

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(result.content.contains("command: printf hello"));
        assert!(result.content.contains("stdout_lines: 1"));
        assert!(result.content.contains("stderr_lines: 0"));
    }

    #[test]
    fn run_command_failure_prefers_stderr_preview() {
        let result = CommandExecutionResult {
            command: "demo".to_owned(),
            cwd: "/tmp/project".to_owned(),
            exit_code: Some(1),
            timed_out: false,
            duration_ms: 42,
            stdout: "line one\nline two\nline three\nline four".to_owned(),
            stderr: "boom\ntrace\nmore\nmore\nmore\nmore".to_owned(),
            truncated: false,
        };

        let summary = format_command_result(&result);

        assert!(summary.contains("status: error"));
        assert!(summary.contains("stderr_preview:"));
        assert!(summary.contains("  boom"));
        assert!(summary.contains("  trace"));
        assert!(summary.contains("  ... (collapsed)"));
        assert!(!summary.contains("stdout_lines: 4"));
        assert!(!summary.contains("stderr_lines: 6"));
        assert!(!summary.contains("duration_ms: 42"));
    }

    #[test]
    fn run_command_blocks_risky_deletion_commands() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new(
            "call-0",
            "run_command",
            "command=rm -rf /tmp/picocode-should-not-run",
        ));

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.content.contains("command requires approval"));
    }

    #[test]
    fn runtime_from_events_seeds_checkpoint_history() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let runtime = ToolRuntime::new(workspace.clone());
        let applied = runtime.execute(ToolCall::new(
            "call-0",
            "apply_patch",
            "path=main.rs\nfind=println!(\"hello\")\nreplace=println!(\"hello world\")",
        ));
        let edit = applied
            .edits
            .into_iter()
            .next()
            .expect("expected file edit checkpoint");
        let events = vec![crate::event::Event::new(
            "evt-0",
            crate::event::EventMsg::file_edit(edit),
        )];
        let resumed = ToolRuntime::from_events(workspace, &events);

        let rolled_back = resumed.execute(ToolCall::new("call-1", "rewind_edit", "path=main.rs"));

        assert_eq!(rolled_back.status, ToolResultStatus::Success);
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
    }

    #[test]
    fn grep_tool_rejects_empty_query() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new("call-0", "search_text", "query=  \npath=."));

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.content.contains("query must not be empty"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let result = runtime.execute(ToolCall::new("call-0", "missing", ""));

        assert_eq!(result.status, ToolResultStatus::Error);
        assert!(result.content.contains("unknown tool"));
    }

    #[test]
    fn tool_definition_exports_ai_schema() {
        let temp = TempWorkspace::new();
        let runtime = ToolRuntime::new(Workspace::new(temp.root()).unwrap());

        let specs = runtime.tool_specs();

        assert_eq!(specs[0].name, "ls");
        assert!(specs[0].input_schema_json.contains("\"type\":\"object\""));
        assert!(specs[0].input_schema_json.contains("\"path\""));
    }

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "picocode-tool-test-{}-{}",
                std::process::id(),
                id
            ));
            create_dir_all(&root).unwrap();
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path)
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}
