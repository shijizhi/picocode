mod agent_core;
mod ai;
mod app;
mod capability;
mod config;
mod config_editor;
mod event;
mod image;
mod instruction;
mod model_picker;
mod session;
mod session_picker;
mod session_tree;
mod submission;
mod terminal;
mod tool;
mod ui;
mod workspace;

use std::{io, path::PathBuf};

use capability::{CapabilityIndex, CapabilityKind, CapabilityPreferences};
use event::{Event, EventMsg};
use instruction::load_instructions;
use session::{SessionItem, SessionStore};
use tool::{PermissionKind, ToolCall, ToolResult, ToolRuntime};
use workspace::{Workspace, WorkspaceOptions};

pub fn run_cli(args: impl IntoIterator<Item = String>) -> io::Result<()> {
    let args = args.into_iter().collect::<Vec<_>>();
    let cli = CliArgs::parse(args);
    let store = SessionStore::new(&cli.project_root);

    match cli.args.as_slice() {
        [] => run_default_session(&cli.project_root),
        [flag] if flag == "--continue" || flag == "-c" => run_default_session(&cli.project_root),
        [flag] if flag == "--new" => terminal::run(None, &cli.project_root),
        [flag] if flag == "--list-sessions" => {
            for summary in store.list_session_summaries()? {
                println!(
                    "{} events={} {} cwd={} last={}",
                    summary.session.id,
                    summary.event_count,
                    summary.stats.compact_label(),
                    summary.cwd.as_deref().unwrap_or("-"),
                    summary.last_timestamp.as_deref().unwrap_or("-")
                );
            }
            Ok(())
        }
        [flag] if flag == "--list-capabilities" => run_list_capabilities(&cli.project_root),
        [flag, query] if flag == "--capability" => run_capability_detail(&cli.project_root, query),
        [flag, query] if flag == "--cap-enable" => {
            run_capability_toggle(&cli.project_root, query, true)
        }
        [flag, query] if flag == "--cap-disable" => {
            run_capability_toggle(&cli.project_root, query, false)
        }
        [flag, query] if flag == "--skill" => run_skill_load(&cli.project_root, query),
        [flag, id] if flag == "--replay" => {
            let session = store.open_session(id);
            for line in store.load_lines(&session)? {
                match line.item {
                    SessionItem::SessionMeta(meta) => {
                        println!(
                            "{} session_meta: session={} cwd={} version={}",
                            line.timestamp, meta.session_id, meta.cwd, meta.app_version
                        );
                    }
                    SessionItem::EventMsg(event) => {
                        println!(
                            "{} {}: {}",
                            line.timestamp,
                            event.msg.label(),
                            event.msg.content()
                        );
                    }
                }
            }
            Ok(())
        }
        [flag] if flag == "--resume" => match terminal::pick_session(&cli.project_root)? {
            Some(id) => terminal::run(Some(&id), &cli.project_root),
            None => Ok(()),
        },
        [flag, id] if flag == "--resume" => terminal::run(Some(id), &cli.project_root),
        [flag, id] if flag == "--session" => terminal::run(Some(id), &cli.project_root),
        [flag] if flag == "--tree" => match terminal::pick_session_tree(&cli.project_root)? {
            Some(id) => terminal::run(Some(&id), &cli.project_root),
            None => Ok(()),
        },
        [flag, id] if flag == "--fork" => {
            let forked = terminal::fork_session_from(&cli.project_root, id)?;
            terminal::run(Some(&forked), &cli.project_root)
        }
        [flag, id] if flag == "--export" => run_session_export(&cli.project_root, id, false),
        [flag, id] if flag == "--share" => run_session_export(&cli.project_root, id, true),
        [flag, name] if flag == "--tool" && name == "list" => run_tool_list(&cli.project_root),
        [flag, name] if flag == "--tool" && name == "ls" => run_tool_ls(&cli.project_root, "."),
        [flag, name, path] if flag == "--tool" && name == "ls" => {
            run_tool_ls(&cli.project_root, path)
        }
        [flag, name, path] if flag == "--tool" && name == "read" => {
            run_tool_read(&cli.project_root, path, 0, 200)
        }
        [flag, name, path, offset, limit] if flag == "--tool" && name == "read" => {
            let offset = offset.parse::<usize>().unwrap_or(0);
            let limit = limit.parse::<usize>().unwrap_or(200);
            run_tool_read(&cli.project_root, path, offset, limit)
        }
        [flag, name, query] if flag == "--tool" && name == "find_files" => {
            run_tool_find_files(&cli.project_root, query, ".", 100)
        }
        [flag, name, query] if flag == "--tool" && name == "find" => {
            run_tool_find_files(&cli.project_root, query, ".", 100)
        }
        [flag, name, query, path] if flag == "--tool" && name == "find_files" => {
            run_tool_find_files(&cli.project_root, query, path, 100)
        }
        [flag, name, query, path] if flag == "--tool" && name == "find" => {
            run_tool_find_files(&cli.project_root, query, path, 100)
        }
        [flag, name, query, path, limit] if flag == "--tool" && name == "find_files" => {
            let limit = limit.parse::<usize>().unwrap_or(100);
            run_tool_find_files(&cli.project_root, query, path, limit)
        }
        [flag, name, query, path, limit] if flag == "--tool" && name == "find" => {
            let limit = limit.parse::<usize>().unwrap_or(100);
            run_tool_find_files(&cli.project_root, query, path, limit)
        }
        [flag, name, query] if flag == "--tool" && name == "search_text" => {
            run_tool_search_text(&cli.project_root, query, ".", 100, false)
        }
        [flag, name, query] if flag == "--tool" && name == "grep" => {
            run_tool_search_text(&cli.project_root, query, ".", 100, false)
        }
        [flag, name, query, path] if flag == "--tool" && name == "search_text" => {
            run_tool_search_text(&cli.project_root, query, path, 100, false)
        }
        [flag, name, query, path] if flag == "--tool" && name == "grep" => {
            run_tool_search_text(&cli.project_root, query, path, 100, false)
        }
        [flag, name, query, path, limit] if flag == "--tool" && name == "search_text" => {
            let limit = limit.parse::<usize>().unwrap_or(100);
            run_tool_search_text(&cli.project_root, query, path, limit, false)
        }
        [flag, name, query, path, limit] if flag == "--tool" && name == "grep" => {
            let limit = limit.parse::<usize>().unwrap_or(100);
            run_tool_search_text(&cli.project_root, query, path, limit, false)
        }
        [flag, name, query, path, limit, ignore_case]
            if flag == "--tool" && name == "search_text" =>
        {
            let limit = limit.parse::<usize>().unwrap_or(100);
            let ignore_case = ignore_case == "true";
            run_tool_search_text(&cli.project_root, query, path, limit, ignore_case)
        }
        [flag, name, query, path, limit, ignore_case] if flag == "--tool" && name == "grep" => {
            let limit = limit.parse::<usize>().unwrap_or(100);
            let ignore_case = ignore_case == "true";
            run_tool_search_text(&cli.project_root, query, path, limit, ignore_case)
        }
        [flag, name, path, find, replace] if flag == "--tool" && name == "propose_edit" => {
            run_tool_propose_edit(&cli.project_root, path, find, replace)
        }
        [flag, name, path, find, replace] if flag == "--tool" && name == "apply_patch" => {
            run_tool_apply_patch(&cli.project_root, path, find, replace)
        }
        [flag, name, edits_json] if flag == "--tool" && name == "propose_edit_batch" => {
            run_tool_edit_batch(&cli.project_root, "propose_edit_batch", edits_json)
        }
        [flag, name, edits_json] if flag == "--tool" && name == "apply_patch_batch" => {
            run_tool_edit_batch(&cli.project_root, "apply_patch_batch", edits_json)
        }
        [flag, name, path] if flag == "--tool" && name == "rewind_edit" => {
            run_tool_rewind_edit(&cli.project_root, path)
        }
        [flag, name, path] if flag == "--tool" && name == "rollback_edit" => {
            run_tool_rewind_edit(&cli.project_root, path)
        }
        [flag, name, command] if flag == "--tool" && name == "run_command" => {
            run_tool_run_command(&cli.project_root, command)
        }
        _ => {
            eprintln!("usage:");
            eprintln!("  picocode [--project <path>]");
            eprintln!("  picocode [--project <path>] --continue | -c");
            eprintln!("  picocode [--project <path>] --new");
            eprintln!("  picocode [--project <path>] --list-sessions");
            eprintln!("  picocode [--project <path>] --list-capabilities");
            eprintln!("  picocode [--project <path>] --capability <query>");
            eprintln!("  picocode [--project <path>] --cap-enable <query>");
            eprintln!("  picocode [--project <path>] --cap-disable <query>");
            eprintln!("  picocode [--project <path>] --skill <query>");
            eprintln!("  picocode [--project <path>] --replay <session-id>");
            eprintln!("  picocode [--project <path>] --resume");
            eprintln!("  picocode [--project <path>] --session <session-id>");
            eprintln!("  picocode [--project <path>] --tree");
            eprintln!("  picocode [--project <path>] --fork <session-id>");
            eprintln!("  picocode [--project <path>] --export <session-id>");
            eprintln!("  picocode [--project <path>] --share <session-id>");
            eprintln!("  picocode [--project <path>] --tool list");
            eprintln!("  picocode [--project <path>] --tool ls [path]");
            eprintln!("  picocode [--project <path>] --tool read <path> [offset limit]");
            eprintln!("  picocode [--project <path>] --tool find_files <query> [path limit]");
            eprintln!(
                "  picocode [--project <path>] --tool search_text <query> [path limit ignore_case]"
            );
            eprintln!("  picocode [--project <path>] --tool propose_edit <path> <find> <replace>");
            eprintln!("  picocode [--project <path>] --tool apply_patch <path> <find> <replace>");
            eprintln!("  picocode [--project <path>] --tool propose_edit_batch <edits_json>");
            eprintln!("  picocode [--project <path>] --tool apply_patch_batch <edits_json>");
            eprintln!("  picocode [--project <path>] --tool rewind_edit <path>");
            eprintln!("  picocode [--project <path>] --tool rollback_edit <path>  # alias");
            eprintln!("  picocode [--project <path>] --tool run_command <command>");
            Ok(())
        }
    }
}

