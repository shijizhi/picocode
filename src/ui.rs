use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};
use std::collections::{HashMap, HashSet};
use unicode_width::UnicodeWidthStr;

use crate::{
    app::AppState,
    config_editor::{ConfigEditorState, ConfigField},
    event::{
        CommandOutputEvent, CommandRunEvent, EventMsg, ImageAttachmentEvent, ToolCallEvent,
        ToolResultEvent,
    },
    model_picker::ModelPickerState,
    session_picker::SessionPickerState,
    session_tree::{tree_window, SessionTreeState, TreeRow},
};

const EDITOR_HEIGHT: u16 = 3;
const FOOTER_HEIGHT: u16 = 1;
const HEADER_HEIGHT: u16 = 6;
const TIP_HEIGHT: u16 = 2;
const TRANSCRIPT_EDITOR_GAP: u16 = 1;
const EDITOR_PROMPT: &str = "> ";
const DEFAULT_TOOL_RESULT_DISPLAY_LINES: usize = 20;
const FIND_RESULT_DISPLAY_LINES: usize = 20;
const GREP_RESULT_DISPLAY_LINES: usize = 15;
const READ_RESULT_DISPLAY_LINES: usize = 40;

mod dracula {
    use ratatui::style::Color;

    pub const BACKGROUND: Color = Color::Rgb(40, 42, 54);
    pub const CURRENT_LINE: Color = Color::Rgb(68, 71, 90);
    pub const FOREGROUND: Color = Color::Rgb(248, 248, 242);
    pub const COMMENT: Color = Color::Rgb(98, 114, 164);
    pub const ANSWER: Color = Color::Rgb(198, 202, 220);
    pub const PROCESS: Color = Color::Rgb(139, 145, 173);
    pub const RED: Color = Color::Rgb(255, 85, 85);
    pub const ORANGE: Color = Color::Rgb(255, 184, 108);
    pub const YELLOW: Color = Color::Rgb(241, 250, 140);
    pub const GREEN: Color = Color::Rgb(80, 250, 123);
    pub const CYAN: Color = Color::Rgb(139, 233, 253);
    pub const PURPLE: Color = Color::Rgb(189, 147, 249);
}

pub fn render(frame: &mut Frame<'_>, app: &AppState) {
    if app.is_config_editor_active() {
        if let crate::app::AppMode::ConfigEditor(state) = &app.mode {
            render_config_editor(frame, state);
        }
        return;
    }
    if app.is_model_picker_active() {
        if let crate::app::AppMode::ModelPicker(state) = &app.mode {
            render_model_picker(frame, state);
        }
        return;
    }
    if app.is_session_tree_active() {
        if let crate::app::AppMode::SessionTree(state) = &app.mode {
            render_session_tree(frame, state);
        }
        return;
    }
    if app.is_session_picker_active() {
        if let crate::app::AppMode::SessionPicker(state) = &app.mode {
            render_session_picker(frame, state);
        }
        return;
    }

    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);
    let show_home = app.input.is_empty()
        && app.submissions.is_empty()
        && !app
            .events
            .iter()
            .any(|event| !matches!(event.msg, EventMsg::SystemMessage(_)));
    let chunks = if show_home {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(HEADER_HEIGHT),
                Constraint::Length(TIP_HEIGHT),
                Constraint::Min(1),
                Constraint::Length(TRANSCRIPT_EDITOR_GAP),
                Constraint::Length(EDITOR_HEIGHT),
                Constraint::Length(FOOTER_HEIGHT),
            ])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(TRANSCRIPT_EDITOR_GAP),
                Constraint::Length(EDITOR_HEIGHT),
                Constraint::Length(FOOTER_HEIGHT),
            ])
            .split(area)
    };

    if show_home {
        render_home_header(frame, chunks[0], app);
        render_home_tip(frame, chunks[1], app);
        render_transcript(frame, chunks[2], app);
        render_editor(frame, chunks[4], app);
        render_footer(frame, chunks[5], app);
    } else {
        render_transcript(frame, chunks[0], app);
        render_editor(frame, chunks[2], app);
        render_footer(frame, chunks[3], app);
    }
}

fn render_home_header(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let profile = app
        .ai_profile
        .as_ref()
        .map(|profile: &crate::app::AiProfile| profile.label())
        .unwrap_or_else(|| "none".to_owned());
    let workspace_root = app.workspace_root.as_deref().unwrap_or(".");
    let title = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            ">_ PicoCode",
            Style::default()
                .fg(dracula::FOREGROUND)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("model: ", Style::default().fg(dracula::COMMENT)),
            Span::styled(profile, Style::default().fg(dracula::FOREGROUND)),
            Span::raw("   "),
            Span::styled("/model", Style::default().fg(dracula::CYAN)),
            Span::styled(" to change", Style::default().fg(dracula::COMMENT)),
        ]),
        Line::from(vec![
            Span::styled("directory: ", Style::default().fg(dracula::COMMENT)),
            Span::styled(workspace_root, Style::default().fg(dracula::ANSWER)),
        ]),
        Line::from(vec![
            Span::styled("status: ", Style::default().fg(dracula::COMMENT)),
            Span::styled(
                app.runtime_status.label.as_str(),
                Style::default().fg(dracula::GREEN),
            ),
            Span::raw(" "),
            Span::styled(
                runtime_elapsed_text(&app.runtime_status),
                Style::default().fg(dracula::YELLOW),
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(dracula::CURRENT_LINE)),
    )
    .style(base_style())
    .wrap(Wrap { trim: false });

    frame.render_widget(title, area);
}

