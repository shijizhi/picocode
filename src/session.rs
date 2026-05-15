use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    event::{
        is_summary_event, latest_summary_event_index, summarize_branch_summary, CommandOutputEvent,
        CommandRunEvent, CompactionEvent, Event, EventMsg, ImageAttachmentEvent, ToolCallEvent,
        ToolResultEvent,
    },
    tool::ToolResultStatus,
};
use serde_json::{json, Value};

const SESSION_DIR: &str = ".picocode/sessions";
const EXPORT_DIR: &str = ".picocode/exports";
const SHARE_DIR: &str = ".picocode/shares";
const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub path: PathBuf,
}

impl Session {
    pub fn new(id: impl Into<String>, root: &Path) -> Self {
        let id = id.into();
        Self {
            path: root.join(SESSION_DIR).join(format!("{id}.jsonl")),
            id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub cwd: String,
    pub app_version: String,
    pub ai_provider: Option<String>,
    pub ai_model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionItem {
    SessionMeta(SessionMeta),
    EventMsg(Event),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLine {
    pub timestamp: String,
    pub item: SessionItem,
}

impl SessionLine {
    pub fn session_meta(meta: SessionMeta) -> Self {
        Self {
            timestamp: timestamp_millis(),
            item: SessionItem::SessionMeta(meta),
        }
    }

    pub fn event_msg(event: Event) -> Self {
        Self {
            timestamp: timestamp_millis(),
            item: SessionItem::EventMsg(event),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionStore {
    root: PathBuf,
}

static SESSION_ID_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session: Session,
    pub parent_session_id: Option<String>,
    pub cwd: Option<String>,
    pub app_version: Option<String>,
    pub ai_provider: Option<String>,
    pub ai_model: Option<String>,
    pub event_count: usize,
    pub stats: SessionStats,
    pub first_timestamp: Option<String>,
    pub last_timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionStats {
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub image_attachments: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub command_runs: usize,
    pub command_outputs: usize,
    pub compactions: usize,
    pub branch_summaries: usize,
    pub file_edits: usize,
    pub errors: usize,
    pub finals: usize,
    pub duration_millis: Option<u128>,
}

impl SessionStats {
    pub fn compact_label(&self) -> String {
        let mut parts = Vec::new();
        parts.push(format!("u={}", self.user_messages));
        parts.push(format!("a={}", self.assistant_messages));
        parts.push(format!("img={}", self.image_attachments));
        parts.push(format!("tool={}", self.tool_calls));
        parts.push(format!("cmd={}", self.command_runs));
        parts.push(format!("edit={}", self.file_edits));
        parts.push(format!("sum={}", self.compactions + self.branch_summaries));
        if let Some(duration) = self.duration_millis {
            parts.push(format!("span={}s", duration / 1000));
        }
        parts.join(" ")
    }

    pub fn verbose_label(&self) -> String {
        format!(
            "user={} assistant={} img={} tool={} cmd={} edit={} compact={} branch={} err={} final={}{}",
            self.user_messages,
            self.assistant_messages,
            self.image_attachments,
            self.tool_calls,
            self.command_runs,
            self.file_edits,
            self.compactions,
            self.branch_summaries,
            self.errors,
            self.finals,
            self.duration_millis
                .map(|duration| format!(" span={}s", duration / 1000))
                .unwrap_or_default()
        )
    }
}

impl SessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn create_session(&self) -> io::Result<Session> {
        self.create_session_with_parent(None)
    }

    pub fn create_session_with_parent(
        &self,
        parent_session_id: Option<String>,
    ) -> io::Result<Session> {
        let id = new_session_id();
        let session = Session::new(id, &self.root);
        self.ensure_dir()?;
        File::create(&session.path)?;
        self.append_line(
            &session,
            &SessionLine::session_meta(SessionMeta {
                session_id: session.id.clone(),
                parent_session_id,
                cwd: self.root.display().to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                ai_provider: None,
                ai_model: None,
            }),
        )?;
        Ok(session)
    }

    pub fn fork_session(&self, session: &Session) -> io::Result<Session> {
        self.ensure_dir()?;
        let forked = self.create_session_with_parent(Some(session.id.clone()))?;
        let events = self.load_events(session)?;
        if let Some(summary_index) = latest_summary_event_index(&events) {
            let active_tail = &events[summary_index.saturating_add(1)..];
            if !active_tail.is_empty() {
                let folded_event_count = events[..summary_index]
                    .iter()
                    .filter(|event| !is_summary_event(event))
                    .count();
                let branch_summary =
                    summarize_branch_summary(session.id.clone(), folded_event_count, active_tail);
                self.append_line(
                    &forked,
                    &SessionLine::event_msg(Event::new(
                        format!("evt-branch-{}", forked.id),
                        EventMsg::branch_summary(branch_summary),
                    )),
                )?;
                for event in active_tail {
                    self.append_event(&forked, event)?;
                }
                return Ok(forked);
            }
        }

        let lines = self.load_lines(session)?;
        for line in lines {
            let SessionLine { timestamp, item } = line;
            if let SessionItem::EventMsg(event) = item {
                self.append_line(
                    &forked,
                    &SessionLine {
                        timestamp,
                        item: SessionItem::EventMsg(event),
                    },
                )?;
            }
        }

        Ok(forked)
    }

    pub fn open_session(&self, id: &str) -> Session {
        Session::new(id, &self.root)
    }

    pub fn rename_session(&self, session: &Session, new_id: &str) -> io::Result<Session> {
        self.ensure_dir()?;
        let trimmed = new_id.trim();
        if trimmed.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "session name cannot be empty",
            ));
        }

        let new_session = Session::new(trimmed, &self.root);
        if new_session.path == session.path {
            return Ok(new_session);
        }
        if new_session.path.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("session already exists: {}", new_session.id),
            ));
        }
        fs::rename(&session.path, &new_session.path)?;
        Ok(new_session)
    }