fn run_list_capabilities(project_root: impl AsRef<std::path::Path>) -> io::Result<()> {
    let index = CapabilityIndex::discover(project_root.as_ref())?;
    let preferences = CapabilityPreferences::load(project_root.as_ref())?;
    let enabled = index.enabled_entries(&preferences);
    if enabled.is_empty() {
        println!("no enabled capabilities discovered");
        return Ok(());
    }

    for entry in enabled {
        println!("{}", entry.compact_label());
    }
    Ok(())
}

fn run_capability_detail(project_root: impl AsRef<std::path::Path>, query: &str) -> io::Result<()> {
    let index = CapabilityIndex::discover(project_root.as_ref())?;
    let preferences = CapabilityPreferences::load(project_root.as_ref())?;
    let matches = index
        .entries
        .into_iter()
        .filter(|entry| entry.matches_query(query))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => {
            println!("no capability matched: {query}");
        }
        [entry] => {
            println!(
                "{}",
                entry.detail_text_with_enabled(preferences.is_enabled(entry))?
            );
        }
        _ => {
            println!("multiple capabilities matched: {query}");
            for entry in matches {
                let status = if preferences.is_enabled(&entry) {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("{} [{}]", entry.compact_label(), status);
            }
        }
    }
    Ok(())
}