fn render_home_tip(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let lines = if app.workspace_root.is_some() {
        vec![
            Line::from(vec![
                Span::styled("Tip: ", Style::default().fg(dracula::ORANGE)),
                Span::styled(
                    "Use /new for a blank session, Ctrl+V to paste text.",
                    Style::default().fg(dracula::PROCESS),
                ),
            ]),
            Line::from(vec![
                Span::styled("      ", Style::default().fg(dracula::ORANGE)),
                Span::styled(
                    "/image <path|clip> to attach a screenshot, /skill <query> to load a skill, /capabilities to browse enabled capabilities.",
                    Style::default().fg(dracula::PROCESS),
                ),
            ]),
        ]
    } else {
        vec![
            Line::from(vec![
                Span::styled("Tip: ", Style::default().fg(dracula::ORANGE)),
                Span::styled(
                    "Use /new for a blank session.",
                    Style::default().fg(dracula::PROCESS),
                ),
            ]),
            Line::from(vec![
                Span::styled("      ", Style::default().fg(dracula::ORANGE)),
                Span::styled(
                    "/resume to continue a session, /model to switch models.",
                    Style::default().fg(dracula::PROCESS),
                ),
            ]),
        ]
    };
    let tip = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(tip, area);
}

pub fn render_session_picker(frame: &mut Frame<'_>, state: &SessionPickerState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_session_picker_header(frame, chunks[0], state);
    render_session_picker_list(frame, chunks[1], state);
    render_session_picker_footer(frame, chunks[2], state);
}

pub fn render_session_tree(frame: &mut Frame<'_>, state: &SessionTreeState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_session_tree_header(frame, chunks[0], state);
    render_session_tree_list(frame, chunks[1], state);
    render_session_tree_footer(frame, chunks[2], state);
}

pub fn render_model_picker(frame: &mut Frame<'_>, state: &ModelPickerState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_model_picker_header(frame, chunks[0], state);
    render_model_picker_list(frame, chunks[1], state);
    render_model_picker_footer(frame, chunks[2], state);
}

pub fn render_config_editor(frame: &mut Frame<'_>, state: &ConfigEditorState) {
    let area = frame.area();
    frame.render_widget(Block::default().style(base_style()), area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_config_editor_header(frame, chunks[0], state);
    render_config_editor_fields(frame, chunks[1], state);
    render_config_editor_footer(frame, chunks[2], state);
}

fn render_config_editor_header(frame: &mut Frame<'_>, area: Rect, state: &ConfigEditorState) {
    let mut lines = vec![
        Line::from(vec![Span::styled(
            "config editor",
            Style::default()
                .fg(dracula::PURPLE)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            state.summary_label(),
            Style::default().fg(dracula::COMMENT),
        )]),
        Line::from(vec![Span::styled(
            "Left/Right model  ·  Up/Down field  ·  Enter edit/toggle",
            Style::default().fg(dracula::ORANGE),
        )]),
    ];
    if let Some(prompt) = state.prompt_label() {
        lines.push(Line::from(vec![Span::styled(
            prompt,
            Style::default().fg(dracula::CYAN),
        )]));
    } else {
        lines.push(Line::from(vec![Span::styled(
            "Ctrl+N new model  ·  Ctrl+D delete  ·  Ctrl+S save  ·  Esc cancel",
            Style::default().fg(dracula::COMMENT),
        )]));
    }

    let header = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(header, area);
}

fn render_config_editor_fields(frame: &mut Frame<'_>, area: Rect, state: &ConfigEditorState) {
    let Some(option) = state.current_option() else {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "no model configured",
            Style::default().fg(dracula::RED),
        )]))
        .style(base_style());
        frame.render_widget(empty, area);
        return;
    };

    let mut lines = Vec::new();
    for field in ConfigField::all() {
        let selected = field == state.selected_field;
        let style = if selected {
            Style::default()
                .fg(dracula::BACKGROUND)
                .bg(dracula::GREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dracula::ANSWER)
        };
        let value = field.display_value(option);
        let label_style = if selected {
            Style::default().fg(dracula::BACKGROUND).bg(dracula::GREEN)
        } else {
            Style::default().fg(dracula::COMMENT)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}: ", field.label()),
                label_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(value, style),
        ]));
    }

    let body = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(body, area);
}

fn render_config_editor_footer(frame: &mut Frame<'_>, area: Rect, state: &ConfigEditorState) {
    let footer = if state.prompt_label().is_some() {
        Paragraph::new(Line::from(vec![
            Span::styled("Enter ", Style::default().fg(dracula::GREEN)),
            Span::styled("save", Style::default().fg(dracula::COMMENT)),
            Span::raw("  "),
            Span::styled("Esc ", Style::default().fg(dracula::RED)),
            Span::styled("cancel", Style::default().fg(dracula::COMMENT)),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("Enter ", Style::default().fg(dracula::GREEN)),
            Span::styled("edit", Style::default().fg(dracula::COMMENT)),
            Span::raw("  "),
            Span::styled("Space ", Style::default().fg(dracula::GREEN)),
            Span::styled("toggle", Style::default().fg(dracula::COMMENT)),
        ]))
    };
    frame.render_widget(footer.style(base_style().bg(dracula::CURRENT_LINE)), area);
}

fn render_session_picker_header(frame: &mut Frame<'_>, area: Rect, state: &SessionPickerState) {
    let filtered = state.filtered_summaries();
    let prompt = state.prompt_label().unwrap_or_else(|| "".to_owned());
    let title = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "resume sessions",
            Style::default()
                .fg(dracula::PURPLE)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!(
                "search: {}  ·  {} result(s)  ·  {}  ·  sort: {}",
                if state.query.is_empty() {
                    "<type to filter>".to_owned()
                } else {
                    state.query.clone()
                },
                filtered.len(),
                if state.show_paths {
                    "paths on"
                } else {
                    "paths off"
                },
                if state.newest_first {
                    "newest"
                } else {
                    "oldest"
                }
            ),
            Style::default().fg(dracula::COMMENT),
        )]),
        Line::from(vec![Span::styled(
            prompt,
            Style::default().fg(dracula::ORANGE),
        )]),
    ])
    .style(base_style());

    frame.render_widget(title, area);
}