    pub fn delete_session(&self, session: &Session) -> io::Result<()> {
        if session.path.exists() {
            fs::remove_file(&session.path)?;
        }
        Ok(())
    }

    pub fn export_session_html(&self, session: &Session) -> io::Result<PathBuf> {
        self.write_session_html(session, EXPORT_DIR, "export")
    }

    pub fn share_session_html(&self, session: &Session) -> io::Result<PathBuf> {
        self.write_session_html(session, SHARE_DIR, "share")
    }

    pub fn list_sessions(&self) -> io::Result<Vec<Session>> {
        let dir = self.root.join(SESSION_DIR);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut sessions = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
                sessions.push(Session {
                    id: id.to_owned(),
                    path,
                });
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    pub fn list_session_summaries(&self) -> io::Result<Vec<SessionSummary>> {
        let mut summaries = self
            .list_sessions()?
            .into_iter()
            .map(|session| self.summarize_session(&session))
            .collect::<io::Result<Vec<_>>>()?;
        summaries.sort_by(|left, right| {
            let left_key = (
                left.last_timestamp.as_deref().unwrap_or("0").to_owned(),
                left.session.id.clone(),
            );
            let right_key = (
                right.last_timestamp.as_deref().unwrap_or("0").to_owned(),
                right.session.id.clone(),
            );
            right_key.cmp(&left_key)
        });
        Ok(summaries)
    }

    pub fn append_event(&self, session: &Session, event: &Event) -> io::Result<()> {
        self.append_line(session, &SessionLine::event_msg(event.clone()))
    }

    pub fn append_session_meta(&self, session: &Session, meta: SessionMeta) -> io::Result<()> {
        self.append_line(session, &SessionLine::session_meta(meta))
    }

    pub fn load_events(&self, session: &Session) -> io::Result<Vec<Event>> {
        Ok(self
            .load_lines(session)?
            .into_iter()
            .filter_map(|line| match line.item {
                SessionItem::EventMsg(event) => Some(event),
                SessionItem::SessionMeta(_) => None,
            })
            .collect())
    }

    pub fn load_lines(&self, session: &Session) -> io::Result<Vec<SessionLine>> {
        let file = File::open(&session.path)?;
        let reader = BufReader::new(file);
        let mut lines = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(session_line) = session_line_from_json(&line) {
                lines.push(session_line);
            }
        }

        Ok(lines)
    }

    pub fn summarize_session(&self, session: &Session) -> io::Result<SessionSummary> {
        let lines = self.load_lines(session)?;
        let mut cwd = None;
        let mut parent_session_id = None;
        let mut app_version = None;
        let mut ai_provider = None;
        let mut ai_model = None;
        let mut event_count = 0;
        let mut stats = SessionStats::default();
        let mut first_timestamp = None;
        let mut last_timestamp = None;

        for line in lines {
            if first_timestamp.is_none() {
                first_timestamp = Some(line.timestamp.clone());
            }
            last_timestamp = Some(line.timestamp);
            match line.item {
                SessionItem::SessionMeta(meta) => {
                    parent_session_id = meta.parent_session_id;
                    cwd = Some(meta.cwd);
                    app_version = Some(meta.app_version);
                    ai_provider = meta.ai_provider;
                    ai_model = meta.ai_model;
                }
                SessionItem::EventMsg(event) => {
                    event_count += 1;
                    match event.msg {
                        EventMsg::UserMessage(_) => stats.user_messages += 1,
                        EventMsg::AssistantMessage(_) => stats.assistant_messages += 1,
                        EventMsg::ImageAttachment(_) => stats.image_attachments += 1,
                        EventMsg::ToolCall(_) => stats.tool_calls += 1,
                        EventMsg::ToolResult(_) => stats.tool_results += 1,
                        EventMsg::CommandRun(_) => stats.command_runs += 1,
                        EventMsg::CommandOutput(_) => stats.command_outputs += 1,
                        EventMsg::Compaction(_) => stats.compactions += 1,
                        EventMsg::BranchSummary(_) => stats.branch_summaries += 1,
                        EventMsg::FileEdit(_) => stats.file_edits += 1,
                        EventMsg::Error(_) => stats.errors += 1,
                        EventMsg::Final(_) => stats.finals += 1,
                        EventMsg::SystemMessage(_) => {}
                    }
                }
            }
        }

        stats.duration_millis = match (&first_timestamp, &last_timestamp) {
            (Some(first), Some(last)) => match (first.parse::<u128>(), last.parse::<u128>()) {
                (Ok(first), Ok(last)) if last >= first => Some(last - first),
                _ => None,
            },
            _ => None,
        };

        Ok(SessionSummary {
            session: session.clone(),
            parent_session_id,
            cwd,
            app_version,
            ai_provider,
            ai_model,
            event_count,
            stats,
            first_timestamp,
            last_timestamp,
        })
    }