fn run_capability_toggle(
    project_root: impl AsRef<std::path::Path>,
    query: &str,
    enabled: bool,
) -> io::Result<()> {
    let index = CapabilityIndex::discover(project_root.as_ref())?;
    let matches = index
        .entries
        .into_iter()
        .filter(|entry| entry.matches_query(query))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => {
            println!("no capability matched: {query}");
        }
        [entry] => {
            let mut preferences = CapabilityPreferences::load(project_root.as_ref())?;
            preferences.set_enabled(entry, enabled)?;
            let state = if enabled { "enabled" } else { "disabled" };
            println!("capability {state}: {}", entry.compact_label());
        }
        _ => {
            println!("multiple capabilities matched: {query}");
            for entry in matches {
                println!("{}", entry.compact_label());
            }
        }
    }
    Ok(())
}

fn run_skill_load(project_root: impl AsRef<std::path::Path>, query: &str) -> io::Result<()> {
    let index = CapabilityIndex::discover(project_root.as_ref())?;
    let preferences = CapabilityPreferences::load(project_root.as_ref())?;
    let matches = index
        .entries
        .into_iter()
        .filter(|entry| entry.kind == CapabilityKind::Skill)
        .filter(|entry| entry.matches_query(query))
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [] => {
            println!("no skill matched: {query}");
        }
        [entry] => {
            if !preferences.is_enabled(entry) {
                println!("skill is disabled: {}", entry.compact_label());
            } else {
                println!("{}", entry.skill_context_text(true)?);
            }
        }
        _ => {
            println!("multiple skills matched: {query}");
            for entry in matches {
                let status = if preferences.is_enabled(&entry) {
                    "enabled"
                } else {
                    "disabled"
                };
                println!("{} [{}]", entry.compact_label(), status);
            }
        }
    }
    Ok(())
}