fn render_session_picker_list(frame: &mut Frame<'_>, area: Rect, state: &SessionPickerState) {
    let filtered = state.filtered_summaries();
    let lines = session_picker_lines(&filtered, state, area.height as usize);
    let list = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(list, area);
}

fn render_session_picker_footer(frame: &mut Frame<'_>, area: Rect, _state: &SessionPickerState) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Enter ", Style::default().fg(dracula::GREEN)),
        Span::styled("resume", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Esc ", Style::default().fg(dracula::RED)),
        Span::styled("cancel", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+P ", Style::default().fg(dracula::CYAN)),
        Span::styled("paths", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+S ", Style::default().fg(dracula::CYAN)),
        Span::styled("sort", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+R ", Style::default().fg(dracula::CYAN)),
        Span::styled("rename", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+D ", Style::default().fg(dracula::CYAN)),
        Span::styled("delete", Style::default().fg(dracula::COMMENT)),
    ]))
    .style(base_style().bg(dracula::CURRENT_LINE));
    frame.render_widget(footer, area);
}

fn render_session_tree_header(frame: &mut Frame<'_>, area: Rect, state: &SessionTreeState) {
    let (_, rows) = tree_window(state, area.height as usize);
    let title = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "session tree",
            Style::default()
                .fg(dracula::PURPLE)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!(
                "search: {}  ·  {} result(s)  ·  {}  ·  sort: {}",
                if state.query.is_empty() {
                    "<type to filter>".to_owned()
                } else {
                    state.query.clone()
                },
                rows.len(),
                if state.show_paths {
                    "paths on"
                } else {
                    "paths off"
                },
                if state.newest_first {
                    "newest"
                } else {
                    "oldest"
                }
            ),
            Style::default().fg(dracula::COMMENT),
        )]),
        Line::from(vec![Span::styled(
            state.prompt_label().unwrap_or_default(),
            Style::default().fg(dracula::ORANGE),
        )]),
    ])
    .style(base_style());

    frame.render_widget(title, area);
}

fn render_session_tree_list(frame: &mut Frame<'_>, area: Rect, state: &SessionTreeState) {
    let (visible_start, rows) = tree_window(state, area.height as usize);
    if rows.is_empty() {
        let list = Paragraph::new(Line::from(vec![Span::styled(
            "no sessions found",
            Style::default().fg(dracula::COMMENT),
        )]))
        .style(base_style());
        frame.render_widget(list, area);
        return;
    }

    let lines = session_tree_lines(&rows, state, visible_start);
    let list = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(list, area);
}

fn render_session_tree_footer(frame: &mut Frame<'_>, area: Rect, _state: &SessionTreeState) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Enter ", Style::default().fg(dracula::GREEN)),
        Span::styled("resume", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+F ", Style::default().fg(dracula::CYAN)),
        Span::styled("fork", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Esc ", Style::default().fg(dracula::RED)),
        Span::styled("cancel", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+P ", Style::default().fg(dracula::CYAN)),
        Span::styled("paths", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+S ", Style::default().fg(dracula::CYAN)),
        Span::styled("sort", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+R ", Style::default().fg(dracula::CYAN)),
        Span::styled("rename", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Ctrl+D ", Style::default().fg(dracula::CYAN)),
        Span::styled("delete", Style::default().fg(dracula::COMMENT)),
    ]))
    .style(base_style().bg(dracula::CURRENT_LINE));
    frame.render_widget(footer, area);
}

fn session_tree_lines<'a>(
    rows: &'a [TreeRow<'a>],
    state: &'a SessionTreeState,
    visible_start: usize,
) -> Vec<Line<'a>> {
    rows.iter()
        .enumerate()
        .map(|(offset, row)| {
            let selected = visible_start + offset == state.selected;
            let style = if selected {
                Style::default()
                    .fg(dracula::BACKGROUND)
                    .bg(dracula::GREEN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(dracula::ANSWER)
            };
            let mut text = format!(
                "{}{}  events={}  {}  last={}",
                row.prefix,
                row.summary.session.id,
                row.summary.event_count,
                row.summary.stats.compact_label(),
                row.summary.last_timestamp.as_deref().unwrap_or("-")
            );
            if state.show_paths {
                text.push_str(&format!(
                    "  cwd={}",
                    row.summary.cwd.as_deref().unwrap_or("-")
                ));
            }
            Line::from(vec![
                Span::styled(if selected { "▸ " } else { "  " }, style),
                Span::styled(text, style),
            ])
        })
        .collect()
}

fn render_model_picker_header(frame: &mut Frame<'_>, area: Rect, state: &ModelPickerState) {
    let filtered = state.filtered_options();
    let title = Paragraph::new(vec![
        Line::from(vec![Span::styled(
            "model selector",
            Style::default()
                .fg(dracula::PURPLE)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::styled(
            format!(
                "search: {}  ·  {} result(s)  ·  current: {}",
                if state.query.is_empty() {
                    "<type to filter>".to_owned()
                } else {
                    state.query.clone()
                },
                filtered.len(),
                state.current_label()
            ),
            Style::default().fg(dracula::COMMENT),
        )]),
        Line::from(vec![Span::styled(
            "Enter select  ·  Esc cancel",
            Style::default().fg(dracula::ORANGE),
        )]),
    ])
    .style(base_style());

    frame.render_widget(title, area);
}

fn render_model_picker_list(frame: &mut Frame<'_>, area: Rect, state: &ModelPickerState) {
    let filtered = state.filtered_options();
    let lines = model_picker_lines(&filtered, state, area.height as usize);
    let list = Paragraph::new(lines)
        .style(base_style())
        .wrap(Wrap { trim: false });
    frame.render_widget(list, area);
}

fn render_model_picker_footer(frame: &mut Frame<'_>, area: Rect, _state: &ModelPickerState) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("Enter ", Style::default().fg(dracula::GREEN)),
        Span::styled("select", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("Esc ", Style::default().fg(dracula::RED)),
        Span::styled("cancel", Style::default().fg(dracula::COMMENT)),
    ]))
    .style(base_style().bg(dracula::CURRENT_LINE));
    frame.render_widget(footer, area);
}

