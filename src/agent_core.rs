use std::sync::Arc;

use crate::{
    ai::{context_from_events, AiClient, AiContext, ToolCallContent},
    event::{Event, EventMsg},
    submission::{Op, Submission},
    tool::{ToolCall, ToolResultStatus, ToolRuntime},
};

const TOOL_PROTOCOL: &str = r#"You can inspect the workspace with read-only tools.

If you need a tool, respond with exactly this format and no extra prose:
<tool_call>
name=ls
path=.
limit=200
</tool_call>

or:
<tool_call>
name=read
path=src/lib.rs
offset=0
limit=4000
</tool_call>

Available tools: ls, read, find_files, search_text, propose_edit, propose_edit_batch, apply_patch, apply_patch_batch, rewind_edit, run_command.
Aliases accepted: find -> find_files, grep -> search_text.
For edits, use propose_edit to preview a diff before any write action.
For multi-file edits, use propose_edit_batch with edits_json, then apply_patch_batch to write them together.
If the user wants a real change, apply_patch writes the file and records a reversible checkpoint.
apply_patch_batch does the same for several files and aborts on conflict before any partial write escapes.
If the user interrupts an in-progress edit session and you need to restore the latest file state, use rewind_edit on the affected file. rollback_edit is accepted as an alias.
Use run_command for shell tasks such as builds, tests, dependency installation, and environment setup. It runs in the workspace root, streams stdout/stderr as command output events, captures the final exit status, and respects project command approval and timeout settings.
If a run_command call fails, inspect the streamed output, search for the cause, edit the relevant files, and rerun the command until the issue is fixed or clearly explained.
Use tools progressively: find files, search text, read exact files, preview edits, apply the patch, run commands, and rewind if the user asks to undo.
Stop calling tools once you have enough evidence to answer."#;

#[derive(Debug, Clone)]
pub struct AgentCore {
    ai: Arc<AiClient>,
    tools: ToolRuntime,
    config: AgentConfig,
}

impl AgentCore {
    pub fn new(ai: Arc<AiClient>, tools: ToolRuntime) -> Self {
        Self {
            ai,
            tools,
            config: AgentConfig::default(),
        }
    }

    pub fn run_submission(&self, events: &[Event], submission: &Submission) -> AgentRunResult {
        match &submission.op {
            Op::UserInput { .. } => self.run_prompt_turn(events),
            Op::LocalCommand { .. } => AgentRunResult::new(Vec::new()),
        }
    }

    fn run_prompt_turn(&self, events: &[Event]) -> AgentRunResult {
        let mut output_events = Vec::new();
        let mut model_steps = 0;
        let mut tool_steps = 0;
        let mut repair_context: Option<String> = None;

        loop {
            if model_steps >= self.config.max_model_steps {
                output_events.push(EventMsg::error(
                    "model step limit reached before final answer",
                ));
                break;
            }

            let context = self.context_for_model(events, &output_events);
            let output = match self.ai.complete(&context) {
                Ok(output) => output,
                Err(error) => {
                    output_events.push(EventMsg::error(error.to_string()));
                    break;
                }
            };
            model_steps += 1;

            let Some(tool_call) = tool_call_from_output(&output) else {
                if let Some(repair_context) = &repair_context {
                    output_events.push(EventMsg::system(format!(
                        "{repair_context}\nContinue the repair loop: inspect the failure, search the code, fix the relevant files, and rerun the same command. Do not finalize yet."
                    )));
                    continue;
                }
                output_events.push(EventMsg::assistant(output.text_content()));
                break;
            };

            if tool_steps >= self.config.max_tool_steps {
                let mut final_pending = output_events.clone();
                final_pending.push(EventMsg::system(format!(
                    "Tool step limit reached ({}/{}). Do not call more tools; answer using the evidence already available.",
                    tool_steps, self.config.max_tool_steps
                )));
                self.push_final_answer_without_tools(events, &final_pending, &mut output_events);
                break;
            }

            output_events.push(EventMsg::tool_call(tool_call.clone()));
            let tool_name = tool_call.name.clone();
            let tool_summary = tool_call.summary();
            let tool_result = self.tools.execute(tool_call);
            let tool_result_status = tool_result.status;
            let file_edits = tool_result.edits.clone();
            output_events.push(EventMsg::tool_result(tool_result));
            output_events.extend(file_edits.into_iter().map(EventMsg::file_edit));
            if tool_name == "run_command" && tool_result_status != ToolResultStatus::Success {
                repair_context = Some(repair_loop_instruction(&tool_summary, tool_result_status));
                output_events.push(EventMsg::system(
                    repair_context
                        .as_ref()
                        .expect("repair context should be set")
                        .clone(),
                ));
            } else if tool_name == "run_command" {
                repair_context = None;
            }
            tool_steps += 1;

            if tool_result_status == ToolResultStatus::Denied {
                output_events.push(EventMsg::error("tool call denied"));
                break;
            }
        }

        AgentRunResult::new(output_events)
    }