    fn append_line(&self, session: &Session, line: &SessionLine) -> io::Result<()> {
        self.ensure_dir()?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&session.path)?;
        writeln!(file, "{}", session_line_to_json(line))
    }

    fn write_session_html(
        &self,
        session: &Session,
        export_dir: &str,
        mode_label: &str,
    ) -> io::Result<PathBuf> {
        let summary = self.summarize_session(session)?;
        let lines = self.load_lines(session)?;
        let output_dir = self.root.join(export_dir);
        fs::create_dir_all(&output_dir)?;
        let output_path = output_dir.join(format!("{}.html", session.id));
        fs::write(
            &output_path,
            session_html_document(session, &summary, &lines, mode_label),
        )?;
        Ok(output_path)
    }

    fn ensure_dir(&self) -> io::Result<()> {
        fs::create_dir_all(self.root.join(SESSION_DIR))
    }
}

fn new_session_id() -> String {
    let seq = SESSION_ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{}", epoch_millis(), seq)
}

fn timestamp_millis() -> String {
    epoch_millis().to_string()
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn session_line_to_json(line: &SessionLine) -> String {
    let (kind, payload) = match &line.item {
        SessionItem::SessionMeta(meta) => (
            "session_meta",
            json!({
                "format_version": FORMAT_VERSION,
                "session_id": meta.session_id,
                "parent_session_id": meta.parent_session_id,
                "cwd": meta.cwd,
                "app_version": meta.app_version,
                "ai_provider": meta.ai_provider,
                "ai_model": meta.ai_model,
            }),
        ),
        SessionItem::EventMsg(event) => ("event_msg", event_payload_to_json(event)),
    };

    json!({
        "timestamp": line.timestamp,
        "type": kind,
        "payload": payload,
    })
    .to_string()
}

fn session_html_document(
    session: &Session,
    summary: &SessionSummary,
    lines: &[SessionLine],
    mode_label: &str,
) -> String {
    let mut html = String::new();
    let title = format!("PicoCode {mode_label}: {}", session.id);
    let parent_session = summary.parent_session_id.as_deref().unwrap_or("-");
    let cwd = summary.cwd.as_deref().unwrap_or("-");
    let ai_profile = match (&summary.ai_provider, &summary.ai_model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        _ => "-".to_owned(),
    };

    html.push_str("<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">");
    write!(
        html,
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
    )
    .ok();
    write!(html, "<title>{}</title>", html_escape(&title)).ok();
    html.push_str(
        "<style>\
        :root{color-scheme:dark;}\
        body{margin:0;font-family:ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,sans-serif;background:#282a36;color:#f8f8f2;}\
        .wrap{max-width:1100px;margin:0 auto;padding:24px;}\
        .hero{padding:20px 24px;border:1px solid #44475a;border-radius:16px;background:#2b2d3a;margin-bottom:20px;}\
        .k{color:#6272a4;text-transform:uppercase;letter-spacing:.08em;font-size:12px;}\
        .v{color:#f8f8f2;font-weight:600;}\
        .meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(220px,1fr));gap:12px;margin-top:16px;}\
        .meta div{padding:12px 14px;background:#21222c;border:1px solid #44475a;border-radius:12px;}\
        .timeline{display:flex;flex-direction:column;gap:12px;}\
        .event{padding:16px 18px;border:1px solid #44475a;border-radius:14px;background:#21222c;}\
        .label{color:#bd93f9;font-weight:700;margin-bottom:10px;}\
        .system .label,.summary .label{color:#f1fa8c;}\
        .user .label{color:#50fa7b;}\
        .assistant .label{color:#f8f8f2;}\
        .error .label{color:#ff5555;}\
        pre{margin:0;white-space:pre-wrap;word-break:break-word;color:#f8f8f2;font-family:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,monospace;line-height:1.55;}\
        .content{color:#f8f8f2;}\
        .comment{color:#6272a4;}\
        .process{color:#9ea4bd;}\
        img.preview{display:block;max-width:100%;margin-top:12px;border:1px solid #44475a;border-radius:12px;background:#21222c;}\
        </style>",
    );
    html.push_str("</head><body><div class=\"wrap\">");
    html.push_str("<section class=\"hero\">");
    write!(
        html,
        "<div class=\"k\">PicoCode {}</div>",
        html_escape(mode_label)
    )
    .ok();
    write!(
        html,
        "<h1 style=\"margin:8px 0 0;\">{}</h1>",
        html_escape(&session.id)
    )
    .ok();
    html.push_str("<div class=\"meta\">");
    html.push_str(&summary_box("session", &summary.session.id));
    html.push_str(&summary_box("parent", parent_session));
    html.push_str(&summary_box("cwd", cwd));
    html.push_str(&summary_box("ai", ai_profile.as_str()));
    html.push_str(&summary_box("events", &summary.event_count.to_string()));
    html.push_str(&summary_box("stats", &summary.stats.compact_label()));
    html.push_str(&summary_box(
        "last activity",
        summary.last_timestamp.as_deref().unwrap_or("-"),
    ));
    html.push_str("</div></section>");
    html.push_str("<section class=\"timeline\">");

    for line in lines {
        if let SessionItem::EventMsg(event) = &line.item {
            html.push_str(&render_export_event(event));
        }
    }

    html.push_str("</section></div></body></html>");
    html
}

fn summary_box(label: &str, value: &str) -> String {
    format!(
        "<div><div class=\"k\">{}</div><div class=\"v\">{}</div></div>",
        html_escape(label),
        html_escape(value)
    )
}

fn render_export_image(event: &Event, image: &ImageAttachmentEvent) -> String {
    format!(
        "<article class=\"event image\"><div class=\"label\">{}</div><div class=\"content\"><div class=\"process\">source: {}</div><div class=\"process\">file: {}</div><div class=\"process\">mime: {} · bytes: {}</div><img class=\"preview\" src=\"{}\" alt=\"{}\"></div></article>",
        html_escape(event.msg.label()),
        html_escape(&image.source_path),
        html_escape(&image.file_name),
        html_escape(&image.mime_type),
        html_escape(&image.byte_len.to_string()),
        html_escape(&image.data_url),
        html_escape(&image.file_name),
    )
}

fn render_export_event(event: &Event) -> String {
    let (class_name, label, body, body_class) = match &event.msg {
        EventMsg::SystemMessage(message) => (
            "event system",
            "system".to_owned(),
            message.content.clone(),
            "process",
        ),
        EventMsg::UserMessage(message) => (
            "event user",
            "you".to_owned(),
            message.content.clone(),
            "content",
        ),
        EventMsg::AssistantMessage(message) => (
            "event assistant",
            "assistant".to_owned(),
            message.content.clone(),
            "content",
        ),
        EventMsg::ImageAttachment(image) => {
            return render_export_image(event, image);
        }
        EventMsg::ToolCall(call) => (
            "event summary",
            "tool".to_owned(),
            format!("# {}\n$ {}", call.name, call.arguments),
            "process",
        ),
        EventMsg::ToolResult(result) => (
            "event summary",
            "tool result".to_owned(),
            result.content.clone(),
            "process",
        ),
        EventMsg::CommandRun(command) => (
            "event summary",
            "command".to_owned(),
            format!(
                "# Run command\n$ {}\ncwd: {}  timeout: {}s",
                command.command, command.cwd, command.timeout_seconds
            ),
            "process",
        ),
        EventMsg::CommandOutput(output) => (
            "event summary",
            format!("command output · {}", output.stream),
            output.content.clone(),
            "process",
        ),
        EventMsg::Compaction(compaction) => (
            "event summary",
            "session compacted".to_owned(),
            format!(
                "folded {} event(s)\n\n{}",
                compaction.folded_event_count, compaction.summary
            ),
            "process",
        ),
        EventMsg::BranchSummary(summary) => (
            "event summary",
            format!("branch summary · from {}", summary.source_session_id),
            format!(
                "folded {} event(s)\n\n{}",
                summary.folded_event_count, summary.summary
            ),
            "process",
        ),
        EventMsg::FileEdit(edit) => (
            "event summary",
            match edit.action {
                crate::event::FileEditAction::Applied => "edit checkpoint".to_owned(),
                crate::event::FileEditAction::RolledBack => "rewind edit".to_owned(),
            },
            format!("{}\n\n{}", edit.summary, edit.checkpoint.diff),
            "process",
        ),
        EventMsg::Error(message) => (
            "event error",
            "error".to_owned(),
            message.message.clone(),
            "content",
        ),
        EventMsg::Final(message) => (
            "event summary",
            "final".to_owned(),
            message.summary.clone(),
            "content",
        ),
    };

    format!(
        "<article class=\"{}\"><div class=\"label\">{}</div><pre class=\"{}\">{}</pre></article>",
        class_name,
        html_escape(label.as_str()),
        body_class,
        html_escape(&body)
    )
}

fn html_escape(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(ch),
        }
    }
    output
}