fn model_picker_lines<'a>(
    options: &[&crate::config::ModelOption],
    state: &ModelPickerState,
    area_height: usize,
) -> Vec<Line<'a>> {
    if options.is_empty() {
        return vec![Line::from(vec![Span::styled(
            "no models found",
            Style::default().fg(dracula::COMMENT),
        )])];
    }

    let visible = picker_visible_window(options.len(), area_height, state.selected);
    let mut lines = Vec::new();
    for (index, option) in options
        .iter()
        .enumerate()
        .skip(visible.start)
        .take(visible.end.saturating_sub(visible.start))
    {
        let selected = index == state.selected;
        let style = if selected {
            Style::default()
                .fg(dracula::BACKGROUND)
                .bg(dracula::GREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dracula::ANSWER)
        };
        let mut text = format!("{}/{}  api={}", option.provider, option.model, option.api);
        let mut caps = Vec::new();
        if option.tools {
            caps.push("tools");
        }
        if option.images {
            caps.push("images");
        }
        if option.reasoning {
            caps.push("reasoning");
        }
        if !caps.is_empty() {
            text.push_str(&format!("  [{}]", caps.join(", ")));
        }
        lines.push(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, style),
            Span::styled(text, style),
        ]));
    }

    lines
}

fn session_picker_lines<'a>(
    summaries: &[&crate::session::SessionSummary],
    state: &SessionPickerState,
    area_height: usize,
) -> Vec<Line<'a>> {
    if summaries.is_empty() {
        return vec![Line::from(vec![Span::styled(
            "no sessions found",
            Style::default().fg(dracula::COMMENT),
        )])];
    }

    let visible = picker_visible_window(summaries.len(), area_height, state.selected);
    let mut lines = Vec::new();
    for (index, summary) in summaries
        .iter()
        .enumerate()
        .skip(visible.start)
        .take(visible.end.saturating_sub(visible.start))
    {
        let selected = index == state.selected;
        let style = if selected {
            Style::default()
                .fg(dracula::BACKGROUND)
                .bg(dracula::GREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dracula::ANSWER)
        };
        let path = summary.cwd.as_deref().unwrap_or("-");
        let mut text = format!(
            "{}  events={}  {}  last={}",
            summary.session.id,
            summary.event_count,
            summary.stats.compact_label(),
            summary.last_timestamp.as_deref().unwrap_or("-")
        );
        if state.show_paths {
            text.push_str(&format!("  cwd={path}"));
        }
        lines.push(Line::from(vec![
            Span::styled(if selected { "▸ " } else { "  " }, style),
            Span::styled(text, style),
        ]));
    }
    lines
}

#[derive(Debug, Clone, Copy)]
struct PickerWindow {
    start: usize,
    end: usize,
}

fn picker_visible_window(total: usize, area_height: usize, selected: usize) -> PickerWindow {
    if total == 0 || area_height == 0 {
        return PickerWindow { start: 0, end: 0 };
    }

    let height = area_height.min(total);
    let mut start = selected.saturating_sub(height / 2);
    if start + height > total {
        start = total.saturating_sub(height);
    }

    PickerWindow {
        start,
        end: start + height,
    }
}

fn render_transcript(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let lines = transcript_lines(app);
    let visible_lines =
        visible_transcript_lines(&lines, area.height as usize, app.transcript_scroll);
    let transcript = Paragraph::new(visible_lines)
        .style(base_style())
        .wrap(Wrap { trim: false });

    frame.render_widget(transcript, area);
}

fn transcript_lines(app: &AppState) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    let mut last_tool_name = ToolName::Unknown;
    let command_outputs = command_outputs_by_call_id(&app.events);
    let command_runs = command_run_call_ids(&app.events);
    let start_index = app
        .events
        .iter()
        .rposition(|event| {
            matches!(
                event.msg,
                EventMsg::Compaction(_) | EventMsg::BranchSummary(_)
            )
        })
        .unwrap_or(0);

    for event in &app.events[start_index..] {
        let rendered = match &event.msg {
            EventMsg::CommandRun(command_run) => Some(command_block_lines(
                &command_run,
                command_outputs
                    .get(&command_run.call_id)
                    .map_or(&[], |outputs| outputs.as_slice()),
            )),
            EventMsg::CommandOutput(output) if command_runs.contains(&output.call_id) => None,
            _ => Some(event_lines(&event.msg, last_tool_name)),
        };

        let Some(rendered) = rendered else {
            continue;
        };

        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.extend(rendered);
        if let EventMsg::ToolCall(tool_call) = &event.msg {
            last_tool_name = ToolName::from_str(&tool_call.name);
        }
    }

    lines
}

fn command_run_call_ids(events: &[crate::event::Event]) -> HashSet<String> {
    events
        .iter()
        .filter_map(|event| match &event.msg {
            EventMsg::CommandRun(command_run) => Some(command_run.call_id.clone()),
            _ => None,
        })
        .collect()
}

fn command_outputs_by_call_id(
    events: &[crate::event::Event],
) -> HashMap<String, Vec<CommandOutputEvent>> {
    let mut outputs_by_call_id: HashMap<String, Vec<CommandOutputEvent>> = HashMap::new();
    for event in events {
        if let EventMsg::CommandOutput(output) = &event.msg {
            outputs_by_call_id
                .entry(output.call_id.clone())
                .or_default()
                .push(output.clone());
        }
    }
    outputs_by_call_id
}