    fn push_final_answer_without_tools(
        &self,
        events: &[Event],
        pending_for_context: &[EventMsg],
        output_events: &mut Vec<EventMsg>,
    ) {
        let context = self.context_for_final_answer(events, pending_for_context);
        match self.ai.complete(&context) {
            Ok(output) => output_events.push(EventMsg::assistant(output.text_content())),
            Err(error) => output_events.push(EventMsg::error(error.to_string())),
        }
    }

    fn context_for_model(&self, events: &[Event], pending: &[EventMsg]) -> AiContext {
        let mut context_events = events.to_vec();
        context_events.push(Event::new(
            "agent-core-tool-protocol",
            EventMsg::system(TOOL_PROTOCOL),
        ));
        context_events.extend(
            pending
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, msg)| Event::new(format!("agent-core-pending-{index}"), msg)),
        );
        let mut context = context_from_events(&context_events);
        context.tools = self.tools.tool_specs();
        context
    }

    fn context_for_final_answer(&self, events: &[Event], pending: &[EventMsg]) -> AiContext {
        let mut context_events = events.to_vec();
        context_events.push(Event::new(
            "agent-core-final-answer-instruction",
            EventMsg::system(
                "Tool budget is exhausted. Do not call tools. Provide the best final answer from the available context.",
            ),
        ));
        context_events.extend(
            pending
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, msg)| Event::new(format!("agent-core-final-pending-{index}"), msg)),
        );
        context_from_events(&context_events)
    }
}

