use std::{
    io::{self, stdout},
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::{
    agent_core::AgentCore,
    ai::AiClient,
    app::{AiProfile, AppMode, AppState},
    capability::{CapabilityIndex, CapabilityKind, CapabilityPreferences},
    config::PicocodeConfig,
    config_editor::{ConfigEditorAction, ConfigField},
    event::{self as picocode_event, EventMsg},
    image::{attach_image, attach_image_from_clipboard, clipboard_text},
    instruction::load_instructions,
    model_picker::ModelPickerAction,
    session::{Session, SessionMeta, SessionStore},
    session_picker::{SessionPickerAction, SessionPickerState},
    session_tree::{SessionTreeAction, SessionTreeState},
    submission::{LocalCommand, Op, Submission},
    tool::ToolRuntime,
    ui,
};

pub fn pick_session(project_root: impl Into<PathBuf>) -> io::Result<Option<String>> {
    let project_root = project_root.into().canonicalize()?;
    let store = SessionStore::new(&project_root);
    let summaries = store.list_session_summaries()?;
    if summaries.is_empty() {
        return Ok(None);
    }

    let mut terminal = setup_terminal()?;
    let mut state = SessionPickerState::new(summaries);
    let result = run_session_picker(&mut terminal, &store, &mut state);
    restore_terminal(&mut terminal)?;
    result
}

pub fn pick_session_tree(project_root: impl Into<PathBuf>) -> io::Result<Option<String>> {
    let project_root = project_root.into().canonicalize()?;
    let store = SessionStore::new(&project_root);
    let summaries = store.list_session_summaries()?;
    if summaries.is_empty() {
        return Ok(None);
    }

    let mut terminal = setup_terminal()?;
    let mut state = SessionTreeState::new(summaries);
    let result = run_session_tree(&mut terminal, &store, &mut state);
    restore_terminal(&mut terminal)?;
    result
}

pub fn fork_session_from(project_root: impl Into<PathBuf>, session_id: &str) -> io::Result<String> {
    let project_root = project_root.into().canonicalize()?;
    let store = SessionStore::new(&project_root);
    let source = store.open_session(session_id);
    let forked = store.fork_session(&source)?;
    Ok(forked.id)
}

pub fn run(session_id: Option<&str>, project_root: impl Into<PathBuf>) -> io::Result<()> {
    let project_root = project_root.into().canonicalize()?;
    let store = SessionStore::new(&project_root);
    let session = match session_id {
        Some(id) => store.open_session(id),
        None => store.create_session()?,
    };
    let initial_events = if session_id.is_some() {
        store.load_events(&session)?
    } else {
        Vec::new()
    };
    let session_summary = if session_id.is_some() {
        Some(store.summarize_session(&session)?)
    } else {
        None
    };
    let initially_persisted_event_count = initial_events.len();

    let mut terminal = setup_terminal()?;
    let mut app = if initial_events.is_empty() {
        AppState::new()
    } else {
        AppState::from_events(initial_events)
    };
    app.set_workspace_root(project_root.display().to_string());
    let instructions = load_instructions(&project_root);
    let project_command_config = instructions
        .project_config
        .as_ref()
        .map(|config| config.command.clone());
    if !instructions.warnings.is_empty() {
        for warning in &instructions.warnings {
            app.push_error_message(warning);
        }
    }
    let mut config = match PicocodeConfig::load() {
        Ok(config) => Some(config),
        Err(error) => {
            app.push_error_message(error.to_string());
            None
        }
    };
    let mut ai = None;
    if let Some(config_ref) = config.as_ref() {
        let session_profile = session_summary.as_ref().and_then(|summary| {
            summary
                .ai_provider
                .as_ref()
                .zip(summary.ai_model.as_ref())
                .and_then(|(provider, model)| config_ref.find_model(provider, model))
                .map(|profile| AiProfile::new(profile.provider.clone(), profile.model.clone()))
        });
        let effective_profile = session_profile.unwrap_or_else(|| {
            AiProfile::new(
                config_ref.model.provider.clone(),
                config_ref.model.model.clone(),
            )
        });
        app.set_ai_profile(Some(effective_profile.clone()));
        let effective_config =
            config_ref.with_model_selection(&effective_profile.provider, &effective_profile.model);

        match AiClient::from_config(&effective_config) {
            Ok(client) => {
                app.set_ai_profile(Some(AiProfile::new(
                    effective_config.model.provider.clone(),
                    client.model_id(),
                )));
                app.set_runtime_status(crate::app::RuntimeStatus::idle());
                ai = Some(Arc::new(client));
            }
            Err(error) => {
                app.push_error_message(error.to_string());
                app.set_ai_profile(Some(AiProfile::new(
                    effective_config.model.provider.clone(),
                    effective_config.model.model.clone(),
                )));
                app.set_runtime_status(crate::app::RuntimeStatus::idle());
                if matches!(
                    &error,
                    crate::ai::AiError::Config(crate::config::ConfigError::MissingEnv(_))
                ) {
                    app.enter_config_editor(
                        effective_config.model_options(),
                        &effective_config.model.provider,
                        &effective_config.model.model,
                        Some(ConfigField::Auth),
                    );
                }
            }
        }
    }
    if let Some(profile) = &app.ai_profile {
        let _ = store.append_session_meta(
            &session,
            SessionMeta {
                session_id: session.id.clone(),
                parent_session_id: session_summary
                    .as_ref()
                    .and_then(|summary| summary.parent_session_id.clone()),
                cwd: project_root.display().to_string(),
                app_version: env!("CARGO_PKG_VERSION").to_owned(),
                ai_provider: Some(profile.provider.clone()),
                ai_model: Some(profile.model.clone()),
            },
        );
    }
    let mut persisted_event_count = initially_persisted_event_count;
    persist_new_events(&store, &session, &app, &mut persisted_event_count)?;

    let result = run_app(
        &mut terminal,
        &mut app,
        &mut ai,
        &mut config,
        &store,
        &session,
        &project_root,
        instructions.workspace_respect_gitignore(),
        project_command_config,
        &mut persisted_event_count,
    );
    let resume_target = app.pending_resume_session.clone();
    let new_session_requested = app.pending_new_session;
    restore_terminal(&mut terminal)?;
    if new_session_requested {
        return run(None, &project_root);
    }
    if let Some(session_id) = resume_target {
        return run(Some(session_id.as_str()), &project_root);
    }
    result
}

fn instruction_sources_already_loaded(events: &[crate::event::Event]) -> bool {
    events.iter().any(|event| {
        matches!(event.msg, EventMsg::SystemMessage(_))
            && (event.msg.content().starts_with("Loaded instruction source")
                || event
                    .msg
                    .content()
                    .starts_with("Instruction sources loaded:"))
    })
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn run_session_picker(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &SessionStore,
    state: &mut SessionPickerState,
) -> io::Result<Option<String>> {
    loop {
        terminal.draw(|frame| ui::render_session_picker(frame, state))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    match state.handle_key(key) {
                        SessionPickerAction::Continue => {}
                        SessionPickerAction::Cancelled => return Ok(None),
                        SessionPickerAction::Selected(session_id) => return Ok(Some(session_id)),
                        SessionPickerAction::RenameRequested { session_id, new_id } => {
                            let renamed =
                                store.rename_session(&store.open_session(&session_id), &new_id)?;
                            let summaries = store.list_session_summaries()?;
                            state.set_summaries(summaries);
                            if let Some(index) = state
                                .filtered_summaries()
                                .iter()
                                .position(|summary| summary.session.id == renamed.id)
                            {
                                state.selected = index;
                            }
                        }
                        SessionPickerAction::DeleteRequested { session_id } => {
                            store.delete_session(&store.open_session(&session_id))?;
                            let summaries = store.list_session_summaries()?;
                            state.set_summaries(summaries);
                            if state.filtered_summaries().is_empty() {
                                return Ok(None);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn run_session_tree(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    store: &SessionStore,
    state: &mut SessionTreeState,
) -> io::Result<Option<String>> {
    loop {
        terminal.draw(|frame| ui::render_session_tree(frame, state))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    match state.handle_key(key) {
                        SessionTreeAction::Continue => {}
                        SessionTreeAction::Cancelled => return Ok(None),
                        SessionTreeAction::Selected(session_id) => return Ok(Some(session_id)),
                        SessionTreeAction::RenameRequested { session_id, new_id } => {
                            let renamed =
                                store.rename_session(&store.open_session(&session_id), &new_id)?;
                            let summaries = store.list_session_summaries()?;
                            state.set_summaries(summaries);
                            if let Some(index) = state
                                .filtered_summaries()
                                .iter()
                                .position(|summary| summary.session.id == renamed.id)
                            {
                                state.selected = index;
                            }
                        }
                        SessionTreeAction::DeleteRequested { session_id } => {
                            store.delete_session(&store.open_session(&session_id))?;
                            let summaries = store.list_session_summaries()?;
                            state.set_summaries(summaries);
                            if state.filtered_summaries().is_empty() {
                                return Ok(None);
                            }
                        }
                        SessionTreeAction::ForkRequested { session_id } => {
                            let forked = store.fork_session(&store.open_session(&session_id))?;
                            let summaries = store.list_session_summaries()?;
                            state.set_summaries(summaries);
                            if state.filtered_summaries().is_empty() {
                                return Ok(Some(forked.id));
                            }
                            return Ok(Some(forked.id));
                        }
                    }
                }
            }
        }
    }
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    ai: &mut Option<Arc<AiClient>>,
    config: &mut Option<PicocodeConfig>,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    respect_gitignore: bool,
    project_command_config: Option<crate::instruction::ProjectCommandConfig>,
    persisted_event_count: &mut usize,
) -> io::Result<()> {
    let (ai_tx, ai_rx) = mpsc::channel();
    let (command_tx, command_rx) = mpsc::channel();

    while !app.should_exit {
        process_ai_completions(app, &ai_rx, store, session, persisted_event_count)?;
        process_command_events(app, &command_rx, store, session, persisted_event_count)?;
        terminal.draw(|frame| ui::render(frame, app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if app.is_session_picker_active() {
                        if let Some(action) = app.handle_picker_key(key) {
                            handle_picker_action(action, app, store)?;
                        }
                    } else if app.is_model_picker_active() {
                        if let Some(action) = app.handle_model_key(key) {
                            handle_model_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    } else if app.is_config_editor_active() {
                        if let Some(action) = app.handle_config_key(key) {
                            handle_config_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    } else if app.is_session_tree_active() {
                        if let Some(action) = app.handle_tree_key(key) {
                            handle_tree_action(action, app, store, session, project_root)?;
                        }
                    } else if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.push_paste_text(clipboard_text().unwrap_or_default());
                    } else {
                        app.handle_key(key);
                    }
                }
                Event::Paste(text) => {
                    if !app.is_session_picker_active()
                        && !app.is_model_picker_active()
                        && !app.is_config_editor_active()
                        && !app.is_session_tree_active()
                    {
                        app.push_paste_text(text);
                    } else if app.is_config_editor_active() {
                        if let Some(action) = app.handle_config_paste(text) {
                            handle_config_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }

        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key)
                    if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
                {
                    if app.is_session_picker_active() {
                        if let Some(action) = app.handle_picker_key(key) {
                            handle_picker_action(action, app, store)?;
                        }
                    } else if app.is_model_picker_active() {
                        if let Some(action) = app.handle_model_key(key) {
                            handle_model_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    } else if app.is_config_editor_active() {
                        if let Some(action) = app.handle_config_key(key) {
                            handle_config_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    } else if app.is_session_tree_active() {
                        if let Some(action) = app.handle_tree_key(key) {
                            handle_tree_action(action, app, store, session, project_root)?;
                        }
                    } else if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.push_paste_text(clipboard_text().unwrap_or_default());
                    } else {
                        app.handle_key(key);
                    }
                }
                Event::Paste(text) => {
                    if !app.is_session_picker_active()
                        && !app.is_model_picker_active()
                        && !app.is_config_editor_active()
                        && !app.is_session_tree_active()
                    {
                        app.push_paste_text(text);
                    } else if app.is_config_editor_active() {
                        if let Some(action) = app.handle_config_paste(text) {
                            handle_config_action(
                                action,
                                app,
                                ai,
                                store,
                                session,
                                project_root,
                                config,
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }

        process_pending_submissions(
            app,
            ai.as_ref(),
            config.as_ref(),
            store,
            session,
            project_root,
            respect_gitignore,
            project_command_config.as_ref(),
            &command_tx,
            &ai_tx,
        );
        persist_new_events(store, session, app, persisted_event_count)?;
    }

    Ok(())
}

fn persist_new_events(
    store: &SessionStore,
    session: &Session,
    app: &AppState,
    persisted_event_count: &mut usize,
) -> io::Result<()> {
    for event in &app.events[*persisted_event_count..] {
        store.append_event(session, event)?;
    }
    *persisted_event_count = app.events.len();
    Ok(())
}

fn process_pending_submissions(
    app: &mut AppState,
    ai: Option<&Arc<AiClient>>,
    config: Option<&PicocodeConfig>,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    respect_gitignore: bool,
    project_command_config: Option<&crate::instruction::ProjectCommandConfig>,
    command_event_tx: &mpsc::Sender<EventMsg>,
    ai_tx: &mpsc::Sender<AiCompletion>,
) {
    for submission in app.take_pending_submissions() {
        process_submission(
            app,
            ai,
            config,
            store,
            session,
            project_root,
            respect_gitignore,
            project_command_config,
            command_event_tx,
            ai_tx,
            &submission,
        );
    }
}

fn process_submission(
    app: &mut AppState,
    ai: Option<&Arc<AiClient>>,
    config: Option<&PicocodeConfig>,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    respect_gitignore: bool,
    project_command_config: Option<&crate::instruction::ProjectCommandConfig>,
    command_event_tx: &mpsc::Sender<EventMsg>,
    ai_tx: &mpsc::Sender<AiCompletion>,
    submission: &Submission,
) {
    match &submission.op {
        Op::UserInput { .. } => {
            let Some(ai) = ai else {
                app.push_error_message(
                    "AI provider is not configured. Set OPENAI_API_KEY to chat.",
                );
                return;
            };

            let ai = Arc::clone(ai);
            let ai_tx = ai_tx.clone();
            let command_event_tx = command_event_tx.clone();
            let events = app.events.clone();
            let submission = submission.clone();
            let project_root = project_root.to_path_buf();
            let project_command_config = project_command_config.cloned();
            app.start_ai_request();
            app.set_runtime_status(crate::app::RuntimeStatus::with_detail(
                "executing",
                "running agent loop",
            ));

            thread::spawn(move || {
                let result = workspace_for_project_with_options(project_root, respect_gitignore)
                    .map_err(|error| error.to_string())
                    .map(|workspace| {
                        let core = AgentCore::new(
                            ai,
                            ToolRuntime::from_events_with_project_config_and_sender(
                                workspace,
                                &events,
                                project_command_config.clone(),
                                command_event_tx,
                            ),
                        );
                        core.run_submission(&events, &submission).events
                    });
                let _ = ai_tx.send(AiCompletion { result });
            });
        }
        Op::LocalCommand { command } => {
            handle_local_command(app, command, store, session, project_root, ai, config);
        }
    }
}

fn handle_local_command(
    app: &mut AppState,
    command: &LocalCommand,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    ai: Option<&Arc<AiClient>>,
    config: Option<&PicocodeConfig>,
) {
    match command {
        LocalCommand::New => {
            app.pending_new_session = true;
            app.should_exit = true;
        }
        LocalCommand::Image { path } => {
            let resolved = resolve_image_path(project_root, path);
            match attach_image(&resolved) {
                Ok(attached) => {
                    app.push_event_msg(EventMsg::image_attachment(
                        attached.source_path,
                        attached.file_name,
                        attached.mime_type,
                        attached.byte_len,
                        attached.data_url,
                    ));
                    if let Some(ai) = ai {
                        if !ai.capabilities().images {
                            app.push_error_message(
                                "Current model does not support image input. Switch to a vision-capable model before sending a prompt.",
                            );
                        }
                    }
                }
                Err(error) => app.push_error_message(error.to_string()),
            }
        }
        LocalCommand::ImageClipboard => match attach_image_from_clipboard() {
            Ok(attached) => {
                app.push_event_msg(EventMsg::image_attachment(
                    attached.source_path,
                    attached.file_name,
                    attached.mime_type,
                    attached.byte_len,
                    attached.data_url,
                ));
                if let Some(ai) = ai {
                    if !ai.capabilities().images {
                        app.push_error_message(
                            "Current model does not support image input. Switch to a vision-capable model before sending a prompt.",
                        );
                    }
                }
            }
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Resume => match store.list_session_summaries() {
            Ok(summaries) if summaries.is_empty() => {
                app.push_error_message("No previous session to resume.");
            }
            Ok(summaries) => app.enter_session_picker(summaries),
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Compact => {
            let compacted = picocode_event::summarize_compaction(&app.events);
            app.push_event_msg(EventMsg::compaction(compacted));
            app.set_runtime_status(crate::app::RuntimeStatus::with_detail(
                "compact", "continue",
            ));
        }
        LocalCommand::Export => match store.export_session_html(session) {
            Ok(path) => {
                app.push_system_message(format!("Exported session HTML to {}", path.display()));
            }
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Share => match store.share_session_html(session) {
            Ok(path) => {
                app.push_system_message(format!(
                    "Shareable session HTML written to {}",
                    path.display()
                ));
            }
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Capabilities => match CapabilityIndex::discover(project_root) {
            Ok(index) => match CapabilityPreferences::load(project_root) {
                Ok(preferences) => {
                    let enabled = index.enabled_entries(&preferences);
                    if enabled.is_empty() {
                        app.push_system_message("No enabled capabilities discovered.");
                        return;
                    }
                    app.push_system_message(format!(
                        "Capabilities discovered: {} enabled of {} total",
                        enabled.len(),
                        index.entries.len()
                    ));
                    for entry in enabled {
                        app.push_system_message(entry.compact_label());
                    }
                }
                Err(error) => app.push_error_message(error.to_string()),
            },
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Capability { query } => match CapabilityIndex::discover(project_root) {
            Ok(index) => match CapabilityPreferences::load(project_root) {
                Ok(preferences) => {
                    let matches = index
                        .entries
                        .into_iter()
                        .filter(|entry| entry.matches_query(query))
                        .collect::<Vec<_>>();
                    match matches.as_slice() {
                        [] => app.push_error_message(format!("No capability matched: {query}")),
                        [entry] => {
                            match entry.detail_text_with_enabled(preferences.is_enabled(entry)) {
                                Ok(detail) => app.push_system_message(detail),
                                Err(error) => app.push_error_message(error.to_string()),
                            }
                        }
                        _ => {
                            app.push_system_message(format!(
                                "Multiple capabilities matched: {}",
                                query
                            ));
                            for entry in matches {
                                app.push_system_message(entry.compact_label());
                            }
                        }
                    }
                }
                Err(error) => app.push_error_message(error.to_string()),
            },
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::CapabilityEnable { query } | LocalCommand::CapabilityDisable { query } => {
            let enable = matches!(command, LocalCommand::CapabilityEnable { .. });
            match CapabilityIndex::discover(project_root) {
                Ok(index) => {
                    let matches = index
                        .entries
                        .into_iter()
                        .filter(|entry| entry.matches_query(query))
                        .collect::<Vec<_>>();
                    match matches.as_slice() {
                        [] => app.push_error_message(format!("No capability matched: {query}")),
                        [entry] => match CapabilityPreferences::load(project_root) {
                            Ok(mut preferences) => match preferences.set_enabled(entry, enable) {
                                Ok(()) => {
                                    let state = if enable { "enabled" } else { "disabled" };
                                    app.push_system_message(format!(
                                        "Capability {state}: {}",
                                        entry.compact_label()
                                    ));
                                }
                                Err(error) => app.push_error_message(error.to_string()),
                            },
                            Err(error) => app.push_error_message(error.to_string()),
                        },
                        _ => {
                            app.push_system_message(format!(
                                "Multiple capabilities matched: {}",
                                query
                            ));
                            for entry in matches {
                                app.push_system_message(entry.compact_label());
                            }
                        }
                    }
                }
                Err(error) => app.push_error_message(error.to_string()),
            }
        }
        LocalCommand::Skill { query } => match CapabilityIndex::discover(project_root) {
            Ok(index) => match CapabilityPreferences::load(project_root) {
                Ok(preferences) => {
                    let matches = index
                        .entries
                        .into_iter()
                        .filter(|entry| entry.kind == CapabilityKind::Skill)
                        .filter(|entry| entry.matches_query(query))
                        .collect::<Vec<_>>();
                    match matches.as_slice() {
                        [] => app.push_error_message(format!("No skill matched: {query}")),
                        [entry] => {
                            if !preferences.is_enabled(entry) {
                                app.push_error_message(format!(
                                    "Skill is disabled: {}",
                                    entry.compact_label()
                                ));
                            } else {
                                match entry.skill_context_text(true) {
                                    Ok(detail) => app.push_system_message(detail),
                                    Err(error) => app.push_error_message(error.to_string()),
                                }
                            }
                        }
                        _ => {
                            app.push_system_message(format!("Multiple skills matched: {}", query));
                            for entry in matches {
                                let status = if preferences.is_enabled(&entry) {
                                    "enabled"
                                } else {
                                    "disabled"
                                };
                                app.push_system_message(format!(
                                    "{} [{}]",
                                    entry.compact_label(),
                                    status
                                ));
                            }
                        }
                    }
                }
                Err(error) => app.push_error_message(error.to_string()),
            },
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Tree => match store.list_session_summaries() {
            Ok(summaries) if summaries.is_empty() => {
                app.push_error_message("No previous session tree to browse.");
            }
            Ok(summaries) => app.enter_session_tree(summaries),
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Model => match config {
            Some(config) => {
                let options = config.model_options();
                if options.is_empty() {
                    app.push_error_message("No configured models found.");
                    return;
                }
                let current_provider = app
                    .ai_profile
                    .as_ref()
                    .map(|profile| profile.provider.clone())
                    .unwrap_or_else(|| config.model.provider.clone());
                let current_model = app
                    .ai_profile
                    .as_ref()
                    .map(|profile| profile.model.clone())
                    .unwrap_or_else(|| config.model.model.clone());
                app.enter_model_picker(options, &current_provider, &current_model);
            }
            None => app.push_error_message("AI config is not available."),
        },
        LocalCommand::Config => match config {
            Some(config) => {
                let options = config.model_options();
                let current_provider = app
                    .ai_profile
                    .as_ref()
                    .map(|profile| profile.provider.clone())
                    .unwrap_or_else(|| config.model.provider.clone());
                let current_model = app
                    .ai_profile
                    .as_ref()
                    .map(|profile| profile.model.clone())
                    .unwrap_or_else(|| config.model.model.clone());
                app.enter_config_editor(options, &current_provider, &current_model, None);
            }
            None => app.push_error_message("AI config is not available."),
        },
        LocalCommand::Continue => match store.list_session_summaries() {
            Ok(summaries) => {
                if let Some(summary) = summaries.first() {
                    app.pending_resume_session = Some(summary.session.id.clone());
                    app.should_exit = true;
                } else {
                    app.push_error_message("No previous session to continue.");
                }
            }
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Fork => match store.fork_session(session) {
            Ok(forked) => {
                app.pending_resume_session = Some(forked.id);
                app.should_exit = true;
            }
            Err(error) => app.push_error_message(error.to_string()),
        },
        LocalCommand::Session { id } => {
            if store.open_session(id).path.exists() {
                app.pending_resume_session = Some(id.clone());
                app.should_exit = true;
            } else {
                app.push_error_message(format!(
                    "Session not found: {id} in {}",
                    project_root.display()
                ));
            }
        }
    }
}

fn resolve_image_path(project_root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

fn handle_tree_action(
    action: SessionTreeAction,
    app: &mut AppState,
    store: &SessionStore,
    _session: &Session,
    _project_root: &Path,
) -> io::Result<()> {
    match action {
        SessionTreeAction::Continue | SessionTreeAction::Cancelled => Ok(()),
        SessionTreeAction::Selected(session_id) => {
            app.pending_resume_session = Some(session_id);
            app.should_exit = true;
            Ok(())
        }
        SessionTreeAction::RenameRequested { session_id, new_id } => {
            let renamed = store.rename_session(&store.open_session(&session_id), &new_id)?;
            let summaries = store.list_session_summaries()?;
            if let AppMode::SessionTree(state) = &mut app.mode {
                state.set_summaries(summaries);
                if let Some(index) = state
                    .filtered_summaries()
                    .iter()
                    .position(|summary| summary.session.id == renamed.id)
                {
                    state.selected = index;
                }
            }
            Ok(())
        }
        SessionTreeAction::DeleteRequested { session_id } => {
            store.delete_session(&store.open_session(&session_id))?;
            let summaries = store.list_session_summaries()?;
            if summaries.is_empty() {
                app.mode = AppMode::Chat;
                app.push_error_message("No sessions left.");
                return Ok(());
            }
            if let AppMode::SessionTree(state) = &mut app.mode {
                state.set_summaries(summaries);
            }
            Ok(())
        }
        SessionTreeAction::ForkRequested { session_id } => {
            let forked = store.fork_session(&store.open_session(&session_id))?;
            let summaries = store.list_session_summaries()?;
            if let AppMode::SessionTree(state) = &mut app.mode {
                state.set_summaries(summaries);
            }
            app.pending_resume_session = Some(forked.id);
            app.should_exit = true;
            Ok(())
        }
    }
}

fn handle_model_action(
    action: ModelPickerAction,
    app: &mut AppState,
    ai: &mut Option<Arc<AiClient>>,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    config: &mut Option<PicocodeConfig>,
) -> io::Result<()> {
    match action {
        ModelPickerAction::Continue | ModelPickerAction::Cancelled => Ok(()),
        ModelPickerAction::Selected(selection) => {
            let Some(current_config) = config.as_ref() else {
                app.push_error_message("AI config is not available.");
                return Ok(());
            };
            let updated_config =
                current_config.with_model_selection(&selection.provider, &selection.model);
            let client = match AiClient::from_config(&updated_config) {
                Ok(client) => client,
                Err(error) => {
                    app.push_error_message(error.to_string());
                    app.runtime_status =
                        crate::app::RuntimeStatus::with_detail("model", "selector");
                    return Ok(());
                }
            };

            if let Err(error) = updated_config.save() {
                app.push_error_message(error.to_string());
                app.runtime_status = crate::app::RuntimeStatus::with_detail("model", "save");
                return Ok(());
            }
            *config = Some(updated_config.clone());

            let provider = selection.provider.clone();
            let model = selection.model.clone();
            *ai = Some(Arc::new(client));
            app.set_ai_profile(Some(AiProfile::new(provider.clone(), model.clone())));
            let _ = store.append_session_meta(
                session,
                SessionMeta {
                    session_id: session.id.clone(),
                    parent_session_id: store
                        .summarize_session(session)
                        .ok()
                        .and_then(|summary| summary.parent_session_id),
                    cwd: project_root.display().to_string(),
                    app_version: env!("CARGO_PKG_VERSION").to_owned(),
                    ai_provider: Some(provider.clone()),
                    ai_model: Some(model.clone()),
                },
            );
            app.push_system_message(format!(
                "AI provider {} enabled with model {}",
                provider, model
            ));
            app.runtime_status = crate::app::RuntimeStatus::idle();
            app.mode = AppMode::Chat;
            Ok(())
        }
    }
}

fn handle_config_action(
    action: ConfigEditorAction,
    app: &mut AppState,
    ai: &mut Option<Arc<AiClient>>,
    store: &SessionStore,
    session: &Session,
    project_root: &Path,
    config: &mut Option<PicocodeConfig>,
) -> io::Result<()> {
    match action {
        ConfigEditorAction::Continue | ConfigEditorAction::Cancelled => Ok(()),
        ConfigEditorAction::Saved(saved_config) => {
            if let Err(error) = saved_config.save() {
                app.push_error_message(error.to_string());
                app.runtime_status = crate::app::RuntimeStatus::with_detail("config", "save");
                return Ok(());
            }
            *config = Some(saved_config.clone());

            let client = match AiClient::from_config(&saved_config) {
                Ok(client) => client,
                Err(error) => {
                    app.push_error_message(error.to_string());
                    app.enter_config_editor(
                        saved_config.model_options(),
                        &saved_config.model.provider,
                        &saved_config.model.model,
                        Some(ConfigField::Auth),
                    );
                    return Ok(());
                }
            };

            let provider = saved_config.model.provider.clone();
            let model = client.model_id().to_string();
            *ai = Some(Arc::new(client));
            app.set_ai_profile(Some(AiProfile::new(provider.clone(), model.clone())));
            let _ = store.append_session_meta(
                session,
                SessionMeta {
                    session_id: session.id.clone(),
                    parent_session_id: store
                        .summarize_session(session)
                        .ok()
                        .and_then(|summary| summary.parent_session_id),
                    cwd: project_root.display().to_string(),
                    app_version: env!("CARGO_PKG_VERSION").to_owned(),
                    ai_provider: Some(provider.clone()),
                    ai_model: Some(model.clone()),
                },
            );
            app.push_system_message(format!(
                "AI provider {} enabled with model {}",
                provider, model
            ));
            app.runtime_status = crate::app::RuntimeStatus::idle();
            app.mode = AppMode::Chat;
            Ok(())
        }
    }
}

fn handle_picker_action(
    action: SessionPickerAction,
    app: &mut AppState,
    store: &SessionStore,
) -> io::Result<()> {
    match action {
        SessionPickerAction::Continue | SessionPickerAction::Cancelled => Ok(()),
        SessionPickerAction::Selected(session_id) => {
            app.pending_resume_session = Some(session_id);
            app.should_exit = true;
            Ok(())
        }
        SessionPickerAction::RenameRequested { session_id, new_id } => {
            let renamed = store.rename_session(&store.open_session(&session_id), &new_id)?;
            let summaries = store.list_session_summaries()?;
            if let AppMode::SessionPicker(state) = &mut app.mode {
                state.set_summaries(summaries);
                if let Some(index) = state
                    .filtered_summaries()
                    .iter()
                    .position(|summary| summary.session.id == renamed.id)
                {
                    state.selected = index;
                }
            }
            Ok(())
        }
        SessionPickerAction::DeleteRequested { session_id } => {
            store.delete_session(&store.open_session(&session_id))?;
            let summaries = store.list_session_summaries()?;
            if summaries.is_empty() {
                app.mode = AppMode::Chat;
                app.push_error_message("No sessions left.");
                return Ok(());
            }
            if let AppMode::SessionPicker(state) = &mut app.mode {
                state.set_summaries(summaries);
            }
            Ok(())
        }
    }
}

fn workspace_for_project_with_options(
    project_root: PathBuf,
    respect_gitignore: bool,
) -> io::Result<crate::workspace::Workspace> {
    crate::workspace::Workspace::new_with_options(
        project_root,
        crate::workspace::WorkspaceOptions { respect_gitignore },
    )
}

struct AiCompletion {
    result: Result<Vec<crate::event::EventMsg>, String>,
}

fn process_ai_completions(
    app: &mut AppState,
    ai_rx: &mpsc::Receiver<AiCompletion>,
    store: &SessionStore,
    session: &Session,
    persisted_event_count: &mut usize,
) -> io::Result<()> {
    while let Ok(completion) = ai_rx.try_recv() {
        app.finish_ai_request();
        match completion.result {
            Ok(events) => {
                for event in events {
                    app.push_event_msg(event);
                }
            }
            Err(error) => app.push_error_message(error),
        }
        if app.pending_ai_requests == 0 {
            app.set_runtime_status(crate::app::RuntimeStatus::idle());
        } else {
            app.set_runtime_status(crate::app::RuntimeStatus::thinking());
        }
        persist_new_events(store, session, app, persisted_event_count)?;
    }
    Ok(())
}

fn process_command_events(
    app: &mut AppState,
    command_rx: &mpsc::Receiver<EventMsg>,
    store: &SessionStore,
    session: &Session,
    persisted_event_count: &mut usize,
) -> io::Result<()> {
    while let Ok(event) = command_rx.try_recv() {
        app.push_event_msg(event);
        persist_new_events(store, session, app, persisted_event_count)?;
    }
    Ok(())
}