fn event_lines(event: &EventMsg, last_tool_name: ToolName) -> Vec<Line<'_>> {
    match event {
        EventMsg::UserMessage(_) => user_message_lines(event.content()),
        EventMsg::AssistantMessage(_) => assistant_lines(event.content()),
        EventMsg::ImageAttachment(event) => image_attachment_lines(event),
        EventMsg::ToolCall(event) => tool_call_lines(event),
        EventMsg::ToolResult(event) => tool_result_lines(event, last_tool_name),
        EventMsg::CommandRun(event) => command_run_lines(event),
        EventMsg::CommandOutput(event) => command_output_lines(event),
        EventMsg::Compaction(event) => compaction_lines(event),
        EventMsg::BranchSummary(event) => branch_summary_lines(event),
        EventMsg::FileEdit(event) => file_edit_lines(event),
        EventMsg::Error(_) => boxed_message("error", event.content(), dracula::RED),
        EventMsg::SystemMessage(_) | EventMsg::Final(_) => labeled_content_lines(event),
    }
}

fn labeled_content_lines(event: &EventMsg) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        event.label(),
        event_style(event).add_modifier(Modifier::BOLD),
    )])];
    lines.extend(content_lines(event.content(), event_style(event), None));
    lines
}

fn user_message_lines(content: &str) -> Vec<Line<'_>> {
    content_lines(content, Style::default().fg(dracula::GREEN), Some("▌ "))
}

fn boxed_message<'a>(label: &'a str, content: &'a str, color: Color) -> Vec<Line<'a>> {
    let style = Style::default().fg(color);
    let border_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("▌ {label}"),
        border_style,
    )])];
    lines.extend(content_lines(content, style, Some("  ")));
    lines
}

fn assistant_lines(content: &str) -> Vec<Line<'_>> {
    content_lines(content, Style::default().fg(dracula::ANSWER), None)
}

fn image_attachment_lines(event: &ImageAttachmentEvent) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        "▌ image attached",
        Style::default()
            .fg(dracula::CYAN)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ {}", event.file_name),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines.push(Line::from(vec![Span::styled(
        event.summary.as_str(),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!("source: {}  mime: {}", event.source_path, event.mime_type),
        Style::default().fg(dracula::COMMENT),
    )]));
    lines.push(Line::from(vec![Span::styled(
        "queued for the next prompt",
        Style::default().fg(dracula::PROCESS),
    )]));
    lines
}

fn compaction_lines(event: &crate::event::CompactionEvent) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        "session compacted · continue",
        Style::default()
            .fg(dracula::YELLOW)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ folded {} event(s)", event.folded_event_count),
        Style::default().fg(dracula::COMMENT),
    )]));
    lines.extend(content_lines(
        event.summary.as_str(),
        Style::default().fg(dracula::PROCESS),
        Some("│ "),
    ));
    lines
}

fn branch_summary_lines(event: &crate::event::BranchSummaryEvent) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("branch summary · from {}", event.source_session_id),
        Style::default()
            .fg(dracula::YELLOW)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ folded {} event(s)", event.folded_event_count),
        Style::default().fg(dracula::COMMENT),
    )]));
    lines.extend(content_lines(
        event.summary.as_str(),
        Style::default().fg(dracula::PROCESS),
        Some("│ "),
    ));
    lines
}

fn file_edit_lines(event: &crate::event::FileEditEvent) -> Vec<Line<'_>> {
    let title = match event.action {
        crate::event::FileEditAction::Applied => "# Edit checkpoint",
        crate::event::FileEditAction::RolledBack => "# Rewind edit",
    };
    let mut lines = vec![Line::from(vec![Span::styled(
        title,
        Style::default()
            .fg(dracula::CYAN)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ {}", event.summary),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines.extend(content_lines(
        event.checkpoint.diff.as_str(),
        Style::default().fg(dracula::PROCESS),
        Some("│ "),
    ));
    lines
}

fn command_run_lines(event: &CommandRunEvent) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        "# Run command",
        Style::default()
            .fg(dracula::CYAN)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ {}", event.command),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!("cwd: {}  timeout: {}s", event.cwd, event.timeout_seconds),
        Style::default().fg(dracula::COMMENT),
    )]));
    lines
}

fn command_output_lines(event: &CommandOutputEvent) -> Vec<Line<'_>> {
    let prefix = if event.stream == "stderr" {
        "stderr"
    } else {
        "stdout"
    };
    vec![Line::from(vec![
        Span::styled(prefix, Style::default().fg(dracula::ORANGE)),
        Span::raw(" "),
        Span::styled(
            event.content.as_str(),
            Style::default().fg(dracula::PROCESS),
        ),
    ])]
}

fn command_block_lines<'a>(
    command_run: &CommandRunEvent,
    outputs: &[CommandOutputEvent],
) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        "# Run command",
        Style::default()
            .fg(dracula::CYAN)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ {}", command_run.command),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines.push(Line::from(vec![Span::styled(
        format!(
            "cwd: {}  timeout: {}s",
            command_run.cwd, command_run.timeout_seconds
        ),
        Style::default().fg(dracula::COMMENT),
    )]));

    let stdout = outputs
        .iter()
        .filter(|output| output.stream != "stderr")
        .map(|output| output.content.as_str())
        .collect::<Vec<_>>();
    let stderr = outputs
        .iter()
        .filter(|output| output.stream == "stderr")
        .map(|output| output.content.as_str())
        .collect::<Vec<_>>();

    lines.extend(command_stream_preview_lines(
        "stdout",
        &stdout,
        dracula::PROCESS,
    ));
    lines.extend(command_stream_preview_lines(
        "stderr",
        &stderr,
        dracula::ORANGE,
    ));
    lines
}