fn run_default_session(project_root: impl Into<PathBuf>) -> io::Result<()> {
    let project_root = project_root.into();
    let store = SessionStore::new(&project_root);
    match store.list_session_summaries()?.first() {
        Some(summary) => terminal::run(Some(&summary.session.id), &project_root),
        None => terminal::run(None, &project_root),
    }
}

fn run_session_export(
    project_root: impl Into<PathBuf>,
    session_id: &str,
    share: bool,
) -> io::Result<()> {
    let project_root = project_root.into();
    let store = SessionStore::new(&project_root);
    let session = store.open_session(session_id);
    let path = if share {
        store.share_session_html(&session)?
    } else {
        store.export_session_html(&session)?
    };
    println!("{}", path.display());
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliArgs {
    project_root: PathBuf,
    args: Vec<String>,
}

impl CliArgs {
    fn parse(args: Vec<String>) -> Self {
        let mut project_root = PathBuf::from(".");
        let mut remaining = Vec::new();
        let mut index = 0;

        while index < args.len() {
            if args[index] == "--project" {
                if let Some(path) = args.get(index + 1) {
                    project_root = PathBuf::from(path);
                    index += 2;
                    continue;
                }
            }

            remaining.push(args[index].clone());
            index += 1;
        }

        Self {
            project_root,
            args: remaining,
        }
    }
}

fn run_tool_list(project_root: impl Into<PathBuf>) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    for definition in runtime.definitions() {
        println!(
            "{} [{}]\n{}\n{}",
            definition.name,
            permission_label(definition.permission),
            definition.description,
            definition.input_schema.to_json()
        );
    }
    Ok(())
}

fn run_tool_ls(project_root: impl Into<PathBuf>, path: &str) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new("cli-tool-0", "ls", format!("path={path}"));
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn permission_label(permission: PermissionKind) -> &'static str {
    match permission {
        PermissionKind::Read => "read",
        PermissionKind::Write => "write",
        PermissionKind::Execute => "execute",
        PermissionKind::Network => "network",
    }
}