fn repair_loop_instruction(command_summary: &str, status: ToolResultStatus) -> String {
    let status_text = match status {
        ToolResultStatus::Success => "succeeded",
        ToolResultStatus::Truncated => "truncated",
        ToolResultStatus::Denied => "denied",
        ToolResultStatus::Error => "failed",
    };

    format!(
        "Repair loop: run_command {} for `{}`. Read the stderr preview in the tool result, search for the cause, fix the relevant files, and rerun the same command before answering.",
        status_text, command_summary
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentConfig {
    pub max_model_steps: usize,
    pub max_tool_steps: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_model_steps: 16,
            max_tool_steps: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunResult {
    pub events: Vec<EventMsg>,
}

impl AgentRunResult {
    fn new(events: Vec<EventMsg>) -> Self {
        Self { events }
    }
}

fn parse_tool_call(content: &str) -> Option<ToolCall> {
    let body = content
        .split_once("<tool_call>")?
        .1
        .split_once("</tool_call>")?
        .0;
    let mut name = None;
    let mut arguments = Vec::new();

    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "name" {
            name = Some(value.trim().to_owned());
        } else {
            arguments.push(format!("{}={}", key.trim(), value.trim()));
        }
    }

    Some(ToolCall::new("call-0", name?, arguments.join("\n")))
}

fn tool_call_from_output(output: &crate::ai::AssistantOutput) -> Option<ToolCall> {
    output
        .tool_calls()
        .into_iter()
        .next()
        .map(tool_call_from_content)
        .or_else(|| parse_tool_call(&output.text_content()))
}

fn tool_call_from_content(content: ToolCallContent) -> ToolCall {
    ToolCall::new(
        content.id,
        content.name,
        normalize_tool_arguments(content.arguments),
    )
}

fn normalize_tool_arguments(arguments: String) -> String {
    arguments.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::ApiProvider;
    use crate::ai::{AiError, AssistantOutput, ContentBlock, Model, ModelCapabilities, StopReason};
    use crate::workspace::Workspace;
    use std::{
        fs::{create_dir_all, write},
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicU64, Ordering},
            Mutex,
        },
    };

    #[test]
    fn parse_tool_call_extracts_name_and_arguments() {
        let call = parse_tool_call(
            "<tool_call>\nname=read\npath=Cargo.toml\noffset=0\nlimit=10\n</tool_call>",
        )
        .unwrap();

        assert_eq!(call.name, "read");
        assert_eq!(call.arguments, "path=Cargo.toml\noffset=0\nlimit=10");
    }

    #[test]
    fn run_submission_returns_assistant_without_tool_call() {
        let temp = TempWorkspace::new();
        let ai = Arc::new(fake_ai(vec!["hello"]));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("hello"));
        let events = vec![Event::new("evt-0", EventMsg::user("hello"))];

        let result = core.run_submission(&events, &submission);

        assert_eq!(result.events, vec![EventMsg::assistant("hello")]);
    }

    #[test]
    fn run_submission_executes_one_tool_then_final_answer() {
        let temp = TempWorkspace::new();
        write(temp.path("README.md"), "hello workspace").unwrap();
        let ai = Arc::new(fake_ai(vec![
            "<tool_call>\nname=read\npath=README.md\noffset=0\nlimit=100\n</tool_call>",
            "README says hello workspace.",
        ]));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("read README"));
        let events = vec![Event::new("evt-0", EventMsg::user("read README"))];

        let result = core.run_submission(&events, &submission);

        assert!(matches!(result.events[0], EventMsg::ToolCall(_)));
        assert!(matches!(result.events[1], EventMsg::ToolResult(_)));
        assert_eq!(
            result.events.last().unwrap(),
            &EventMsg::assistant("README says hello workspace.")
        );
    }

    #[test]
    fn run_submission_executes_multiple_tools_then_final_answer() {
        let temp = TempWorkspace::new();
        write(temp.path("README.md"), "picocode install guide").unwrap();
        let ai = Arc::new(fake_ai(vec![
            "<tool_call>\nname=find\nquery=README\npath=.\nlimit=10\n</tool_call>",
            "<tool_call>\nname=grep\nquery=install\npath=README.md\nlimit=10\n</tool_call>",
            "<tool_call>\nname=read\npath=README.md\noffset=0\nlimit=100\n</tool_call>",
            "README includes an install guide.",
        ]));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("read install guide"));
        let events = vec![Event::new("evt-0", EventMsg::user("read install guide"))];

        let result = core.run_submission(&events, &submission);

        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, EventMsg::ToolCall(_)))
                .count(),
            3
        );
        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, EventMsg::ToolResult(_)))
                .count(),
            3
        );
        assert_eq!(
            result.events.last().unwrap(),
            &EventMsg::assistant("README includes an install guide.")
        );
    }

    #[test]
    fn run_submission_injects_repair_loop_after_command_failure() {
        let temp = TempWorkspace::new();
        let ai = Arc::new(fake_ai(vec![
            "<tool_call>\nname=run_command\ncommand=false\n</tool_call>",
            "The command failed because the shell returned a non-zero exit code.",
            "<tool_call>\nname=run_command\ncommand=printf fixed\n</tool_call>",
            "The command is now fixed.",
        ]));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("run the check"));
        let events = vec![Event::new("evt-0", EventMsg::user("run the check"))];

        let result = core.run_submission(&events, &submission);

        assert!(result.events.iter().any(|event| matches!(
            event,
            EventMsg::SystemMessage(message)
                if message.content.contains("Repair loop: run_command failed")
        )));
        assert!(result.events.iter().any(|event| matches!(
            event,
            EventMsg::SystemMessage(message)
                if message.content.contains("Continue the repair loop")
        )));
        assert_eq!(
            result.events.last().unwrap(),
            &EventMsg::assistant("The command is now fixed.")
        );
    }

    #[test]
    fn run_submission_answers_after_tool_limit() {
        let temp = TempWorkspace::new();
        write(temp.path("README.md"), "picocode").unwrap();
        let mut outputs =
            vec!["<tool_call>\nname=read\npath=README.md\noffset=0\nlimit=100\n</tool_call>"; 13];
        outputs.push("I found README.md and it contains picocode.");
        let ai = Arc::new(fake_ai(outputs));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("read README"));
        let events = vec![Event::new("evt-0", EventMsg::user("read README"))];

        let result = core.run_submission(&events, &submission);

        assert_eq!(
            result
                .events
                .iter()
                .filter(|event| matches!(event, EventMsg::ToolCall(_)))
                .count(),
            12
        );
        assert!(!result.events.iter().any(|event| matches!(
            event,
            EventMsg::SystemMessage(message)
                if message.content.contains("Tool step limit reached")
        )));
        assert_eq!(
            result.events.last().unwrap(),
            &EventMsg::assistant("I found README.md and it contains picocode.")
        );
    }

    fn fake_ai(outputs: Vec<&str>) -> AiClient {
        let provider = FakeProvider {
            outputs: Mutex::new(outputs.into_iter().map(str::to_owned).collect()),
            model: Model::new("fake", "fake", "fake", ModelCapabilities::default()),
        };
        let model = provider.model.clone();
        AiClient::new(Box::new(provider), model)
    }

    fn fake_ai_outputs(outputs: Vec<AssistantOutput>) -> AiClient {
        let provider = FakeOutputProvider {
            outputs: Mutex::new(outputs),
            model: Model::new("fake", "fake", "fake", ModelCapabilities::default()),
        };
        let model = provider.model.clone();
        AiClient::new(Box::new(provider), model)
    }

    #[derive(Debug)]
    struct FakeProvider {
        outputs: Mutex<Vec<String>>,
        model: Model,
    }

    impl ApiProvider for FakeProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn model(&self) -> &Model {
            &self.model
        }

        fn complete(
            &self,
            _model: &Model,
            _context: &crate::ai::AiContext,
        ) -> Result<AssistantOutput, AiError> {
            let output = self.outputs.lock().unwrap().remove(0);
            Ok(AssistantOutput::from_provider_content(output))
        }
    }

    #[test]
    fn run_submission_uses_native_tool_call_before_text_fallback() {
        let temp = TempWorkspace::new();
        write(temp.path("README.md"), "hello native").unwrap();
        let ai = Arc::new(fake_ai_outputs(vec![
            AssistantOutput {
                message: crate::ai::AssistantMessage {
                    content: vec![ContentBlock::tool_call(
                        "native-call-0",
                        "read",
                        "{\"path\":\"README.md\",\"offset\":0,\"limit\":100}",
                    )],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: None,
                },
                raw_response_id: None,
            },
            AssistantOutput::from_provider_content("native final"),
        ]));
        let core = AgentCore::new(ai, ToolRuntime::new(Workspace::new(temp.root()).unwrap()));
        let submission = Submission::new("sub-0", Op::user_input("read README"));
        let events = vec![Event::new("evt-0", EventMsg::user("read README"))];

        let result = core.run_submission(&events, &submission);

        match &result.events[0] {
            EventMsg::ToolCall(call) => {
                assert_eq!(call.call_id, "native-call-0");
                assert_eq!(
                    call.arguments,
                    "{\"path\":\"README.md\",\"offset\":0,\"limit\":100}"
                );
            }
            _ => panic!("expected tool call"),
        }
        assert_eq!(
            result.events.last().unwrap(),
            &EventMsg::assistant("native final")
        );
    }

    #[test]
    fn native_json_arguments_support_search_fields() {
        let call = tool_call_from_content(ToolCallContent {
            id: "native-call-0".to_owned(),
            name: "grep".to_owned(),
            arguments:
                "{\"query\":\"ToolRuntime\",\"path\":\"src\",\"limit\":20,\"ignore_case\":true}"
                    .to_owned(),
        });

        assert_eq!(call.name, "grep");
        assert_eq!(
            call.arguments,
            "{\"query\":\"ToolRuntime\",\"path\":\"src\",\"limit\":20,\"ignore_case\":true}"
        );
    }

    #[derive(Debug)]
    struct FakeOutputProvider {
        outputs: Mutex<Vec<AssistantOutput>>,
        model: Model,
    }

    impl ApiProvider for FakeOutputProvider {
        fn name(&self) -> &str {
            "fake"
        }

        fn model(&self) -> &Model {
            &self.model
        }

        fn complete(
            &self,
            _model: &Model,
            _context: &crate::ai::AiContext,
        ) -> Result<AssistantOutput, AiError> {
            Ok(self.outputs.lock().unwrap().remove(0))
        }
    }

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "picocode-agent-core-test-{}-{}",
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