fn command_stream_preview_lines(
    stream: &str,
    entries: &[&str],
    color: Color,
) -> Vec<Line<'static>> {
    const PREVIEW_LIMIT: usize = 4;

    if entries.is_empty() {
        return vec![Line::from(vec![Span::styled(
            format!("{stream}: (empty)"),
            Style::default().fg(dracula::COMMENT),
        )])];
    }

    let mut lines = Vec::new();
    lines.push(Line::from(vec![Span::styled(
        format!("{stream}: {} line(s)", entries.len()),
        Style::default().fg(dracula::COMMENT),
    )]));
    for line in entries.iter().take(PREVIEW_LIMIT) {
        lines.push(Line::from(vec![Span::styled(
            format!("│ {}", line),
            Style::default().fg(color),
        )]));
    }
    if entries.len() > PREVIEW_LIMIT {
        lines.push(Line::from(vec![Span::styled(
            format!(
                "│ ... {} more lines (collapsed)",
                entries.len() - PREVIEW_LIMIT
            ),
            Style::default().fg(dracula::COMMENT),
        )]));
    }
    lines
}

fn tool_call_lines(event: &ToolCallEvent) -> Vec<Line<'_>> {
    let mut lines = vec![Line::from(vec![Span::styled(
        format!("# {}", tool_title(&event.name)),
        Style::default()
            .fg(dracula::PURPLE)
            .add_modifier(Modifier::BOLD),
    )])];
    lines.push(Line::from(vec![Span::styled(
        format!("$ {}", tool_command(event)),
        Style::default().fg(dracula::PROCESS),
    )]));
    lines
}

fn tool_result_lines(event: &ToolResultEvent, tool_name: ToolName) -> Vec<Line<'_>> {
    let status_color = tool_result_color(event);
    let mut lines = vec![Line::from(vec![
        Span::styled("tool result", Style::default().fg(dracula::COMMENT)),
        Span::raw(" "),
        Span::styled(event.status.as_str(), Style::default().fg(status_color)),
        truncated_span(event),
    ])];
    let display_limit = tool_result_display_limit(tool_name);
    let content = folded_tool_result_content(event.content.as_str(), display_limit);
    lines.extend(content_lines(
        content.as_str(),
        Style::default().fg(dracula::PROCESS),
        Some("│ "),
    ));
    lines
}

fn folded_tool_result_content(content: &str, display_limit: usize) -> String {
    let mut content_lines = content.lines().collect::<Vec<_>>();
    if content_lines.len() <= display_limit {
        return content.to_owned();
    }

    let remaining = content_lines.len().saturating_sub(display_limit);
    content_lines.truncate(display_limit);
    let mut folded = content_lines.join("\n");
    folded.push('\n');
    folded.push_str(&format!("... {remaining} more lines (collapsed)"));
    folded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolName {
    FindFiles,
    SearchText,
    ProposeEdit,
    ProposeEditBatch,
    ApplyPatch,
    ApplyPatchBatch,
    RewindEdit,
    RunCommand,
    Read,
    Other,
    Unknown,
}

impl ToolName {
    fn from_str(name: &str) -> Self {
        match name {
            "find_files" | "find" => Self::FindFiles,
            "search_text" | "grep" => Self::SearchText,
            "propose_edit" => Self::ProposeEdit,
            "propose_edit_batch" => Self::ProposeEditBatch,
            "apply_patch" => Self::ApplyPatch,
            "apply_patch_batch" => Self::ApplyPatchBatch,
            "rewind_edit" | "rollback_edit" => Self::RewindEdit,
            "run_command" => Self::RunCommand,
            "read" => Self::Read,
            _ => Self::Other,
        }
    }
}

fn tool_result_display_limit(tool_name: ToolName) -> usize {
    match tool_name {
        ToolName::SearchText => GREP_RESULT_DISPLAY_LINES,
        ToolName::FindFiles => FIND_RESULT_DISPLAY_LINES,
        ToolName::ProposeEdit => READ_RESULT_DISPLAY_LINES,
        ToolName::ProposeEditBatch => READ_RESULT_DISPLAY_LINES,
        ToolName::ApplyPatch => READ_RESULT_DISPLAY_LINES,
        ToolName::ApplyPatchBatch => READ_RESULT_DISPLAY_LINES,
        ToolName::RewindEdit => READ_RESULT_DISPLAY_LINES,
        ToolName::RunCommand => DEFAULT_TOOL_RESULT_DISPLAY_LINES,
        ToolName::Read => READ_RESULT_DISPLAY_LINES,
        ToolName::Other | ToolName::Unknown => DEFAULT_TOOL_RESULT_DISPLAY_LINES,
    }
}

fn content_lines(content: &str, style: Style, prefix: Option<&str>) -> Vec<Line<'static>> {
    Text::raw(content)
        .lines
        .into_iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            match prefix {
                Some(prefix) => Line::from(vec![
                    Span::styled(prefix.to_owned(), Style::default().fg(dracula::COMMENT)),
                    Span::styled(text, style),
                ]),
                None => Line::from(Span::styled(text, style)),
            }
        })
        .collect()
}

fn tool_title(name: &str) -> &'static str {
    match name {
        "ls" => "List directory",
        "find_files" | "find" => "Find files",
        "search_text" | "grep" => "Search text",
        "propose_edit" => "Edit preview",
        "propose_edit_batch" => "Edit preview batch",
        "apply_patch" => "Apply patch",
        "apply_patch_batch" => "Apply patch batch",
        "rewind_edit" | "rollback_edit" => "Rewind edit",
        "run_command" => "Run command",
        "read" => "Read file",
        _ => "Run tool",
    }
}