fn run_tool_read(
    project_root: impl Into<PathBuf>,
    path: &str,
    offset: usize,
    limit: usize,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new(
        "cli-tool-0",
        "read",
        format!("path={path}\noffset={offset}\nlimit={limit}"),
    );
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_find_files(
    project_root: impl Into<PathBuf>,
    query: &str,
    path: &str,
    limit: usize,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new(
        "cli-tool-0",
        "find_files",
        format!("query={query}\npath={path}\nlimit={limit}"),
    );
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_search_text(
    project_root: impl Into<PathBuf>,
    query: &str,
    path: &str,
    limit: usize,
    ignore_case: bool,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new(
        "cli-tool-0",
        "search_text",
        format!("query={query}\npath={path}\nlimit={limit}\nignore_case={ignore_case}"),
    );
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_propose_edit(
    project_root: impl Into<PathBuf>,
    path: &str,
    find: &str,
    replace: &str,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new(
        "cli-tool-0",
        "propose_edit",
        format!("path={path}\nfind={find}\nreplace={replace}"),
    );
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_apply_patch(
    project_root: impl Into<PathBuf>,
    path: &str,
    find: &str,
    replace: &str,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new(
        "cli-tool-0",
        "apply_patch",
        format!("path={path}\nfind={find}\nreplace={replace}"),
    );
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_edit_batch(
    project_root: impl Into<PathBuf>,
    tool_name: &str,
    edits_json: &str,
) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new("cli-tool-0", tool_name, format!("edits_json={edits_json}"));
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_rewind_edit(project_root: impl Into<PathBuf>, path: &str) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new("cli-tool-0", "rewind_edit", format!("path={path}"));
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn run_tool_run_command(project_root: impl Into<PathBuf>, command: &str) -> io::Result<()> {
    let runtime = tool_runtime_for_project(project_root)?;
    let call = ToolCall::new("cli-tool-0", "run_command", format!("command={command}"));
    let result = execute_and_print_tool_events(&runtime, call);
    println!("{}", result.content);
    Ok(())
}

fn tool_runtime_for_project(project_root: impl Into<PathBuf>) -> io::Result<ToolRuntime> {
    let project_root = project_root.into();
    let instructions = load_instructions(&project_root);
    let workspace = Workspace::new_with_options(
        project_root,
        WorkspaceOptions {
            respect_gitignore: instructions.workspace_respect_gitignore(),
        },
    )?;
    Ok(ToolRuntime::with_project_config(
        workspace,
        instructions.project_config.map(|config| config.command),
    ))
}

#[allow(dead_code)]
pub(crate) fn workspace_for_project(project_root: impl Into<PathBuf>) -> io::Result<Workspace> {
    let project_root = project_root.into();
    let instructions = load_instructions(&project_root);
    Workspace::new_with_options(
        project_root,
        WorkspaceOptions {
            respect_gitignore: instructions.workspace_respect_gitignore(),
        },
    )
}

fn execute_and_print_tool_events(runtime: &ToolRuntime, call: ToolCall) -> ToolResult {
    let call_event = Event::new("evt-tool-call", EventMsg::tool_call(call.clone()));
    eprintln!("{}: {}", call_event.msg.label(), call_event.msg.content());

    let result = runtime.execute(call);
    let _result_event = Event::new("evt-tool-result", EventMsg::tool_result(result.clone()));
    eprintln!("tool result: status={}", result.status.as_str());

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_args_default_to_current_directory() {
        let args = CliArgs::parse(vec!["--tool".to_owned(), "list".to_owned()]);

        assert_eq!(args.project_root, PathBuf::from("."));
        assert_eq!(args.args, vec!["--tool", "list"]);
    }

    #[test]
    fn cli_args_extract_project_directory() {
        let args = CliArgs::parse(vec![
            "--project".to_owned(),
            "/tmp/demo".to_owned(),
            "--tool".to_owned(),
            "ls".to_owned(),
        ]);

        assert_eq!(args.project_root, PathBuf::from("/tmp/demo"));
        assert_eq!(args.args, vec!["--tool", "ls"]);
    }

    #[test]
    fn cli_args_keep_continue_and_session_flags() {
        let args = CliArgs::parse(vec![
            "--continue".to_owned(),
            "--session".to_owned(),
            "session-1".to_owned(),
        ]);

        assert_eq!(args.args, vec!["--continue", "--session", "session-1"]);
    }

    #[test]
    fn cli_args_keep_new_flag() {
        let args = CliArgs::parse(vec!["--new".to_owned()]);

        assert_eq!(args.args, vec!["--new"]);
    }
}