fn session_line_from_json(line: &str) -> Option<SessionLine> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let timestamp = value.get("timestamp")?.as_str()?.to_owned();
    let kind = value.get("type")?.as_str()?;
    let payload = value.get("payload")?;
    let item = match kind {
        "session_meta" => SessionItem::SessionMeta(SessionMeta {
            session_id: payload.get("session_id")?.as_str()?.to_owned(),
            parent_session_id: payload
                .get("parent_session_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            cwd: payload.get("cwd")?.as_str()?.to_owned(),
            app_version: payload.get("app_version")?.as_str()?.to_owned(),
            ai_provider: payload
                .get("ai_provider")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ai_model: payload
                .get("ai_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "event_msg" => SessionItem::EventMsg(event_from_payload_json(payload)?),
        _ => return None,
    };

    Some(SessionLine { timestamp, item })
}

fn event_payload_to_json(event: &Event) -> Value {
    match &event.msg {
        EventMsg::SystemMessage(message) => json!({
            "id": event.id,
            "type": "system_message",
            "content": message.content,
        }),
        EventMsg::UserMessage(message) => json!({
            "id": event.id,
            "type": "user_message",
            "content": message.content,
        }),
        EventMsg::AssistantMessage(message) => json!({
            "id": event.id,
            "type": "assistant_message",
            "content": message.content,
        }),
        EventMsg::ImageAttachment(image) => json!({
            "id": event.id,
            "type": "image_attachment",
            "source_path": image.source_path,
            "file_name": image.file_name,
            "mime_type": image.mime_type,
            "byte_len": image.byte_len,
            "data_url": image.data_url,
            "summary": image.summary,
        }),
        EventMsg::ToolCall(call) => {
            json!({
                "id": event.id,
                "type": "tool_call",
                "call_id": call.call_id,
                "name": call.name,
                "arguments": call.arguments,
            })
        }
        EventMsg::ToolResult(result) => {
            json!({
                "id": event.id,
                "type": "tool_result",
                "call_id": result.call_id,
                "status": result.status.as_str(),
                "content": result.content,
                "truncated": result.truncated,
                "next_offset": result.next_offset,
                "edits": result.edits.iter().map(file_edit_to_json).collect::<Vec<_>>(),
            })
        }
        EventMsg::CommandRun(command) => json!({
            "id": event.id,
            "type": "command_run",
            "call_id": command.call_id,
            "command": command.command,
            "cwd": command.cwd,
            "timeout_seconds": command.timeout_seconds,
            "summary": command.summary,
        }),
        EventMsg::CommandOutput(output) => json!({
            "id": event.id,
            "type": "command_output",
            "call_id": output.call_id,
            "stream": output.stream,
            "content": output.content,
            "summary": output.summary,
        }),
        EventMsg::Compaction(compaction) => json!({
            "id": event.id,
            "type": "compaction",
            "summary": compaction.summary,
            "folded_event_count": compaction.folded_event_count,
        }),
        EventMsg::BranchSummary(summary) => json!({
            "id": event.id,
            "type": "branch_summary",
            "source_session_id": summary.source_session_id,
            "summary": summary.summary,
            "folded_event_count": summary.folded_event_count,
        }),
        EventMsg::FileEdit(edit) => json!({
            "id": event.id,
            "type": "file_edit",
            "action": match edit.action {
                crate::event::FileEditAction::Applied => "applied",
                crate::event::FileEditAction::RolledBack => "rolled_back",
            },
            "checkpoint": checkpoint_to_json(&edit.checkpoint),
            "rewound_checkpoint_id": edit.rewound_checkpoint_id.clone(),
            "summary": edit.summary.clone(),
        }),
        EventMsg::Error(message) => json!({
            "id": event.id,
            "type": "error",
            "message": message.message,
        }),
        EventMsg::Final(message) => json!({
            "id": event.id,
            "type": "final",
            "summary": message.summary,
        }),
    }
}

fn event_from_payload_json(payload: &Value) -> Option<Event> {
    let id = payload.get("id")?.as_str()?.to_owned();
    let kind = payload.get("type")?.as_str()?;
    let msg = match kind {
        "system_message" => EventMsg::system(payload.get("content")?.as_str()?.to_owned()),
        "user_message" => EventMsg::user(payload.get("content")?.as_str()?.to_owned()),
        "assistant_message" => EventMsg::assistant(payload.get("content")?.as_str()?.to_owned()),
        "image_attachment" => EventMsg::ImageAttachment(ImageAttachmentEvent::new(
            payload.get("source_path")?.as_str()?.to_owned(),
            payload.get("file_name")?.as_str()?.to_owned(),
            payload.get("mime_type")?.as_str()?.to_owned(),
            payload.get("byte_len")?.as_u64().unwrap_or(0) as usize,
            payload.get("data_url")?.as_str()?.to_owned(),
        )),
        "tool_call" => EventMsg::ToolCall(ToolCallEvent::new(
            payload.get("call_id")?.as_str()?.to_owned(),
            payload.get("name")?.as_str()?.to_owned(),
            payload.get("arguments")?.as_str()?.to_owned(),
        )),
        "tool_result" => EventMsg::ToolResult(ToolResultEvent {
            call_id: payload.get("call_id")?.as_str()?.to_owned(),
            status: ToolResultStatus::from_str(payload.get("status")?.as_str()?)?,
            content: payload.get("content")?.as_str()?.to_owned(),
            truncated: payload
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            next_offset: payload
                .get("next_offset")
                .and_then(Value::as_u64)
                .map(|value| value as usize),
            edits: payload
                .get("edits")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(file_edit_from_json).collect())
                .unwrap_or_else(|| {
                    payload
                        .get("edit")
                        .and_then(file_edit_from_json)
                        .into_iter()
                        .collect()
                }),
        }),
        "command_run" => EventMsg::command_run(CommandRunEvent {
            call_id: payload.get("call_id")?.as_str()?.to_owned(),
            command: payload.get("command")?.as_str()?.to_owned(),
            cwd: payload.get("cwd")?.as_str()?.to_owned(),
            timeout_seconds: payload
                .get("timeout_seconds")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            summary: payload.get("summary")?.as_str()?.to_owned(),
        }),
        "command_output" => EventMsg::command_output(CommandOutputEvent {
            call_id: payload.get("call_id")?.as_str()?.to_owned(),
            stream: payload.get("stream")?.as_str()?.to_owned(),
            content: payload.get("content")?.as_str()?.to_owned(),
            summary: payload.get("summary")?.as_str()?.to_owned(),
        }),
        "compaction" => EventMsg::Compaction(CompactionEvent {
            summary: payload.get("summary")?.as_str()?.to_owned(),
            folded_event_count: payload
                .get("folded_event_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        }),
        "branch_summary" => EventMsg::branch_summary(crate::event::BranchSummaryEvent {
            source_session_id: payload.get("source_session_id")?.as_str()?.to_owned(),
            summary: payload.get("summary")?.as_str()?.to_owned(),
            folded_event_count: payload
                .get("folded_event_count")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize,
        }),
        "file_edit" => EventMsg::file_edit(file_edit_from_json(payload)?),
        "error" => EventMsg::error(payload.get("message")?.as_str()?.to_owned()),
        "final" => EventMsg::final_summary(payload.get("summary")?.as_str()?.to_owned()),
        _ => return None,
    };
    Some(Event::new(id, msg))
}

fn file_edit_from_json(payload: &Value) -> Option<crate::event::FileEditEvent> {
    let action = match payload.get("action")?.as_str()? {
        "applied" => crate::event::FileEditAction::Applied,
        "rolled_back" => crate::event::FileEditAction::RolledBack,
        _ => return None,
    };

    Some(crate::event::FileEditEvent {
        action,
        checkpoint: payload
            .get("checkpoint")
            .and_then(checkpoint_from_json)
            .or_else(|| legacy_checkpoint_from_json(payload))?,
        rewound_checkpoint_id: payload
            .get("rewound_checkpoint_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
        summary: payload.get("summary")?.as_str()?.to_owned(),
    })
}

fn checkpoint_to_json(checkpoint: &crate::workspace::EditCheckpoint) -> Value {
    json!({
        "checkpoint_id": checkpoint.checkpoint_id.clone(),
        "path": checkpoint.path.clone(),
        "base_hash": checkpoint.base_hash.clone(),
        "result_hash": checkpoint.result_hash.clone(),
        "before_content": checkpoint.before_content.clone(),
        "after_content": checkpoint.after_content.clone(),
        "diff": checkpoint.diff.clone(),
    })
}

fn checkpoint_from_json(payload: &Value) -> Option<crate::workspace::EditCheckpoint> {
    Some(crate::workspace::EditCheckpoint {
        checkpoint_id: payload.get("checkpoint_id")?.as_str()?.to_owned(),
        path: payload.get("path")?.as_str()?.to_owned(),
        base_hash: payload
            .get("base_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        result_hash: payload
            .get("result_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        before_content: payload
            .get("before_content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        after_content: payload
            .get("after_content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        diff: payload
            .get("diff")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

fn legacy_checkpoint_from_json(payload: &Value) -> Option<crate::workspace::EditCheckpoint> {
    let path = payload.get("path")?.as_str()?.to_owned();
    Some(crate::workspace::EditCheckpoint {
        checkpoint_id: payload
            .get("checkpoint_id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("backup_path").and_then(Value::as_str))
            .unwrap_or("legacy-checkpoint")
            .to_owned(),
        path,
        base_hash: payload
            .get("base_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        result_hash: payload
            .get("result_hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        before_content: payload
            .get("before_content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        after_content: payload
            .get("after_content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        diff: payload
            .get("diff")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    })
}

fn file_edit_to_json(edit: &crate::event::FileEditEvent) -> Value {
    json!({
        "action": match edit.action {
            crate::event::FileEditAction::Applied => "applied",
            crate::event::FileEditAction::RolledBack => "rolled_back",
        },
        "checkpoint": checkpoint_to_json(&edit.checkpoint),
        "rewound_checkpoint_id": edit.rewound_checkpoint_id.clone(),
        "summary": edit.summary.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, time::Duration};

    #[test]
    fn session_line_json_round_trips_event_msg() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new("evt-1", EventMsg::user("hello\nthere"))),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        assert_eq!(parsed.timestamp, "123");
        match parsed.item {
            SessionItem::EventMsg(event) => {
                assert_eq!(event.id, "evt-1");
                assert_eq!(event.msg.content(), "hello\nthere");
            }
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_session_meta() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::SessionMeta(SessionMeta {
                session_id: "session-1".to_owned(),
                parent_session_id: Some("session-root".to_owned()),
                cwd: ".".to_owned(),
                app_version: "0.1.0".to_owned(),
                ai_provider: Some("openai".to_owned()),
                ai_model: Some("gpt-5".to_owned()),
            }),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        assert_eq!(parsed.timestamp, "123");
        match parsed.item {
            SessionItem::SessionMeta(meta) => {
                assert_eq!(meta.session_id, "session-1");
                assert_eq!(meta.parent_session_id.as_deref(), Some("session-root"));
                assert_eq!(meta.cwd, ".");
                assert_eq!(meta.ai_provider.as_deref(), Some("openai"));
                assert_eq!(meta.ai_model.as_deref(), Some("gpt-5"));
            }
            SessionItem::EventMsg(_) => panic!("expected session meta"),
        }
    }

    #[test]
    fn session_line_json_round_trips_tool_call() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-1",
                EventMsg::ToolCall(ToolCallEvent::new("call-0", "read", "path=Cargo.toml")),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::ToolCall(call) => {
                    assert_eq!(call.call_id, "call-0");
                    assert_eq!(call.name, "read");
                    assert_eq!(call.arguments, "path=Cargo.toml");
                }
                _ => panic!("expected tool call"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_image_attachment() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-img",
                EventMsg::ImageAttachment(ImageAttachmentEvent::new(
                    "./shot.png",
                    "shot.png",
                    "image/png",
                    12,
                    "data:image/png;base64,AAAA",
                )),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::ImageAttachment(image) => {
                    assert_eq!(image.source_path, "./shot.png");
                    assert_eq!(image.file_name, "shot.png");
                    assert_eq!(image.mime_type, "image/png");
                    assert_eq!(image.byte_len, 12);
                    assert_eq!(image.data_url, "data:image/png;base64,AAAA");
                }
                _ => panic!("expected image attachment"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_tool_result() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-2",
                EventMsg::ToolResult(ToolResultEvent {
                    call_id: "call-0".to_owned(),
                    status: ToolResultStatus::Truncated,
                    content: "path: Cargo.toml\n[package]".to_owned(),
                    truncated: true,
                    next_offset: Some(42),
                    edits: Vec::new(),
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::ToolResult(result) => {
                    assert_eq!(result.call_id, "call-0");
                    assert_eq!(result.status, ToolResultStatus::Truncated);
                    assert_eq!(result.content, "path: Cargo.toml\n[package]");
                    assert!(result.truncated);
                    assert_eq!(result.next_offset, Some(42));
                }
                _ => panic!("expected tool result"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_compaction_event() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-4",
                EventMsg::Compaction(CompactionEvent {
                    summary: "Folded 18 events".to_owned(),
                    folded_event_count: 18,
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::Compaction(compaction) => {
                    assert_eq!(compaction.summary, "Folded 18 events");
                    assert_eq!(compaction.folded_event_count, 18);
                }
                _ => panic!("expected compaction event"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_branch_summary_event() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-4b",
                EventMsg::branch_summary(crate::event::BranchSummaryEvent {
                    source_session_id: "session-parent".to_owned(),
                    summary: "Branched from parent".to_owned(),
                    folded_event_count: 4,
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::BranchSummary(summary) => {
                    assert_eq!(summary.source_session_id, "session-parent");
                    assert_eq!(summary.summary, "Branched from parent");
                    assert_eq!(summary.folded_event_count, 4);
                }
                _ => panic!("expected branch summary event"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_tool_result_edits() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-2b",
                EventMsg::ToolResult(ToolResultEvent {
                    call_id: "call-0".to_owned(),
                    status: ToolResultStatus::Success,
                    content: "modified 1 files".to_owned(),
                    truncated: false,
                    next_offset: None,
                    edits: vec![crate::event::FileEditEvent {
                        action: crate::event::FileEditAction::Applied,
                        checkpoint: crate::workspace::EditCheckpoint {
                            checkpoint_id: "checkpoint-1".to_owned(),
                            path: "main.rs".to_owned(),
                            base_hash: "base".to_owned(),
                            result_hash: "result".to_owned(),
                            before_content: "before".to_owned(),
                            after_content: "after".to_owned(),
                            diff: "--- a/main.rs\n+++ b/main.rs".to_owned(),
                        },
                        rewound_checkpoint_id: None,
                        summary: "applied edit to main.rs".to_owned(),
                    }],
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::ToolResult(result) => {
                    assert_eq!(result.edits.len(), 1);
                    assert_eq!(result.edits[0].checkpoint.path, "main.rs");
                }
                _ => panic!("expected tool result"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_file_edit_checkpoint() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-3",
                EventMsg::file_edit(crate::event::FileEditEvent {
                    action: crate::event::FileEditAction::Applied,
                    checkpoint: crate::workspace::EditCheckpoint {
                        checkpoint_id: "checkpoint-1".to_owned(),
                        path: "main.rs".to_owned(),
                        base_hash: "base".to_owned(),
                        result_hash: "result".to_owned(),
                        before_content: "before".to_owned(),
                        after_content: "after".to_owned(),
                        diff: "--- a/main.rs\n+++ b/main.rs".to_owned(),
                    },
                    rewound_checkpoint_id: None,
                    summary: "applied edit to main.rs".to_owned(),
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::FileEdit(edit) => {
                    assert_eq!(edit.checkpoint.checkpoint_id, "checkpoint-1");
                    assert_eq!(edit.checkpoint.path, "main.rs");
                    assert_eq!(edit.checkpoint.before_content, "before");
                    assert_eq!(edit.checkpoint.after_content, "after");
                }
                _ => panic!("expected file edit"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_line_json_round_trips_escaped_tool_result_without_offset() {
        let line = SessionLine {
            timestamp: "123".to_owned(),
            item: SessionItem::EventMsg(Event::new(
                "evt-2",
                EventMsg::ToolResult(ToolResultEvent {
                    call_id: "call-0".to_owned(),
                    status: ToolResultStatus::Success,
                    content: "quote: \"hello\" slash: \\ newline:\nend".to_owned(),
                    truncated: false,
                    next_offset: None,
                    edits: Vec::new(),
                }),
            )),
        };

        let json = session_line_to_json(&line);
        let parsed = session_line_from_json(&json).unwrap();

        match parsed.item {
            SessionItem::EventMsg(event) => match event.msg {
                EventMsg::ToolResult(result) => {
                    assert_eq!(result.content, "quote: \"hello\" slash: \\ newline:\nend");
                    assert_eq!(result.next_offset, None);
                }
                _ => panic!("expected tool result"),
            },
            SessionItem::SessionMeta(_) => panic!("expected event msg"),
        }
    }

    #[test]
    fn session_paths_use_jsonl_extension() {
        let session = Session::new("abc", Path::new("/tmp/project"));

        assert_eq!(session.id, "abc");
        assert!(session.path.ends_with(".picocode/sessions/abc.jsonl"));
    }

    #[test]
    fn list_session_summaries_reports_counts_and_recency() {
        let root = env::temp_dir().join(format!("picocode-session-test-{}", epoch_millis()));
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);

        let first = store.create_session().unwrap();
        store
            .append_event(&first, &Event::new("evt-1", EventMsg::user("one")))
            .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let second = store.create_session().unwrap();
        store
            .append_event(&second, &Event::new("evt-2", EventMsg::user("two")))
            .unwrap();
        store
            .append_event(&second, &Event::new("evt-3", EventMsg::assistant("done")))
            .unwrap();

        let summaries = store.list_session_summaries().unwrap();

        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].session.id, second.id);
        assert_eq!(summaries[0].event_count, 2);
        assert_eq!(summaries[0].cwd.as_deref(), Some(root.to_str().unwrap()));
        assert!(summaries[0].last_timestamp.is_some());
        assert_eq!(summaries[1].session.id, first.id);
        assert_eq!(summaries[1].event_count, 1);
        assert_eq!(summaries[0].ai_provider.as_deref(), None);
        assert_eq!(summaries[0].ai_model.as_deref(), None);
        assert_eq!(summaries[0].parent_session_id.as_deref(), None);
    }

    #[test]
    fn fork_session_copies_history_and_sets_parent_session_id() {
        let root = env::temp_dir().join(format!("picocode-session-fork-{}", epoch_millis()));
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);

        let original = store.create_session().unwrap();
        store
            .append_event(&original, &Event::new("evt-1", EventMsg::user("hello")))
            .unwrap();
        store
            .append_event(&original, &Event::new("evt-2", EventMsg::assistant("hi")))
            .unwrap();

        let forked = store.fork_session(&original).unwrap();
        let summary = store.summarize_session(&forked).unwrap();
        let events = store.load_events(&forked).unwrap();

        assert_eq!(
            summary.parent_session_id.as_deref(),
            Some(original.id.as_str())
        );
        assert_eq!(summary.event_count, 2);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].msg.content(), "hello");
        assert_eq!(events[1].msg.content(), "hi");
    }

    #[test]
    fn fork_session_inserts_branch_summary_after_compaction() {
        let root = env::temp_dir().join(format!("picocode-session-branch-{}", epoch_millis()));
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);

        let original = store.create_session().unwrap();
        store
            .append_event(&original, &Event::new("evt-1", EventMsg::user("hello")))
            .unwrap();
        store
            .append_event(
                &original,
                &Event::new(
                    "evt-2",
                    EventMsg::Compaction(CompactionEvent {
                        summary: "Folded 1 event".to_owned(),
                        folded_event_count: 1,
                    }),
                ),
            )
            .unwrap();
        store
            .append_event(&original, &Event::new("evt-3", EventMsg::assistant("hi")))
            .unwrap();

        let forked = store.fork_session(&original).unwrap();
        let events = store.load_events(&forked).unwrap();

        assert_eq!(events.len(), 2);
        match &events[0].msg {
            EventMsg::BranchSummary(summary) => {
                assert_eq!(summary.source_session_id, original.id);
                assert_eq!(summary.folded_event_count, 1);
            }
            _ => panic!("expected branch summary"),
        }
        assert_eq!(events[1].msg.content(), "hi");
    }

    #[test]
    fn export_and_share_session_html_write_files() {
        let root = env::temp_dir().join(format!("picocode-session-export-{}", epoch_millis()));
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root);

        let session = store.create_session().unwrap();
        store
            .append_event(
                &session,
                &Event::new("evt-1", EventMsg::user("hello export")),
            )
            .unwrap();

        let export_path = store.export_session_html(&session).unwrap();
        let share_path = store.share_session_html(&session).unwrap();

        assert!(export_path
            .to_string_lossy()
            .contains(&format!(".picocode/exports/{}.html", session.id)));
        assert!(share_path
            .to_string_lossy()
            .contains(&format!(".picocode/shares/{}.html", session.id)));

        let export_html = fs::read_to_string(&export_path).unwrap();
        let share_html = fs::read_to_string(&share_path).unwrap();

        assert!(export_html.contains("PicoCode export"));
        assert!(export_html.contains(&session.id));
        assert!(export_html.contains("hello export"));
        assert!(share_html.contains("PicoCode share"));
        assert!(share_html.contains("hello export"));
    }
}