fn tool_command(event: &ToolCallEvent) -> String {
    let args = event
        .arguments
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if args.is_empty() {
        event.name.clone()
    } else {
        format!("{} {}", event.name, args)
    }
}

fn tool_result_color(event: &ToolResultEvent) -> Color {
    if event.status.as_str() == "error" {
        dracula::RED
    } else if event.truncated {
        dracula::ORANGE
    } else {
        dracula::GREEN
    }
}

fn truncated_span(event: &ToolResultEvent) -> Span<'static> {
    if event.truncated {
        Span::styled(" · truncated", Style::default().fg(dracula::ORANGE))
    } else {
        Span::raw("")
    }
}

fn event_style(event: &EventMsg) -> Style {
    match event {
        EventMsg::SystemMessage(_) => Style::default().fg(dracula::COMMENT),
        EventMsg::UserMessage(_) => Style::default().fg(dracula::GREEN),
        EventMsg::AssistantMessage(_) => Style::default().fg(dracula::ANSWER),
        EventMsg::ImageAttachment(_) => Style::default().fg(dracula::CYAN),
        EventMsg::ToolCall(_) => Style::default().fg(dracula::PURPLE),
        EventMsg::ToolResult(event) if event.status.as_str() == "error" => {
            Style::default().fg(dracula::RED)
        }
        EventMsg::ToolResult(event) if event.truncated => Style::default().fg(dracula::ORANGE),
        EventMsg::ToolResult(_) => Style::default().fg(dracula::GREEN),
        EventMsg::CommandRun(_) => Style::default().fg(dracula::PURPLE),
        EventMsg::CommandOutput(_) => Style::default().fg(dracula::PROCESS),
        EventMsg::Compaction(_) | EventMsg::BranchSummary(_) => {
            Style::default().fg(dracula::YELLOW)
        }
        EventMsg::FileEdit(_) => Style::default().fg(dracula::CYAN),
        EventMsg::Error(_) => Style::default().fg(dracula::RED),
        EventMsg::Final(_) => Style::default().fg(dracula::CYAN),
    }
}

fn base_style() -> Style {
    Style::default()
        .fg(dracula::FOREGROUND)
        .bg(dracula::BACKGROUND)
}

fn visible_transcript_lines<'a>(
    lines: &'a [Line<'a>],
    visible_height: usize,
    scroll_from_bottom: usize,
) -> Vec<Line<'a>> {
    if visible_height == 0 {
        return Vec::new();
    }

    let max_start = lines.len().saturating_sub(visible_height);
    let start = max_start.saturating_sub(scroll_from_bottom.min(max_start));
    let end = start.saturating_add(visible_height).min(lines.len());

    lines[start..end].to_vec()
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let editor = Paragraph::new(Line::from(vec![
        Span::styled(EDITOR_PROMPT, Style::default().fg(dracula::COMMENT)),
        Span::styled(app.input.as_str(), Style::default().fg(dracula::FOREGROUND)),
    ]))
    .style(base_style().bg(dracula::CURRENT_LINE))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(dracula::CURRENT_LINE)),
    )
    .wrap(Wrap { trim: false });

    frame.render_widget(editor, area);

    let (cursor_x, cursor_y) = editor_cursor_position(area, &app.input);
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn editor_cursor_position(area: Rect, input: &str) -> (u16, u16) {
    let prompt_width = UnicodeWidthStr::width(EDITOR_PROMPT) as u16;
    let input_width = UnicodeWidthStr::width(input) as u16;
    let cursor_x = area
        .x
        .saturating_add(1)
        .saturating_add(prompt_width)
        .saturating_add(input_width)
        .min(area.right().saturating_sub(2));
    let cursor_y = area.y.saturating_add(1);

    (cursor_x, cursor_y)
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, app: &AppState) {
    let status = app.runtime_status.label.as_str();
    let detail = app.runtime_status.detail.as_deref().unwrap_or("");
    let elapsed = runtime_elapsed_text(&app.runtime_status);
    let footer = Paragraph::new(Line::from(vec![
        Span::styled("PicoCode ", Style::default().fg(dracula::PURPLE)),
        Span::styled("· ", Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("status ", Style::default().fg(dracula::COMMENT)),
        Span::styled(status, Style::default().fg(dracula::GREEN)),
        Span::raw(" "),
        Span::styled(elapsed, Style::default().fg(dracula::YELLOW)),
        Span::raw(" "),
        Span::styled(detail, Style::default().fg(dracula::COMMENT)),
        Span::raw("  "),
        Span::styled("exit ", Style::default().fg(dracula::COMMENT)),
        Span::raw("Esc/Ctrl+C"),
    ]))
    .style(base_style().bg(dracula::CURRENT_LINE));

    frame.render_widget(footer, area);
}

fn runtime_elapsed_text(status: &crate::app::RuntimeStatus) -> String {
    let elapsed = status
        .started_at
        .map(|started_at: std::time::Instant| started_at.elapsed())
        .unwrap_or_default();
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_transcript_defaults_to_latest_lines() {
        let lines = vec![
            Line::raw("one"),
            Line::raw("two"),
            Line::raw("three"),
            Line::raw("four"),
        ];

        let visible = visible_transcript_lines(&lines, 2, 0);

        assert_eq!(visible, vec![Line::raw("three"), Line::raw("four")]);
    }

    #[test]
    fn visible_transcript_can_scroll_up_from_bottom() {
        let lines = vec![
            Line::raw("one"),
            Line::raw("two"),
            Line::raw("three"),
            Line::raw("four"),
        ];

        let visible = visible_transcript_lines(&lines, 2, 1);

        assert_eq!(visible, vec![Line::raw("two"), Line::raw("three")]);
    }

    #[test]
    fn visible_transcript_clamps_large_scroll_to_top() {
        let lines = vec![
            Line::raw("one"),
            Line::raw("two"),
            Line::raw("three"),
            Line::raw("four"),
        ];

        let visible = visible_transcript_lines(&lines, 2, usize::MAX);

        assert_eq!(visible, vec![Line::raw("one"), Line::raw("two")]);
    }

    #[test]
    fn editor_cursor_uses_display_width_without_left_padding() {
        let area = Rect::new(10, 20, 20, 3);

        assert_eq!(editor_cursor_position(area, "abc"), (16, 21));
        assert_eq!(editor_cursor_position(area, "你是谁"), (19, 21));
    }

    #[test]
    fn content_lines_preserve_multiline_event_content() {
        let lines = content_lines("first\nsecond\nthird", Style::default(), None);

        assert_eq!(
            lines,
            vec![Line::raw("first"), Line::raw("second"), Line::raw("third")]
        );
    }

    #[test]
    fn tool_call_lines_render_semantic_action() {
        let event = ToolCallEvent::new("call-0", "search_text", "query=ToolRuntime\npath=src");
        let lines = tool_call_lines(&event);

        assert_eq!(line_text(&lines[0]), "# Search text");
        assert_eq!(
            line_text(&lines[1]),
            "$ search_text query=ToolRuntime path=src"
        );
    }

    #[test]
    fn user_message_lines_omit_label() {
        let lines = user_message_lines("hello");

        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            vec!["▌ hello"]
        );
    }

    #[test]
    fn image_attachment_lines_render_queue_hint() {
        let event = ImageAttachmentEvent {
            source_path: "./shot.png".to_owned(),
            file_name: "shot.png".to_owned(),
            mime_type: "image/png".to_owned(),
            byte_len: 12,
            data_url: "data:image/png;base64,AAAA".to_owned(),
            summary: "attached image: shot.png".to_owned(),
        };
        let lines = image_attachment_lines(&event);

        assert!(lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .contains(&"▌ image attached".to_owned()));
        assert!(lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .contains(&"queued for the next prompt".to_owned()));
    }

    #[test]
    fn tool_result_lines_indent_multiline_output() {
        let event = ToolResultEvent {
            call_id: "call-0".to_owned(),
            status: crate::tool::ToolResultStatus::Success,
            content: "path: src\nsrc/tool.rs:1:hit".to_owned(),
            truncated: false,
            next_offset: None,
            edits: Vec::new(),
        };
        let lines = tool_result_lines(&event, ToolName::SearchText);

        assert_eq!(
            lines.iter().map(line_text).collect::<Vec<_>>(),
            vec!["tool result success", "│ path: src", "│ src/tool.rs:1:hit"]
        );
    }

    #[test]
    fn tool_result_lines_fold_large_grep_output() {
        let content = (1..=18)
            .map(|index| format!("src/lib.rs:{index}:hit"))
            .collect::<Vec<_>>()
            .join("\n");
        let event = ToolResultEvent {
            call_id: "call-0".to_owned(),
            status: crate::tool::ToolResultStatus::Success,
            content,
            truncated: false,
            next_offset: None,
            edits: Vec::new(),
        };
        let lines = tool_result_lines(&event, ToolName::SearchText);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert_eq!(text.len(), 17);
        assert_eq!(text[1], "│ src/lib.rs:1:hit");
        assert_eq!(text[15], "│ src/lib.rs:15:hit");
        assert_eq!(text[16], "│ ... 3 more lines (collapsed)");
    }

    #[test]
    fn transcript_lines_collapses_command_stream_by_call_id() {
        let app = AppState::from_events(vec![
            crate::event::Event::new(
                "evt-0",
                EventMsg::command_output(crate::event::CommandOutputEvent {
                    call_id: "call-1".to_owned(),
                    stream: "stdout".to_owned(),
                    content: "alpha".to_owned(),
                    summary: "stdout: alpha".to_owned(),
                }),
            ),
            crate::event::Event::new("evt-1", EventMsg::system("interleaving event")),
            crate::event::Event::new(
                "evt-2",
                EventMsg::command_run(crate::event::CommandRunEvent {
                    call_id: "call-1".to_owned(),
                    command: "cat file.txt".to_owned(),
                    cwd: "/tmp/project".to_owned(),
                    timeout_seconds: 120,
                    summary: "$ cat file.txt".to_owned(),
                }),
            ),
            crate::event::Event::new(
                "evt-3",
                EventMsg::command_output(crate::event::CommandOutputEvent {
                    call_id: "call-1".to_owned(),
                    stream: "stderr".to_owned(),
                    content: "beta".to_owned(),
                    summary: "stderr: beta".to_owned(),
                }),
            ),
        ]);

        let lines = transcript_lines(&app);
        let text = lines.iter().map(line_text).collect::<Vec<_>>();

        assert!(text.contains(&"# Run command".to_owned()));
        assert!(text.iter().any(|line| line == "stdout: 1 line(s)"));
        assert!(text.iter().any(|line| line == "stderr: 1 line(s)"));
        assert!(text.iter().any(|line| line == "│ alpha"));
        assert!(text.iter().any(|line| line == "│ beta"));
        assert!(!text.iter().any(|line| line == "stdout alpha"));
        assert!(!text.iter().any(|line| line == "stderr beta"));
        assert!(text.iter().any(|line| line == "interleaving event"));
    }

    #[test]
    fn tool_result_display_limit_follows_tool_type() {
        assert_eq!(tool_result_display_limit(ToolName::SearchText), 15);
        assert_eq!(tool_result_display_limit(ToolName::FindFiles), 20);
        assert_eq!(tool_result_display_limit(ToolName::ProposeEdit), 40);
        assert_eq!(tool_result_display_limit(ToolName::ApplyPatch), 40);
        assert_eq!(tool_result_display_limit(ToolName::Read), 40);
        assert_eq!(tool_result_display_limit(ToolName::Other), 20);
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }
}
