use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

const INSTRUCTION_CHAR_LIMIT: usize = 4096;

const GLOBAL_CONTEXT_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];
const PROJECT_CONTEXT_FILES: &[&str] = &["PLAN.md"];
const PROJECT_CONFIG_FILE: &str = "picocode.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionDoc {
    pub path: PathBuf,
    pub content: String,
    pub truncated: bool,
    pub kind: InstructionSourceKind,
}

impl InstructionDoc {
    pub fn display_path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    pub fn to_system_message(&self) -> String {
        let mut message = format!(
            "Loaded instruction source [{}]: {}\n",
            self.kind.label(),
            self.display_path()
        );
        message.push_str(&self.content);
        if self.truncated {
            message.push_str("\n\n[truncated]");
        }
        message
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionLoadReport {
    pub docs: Vec<InstructionDoc>,
    pub warnings: Vec<String>,
    pub project_config: Option<ProjectConfig>,
}

impl InstructionLoadReport {
    pub fn workspace_respect_gitignore(&self) -> bool {
        self.project_config
            .as_ref()
            .map(|config| config.workspace.respect_gitignore)
            .unwrap_or(true)
    }

    pub fn summary_message(&self) -> String {
        let mut global = 0;
        let mut project = 0;
        let mut config = 0;
        let mut plan = 0;

        for doc in &self.docs {
            match doc.kind {
                InstructionSourceKind::Global => global += 1,
                InstructionSourceKind::ProjectInstruction => project += 1,
                InstructionSourceKind::ProjectConfig => config += 1,
                InstructionSourceKind::ProjectPlan => plan += 1,
            }
        }

        format!(
            "Instruction sources loaded: total={} (global={}, project={}, config={}, plan={})",
            self.docs.len(),
            global,
            project,
            config,
            plan
        )
    }
}

pub fn load_instructions(project_root: impl AsRef<Path>) -> InstructionLoadReport {
    load_instructions_with_global_home(project_root.as_ref(), global_picocode_home())
}

fn load_instructions_with_global_home(
    project_root: &Path,
    global_home: Option<PathBuf>,
) -> InstructionLoadReport {
    let root = project_root.as_ref();
    let mut docs = Vec::new();
    let mut warnings = Vec::new();
    let project_config = load_project_config(root, &mut warnings);

    docs.extend(load_global_context_files(global_home.as_deref()));
    docs.extend(load_context_chain(root, GLOBAL_CONTEXT_FILES));
    if let Some(config) = &project_config {
        docs.push(InstructionDoc {
            path: root.join(PROJECT_CONFIG_FILE),
            content: config.summary(),
            truncated: false,
            kind: InstructionSourceKind::ProjectConfig,
        });
    }
    docs.extend(load_project_context_files(root));

    InstructionLoadReport {
        docs,
        warnings,
        project_config,
    }
}

fn load_global_context_files(global_home: Option<&Path>) -> Vec<InstructionDoc> {
    global_home
        .map(|home| load_context_files(home, GLOBAL_CONTEXT_FILES, InstructionSourceKind::Global))
        .unwrap_or_default()
}

fn load_project_context_files(root: &Path) -> Vec<InstructionDoc> {
    load_context_files(
        root,
        PROJECT_CONTEXT_FILES,
        InstructionSourceKind::ProjectPlan,
    )
}

fn load_context_chain(root: &Path, filenames: &[&str]) -> Vec<InstructionDoc> {
    let mut docs = Vec::new();
    for directory in ancestry_chain(root) {
        docs.extend(load_context_files(
            &directory,
            filenames,
            InstructionSourceKind::ProjectInstruction,
        ));
    }
    docs
}

fn load_context_files(
    root: &Path,
    filenames: &[&str],
    kind: InstructionSourceKind,
) -> Vec<InstructionDoc> {
    let mut docs = Vec::new();
    for filename in filenames {
        let path = root.join(filename);
        if let Some(doc) = read_instruction_doc(&path, kind) {
            docs.push(doc);
        }
    }
    docs
}

fn ancestry_chain(root: &Path) -> Vec<PathBuf> {
    let mut chain = root.ancestors().map(Path::to_path_buf).collect::<Vec<_>>();
    chain.reverse();
    chain
}

fn read_instruction_doc(path: &Path, kind: InstructionSourceKind) -> Option<InstructionDoc> {
    if !path.exists() {
        return None;
    }

    let content = fs::read_to_string(path).ok()?;
    let truncated = content.chars().count() > INSTRUCTION_CHAR_LIMIT;
    let content = truncate_content(&content);

    Some(InstructionDoc {
        path: path.to_path_buf(),
        content,
        truncated,
        kind,
    })
}

fn load_project_config(root: &Path, warnings: &mut Vec<String>) -> Option<ProjectConfig> {
    let path = root.join(PROJECT_CONFIG_FILE);
    if !path.exists() {
        return None;
    }

    match fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<ProjectTomlConfig>(&content) {
            Ok(config) => Some(ProjectConfig::from_toml(config, warnings, &path)),
            Err(error) => {
                warnings.push(format!("failed to parse {}: {}", path.display(), error));
                None
            }
        },
        Err(error) => {
            warnings.push(format!("failed to read {}: {}", path.display(), error));
            None
        }
    }
}

fn truncate_content(content: &str) -> String {
    content.chars().take(INSTRUCTION_CHAR_LIMIT).collect()
}

fn global_picocode_home() -> Option<PathBuf> {
    std::env::var("PICOCODE_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(".picocode")))
        .ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub workspace: ProjectWorkspaceConfig,
    pub command: ProjectCommandConfig,
}

impl ProjectConfig {
    fn from_toml(config: ProjectTomlConfig, warnings: &mut Vec<String>, path: &Path) -> Self {
        let workspace = config.workspace.unwrap_or_default();
        let command = config.command.unwrap_or_default();
        let approval = match command.approval {
            Some(raw) => match CommandApproval::from_str(&raw) {
                Some(approval) => Some(approval),
                None => {
                    warnings.push(format!(
                        "failed to parse {}: unsupported command.approval value '{}'",
                        path.display(),
                        raw
                    ));
                    None
                }
            },
            None => None,
        };
        Self {
            workspace: ProjectWorkspaceConfig {
                respect_gitignore: workspace.respect_gitignore.unwrap_or(true),
            },
            command: ProjectCommandConfig {
                approval,
                timeout: command.timeout,
            },
        }
    }

    fn summary(&self) -> String {
        let mut parts = vec![format!(
            "workspace.respect_gitignore={}",
            self.workspace.respect_gitignore
        )];
        if let Some(approval) = &self.command.approval {
            parts.push(format!("command.approval={approval}"));
        }
        if let Some(timeout) = self.command.timeout {
            parts.push(format!("command.timeout={timeout}s"));
        }
        format!("Project config: {}", parts.join(", "))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectWorkspaceConfig {
    pub respect_gitignore: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectCommandConfig {
    pub approval: Option<CommandApproval>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandApproval {
    OnRisky,
    Always,
    Never,
}

impl CommandApproval {
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "on_risky" => Some(Self::OnRisky),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstructionSourceKind {
    Global,
    ProjectInstruction,
    ProjectConfig,
    ProjectPlan,
}

impl InstructionSourceKind {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::ProjectInstruction => "project",
            Self::ProjectConfig => "config",
            Self::ProjectPlan => "plan",
        }
    }
}

impl std::fmt::Display for CommandApproval {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OnRisky => write!(formatter, "on_risky"),
            Self::Always => write!(formatter, "always"),
            Self::Never => write!(formatter, "never"),
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct ProjectTomlConfig {
    #[serde(default)]
    workspace: Option<ProjectTomlWorkspace>,
    #[serde(default)]
    command: Option<ProjectTomlCommand>,
}

#[derive(Debug, Deserialize, Default)]
struct ProjectTomlWorkspace {
    respect_gitignore: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
struct ProjectTomlCommand {
    approval: Option<String>,
    timeout: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{create_dir_all, remove_dir_all, write},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn load_instructions_reads_global_and_project_chain_in_order() {
        let temp = TempRoot::new();
        let home = temp.path(".picocode");
        create_dir_all(&home).unwrap();
        write(home.join("AGENTS.md"), "global").unwrap();

        let parent = temp.path("workspace");
        let child = parent.join("project");
        create_dir_all(&child).unwrap();
        write(parent.join("AGENTS.md"), "parent-agents").unwrap();
        write(parent.join("CLAUDE.md"), "parent").unwrap();
        write(child.join("AGENTS.md"), "local").unwrap();
        write(
            child.join("picocode.toml"),
            r#"
            [workspace]
            respect_gitignore = false

            [command]
            approval = "on_risky"
            timeout = 120
            "#,
        )
        .unwrap();

        let report = load_instructions_with_global_home(&child, Some(home.clone()));

        assert_eq!(
            report
                .docs
                .iter()
                .map(InstructionDoc::display_path)
                .collect::<Vec<_>>(),
            vec![
                home.join("AGENTS.md").display().to_string(),
                parent.join("AGENTS.md").display().to_string(),
                parent.join("CLAUDE.md").display().to_string(),
                child.join("AGENTS.md").display().to_string(),
                child.join("picocode.toml").display().to_string(),
            ]
        );
        assert_eq!(report.docs[0].content, "global");
        assert_eq!(report.docs[1].content, "parent-agents");
        assert_eq!(report.docs[2].content, "parent");
        assert_eq!(report.docs[3].content, "local");
        assert_eq!(report.docs[0].kind, InstructionSourceKind::Global);
        assert_eq!(
            report.docs[1].kind,
            InstructionSourceKind::ProjectInstruction
        );
        assert_eq!(
            report.docs[2].kind,
            InstructionSourceKind::ProjectInstruction
        );
        assert_eq!(
            report.docs[3].kind,
            InstructionSourceKind::ProjectInstruction
        );
        assert_eq!(report.docs[4].kind, InstructionSourceKind::ProjectConfig);
        assert!(report.docs[4]
            .content
            .contains("workspace.respect_gitignore=false"));
        assert!(report.docs[4].content.contains("command.approval=on_risky"));
        assert!(report.docs[4].content.contains("command.timeout=120s"));
        assert_eq!(
            report
                .project_config
                .as_ref()
                .map(|config| config.workspace.respect_gitignore),
            Some(false)
        );
        assert_eq!(
            report
                .project_config
                .as_ref()
                .and_then(|config| config.command.approval.as_ref())
                .map(|approval| approval.to_string()),
            Some("on_risky".to_owned())
        );
        assert_eq!(
            report
                .project_config
                .as_ref()
                .and_then(|config| config.command.timeout),
            Some(120)
        );
        assert_eq!(
            report.summary_message(),
            "Instruction sources loaded: total=5 (global=1, project=3, config=1, plan=0)"
        );
    }

    #[test]
    fn load_instructions_warns_on_invalid_command_approval() {
        let temp = TempRoot::new();
        write(
            temp.path("picocode.toml"),
            r#"
            [command]
            approval = "sometimes"
            timeout = 30
            "#,
        )
        .unwrap();

        let report = load_instructions_with_global_home(temp.root(), None);

        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("unsupported command.approval value 'sometimes'"));
        assert_eq!(
            report
                .project_config
                .as_ref()
                .and_then(|config| config.command.approval.as_ref())
                .map(|approval| approval.to_string()),
            None
        );
        assert_eq!(
            report
                .project_config
                .as_ref()
                .and_then(|config| config.command.timeout),
            Some(30)
        );
    }

    #[test]
    fn load_instructions_truncates_long_content() {
        let temp = TempRoot::new();
        let long = "x".repeat(INSTRUCTION_CHAR_LIMIT + 10);
        write(temp.path("AGENTS.md"), &long).unwrap();

        let report = load_instructions_with_global_home(temp.root(), None);

        assert!(report.docs[0].truncated);
        assert_eq!(
            report.docs[0].kind,
            InstructionSourceKind::ProjectInstruction
        );
        assert_eq!(
            report.docs[0].content.chars().count(),
            INSTRUCTION_CHAR_LIMIT
        );
        assert_eq!(
            report.summary_message(),
            "Instruction sources loaded: total=1 (global=0, project=1, config=0, plan=0)"
        );
    }

    #[test]
    fn load_instructions_ignores_missing_files() {
        let temp = TempRoot::new();

        let report = load_instructions_with_global_home(temp.root(), None);

        assert!(report.docs.is_empty());
        assert!(report.warnings.is_empty());
    }

    struct TempRoot {
        root: PathBuf,
    }

    impl TempRoot {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "picocode-instructions-{}-{}",
                std::process::id(),
                id
            ));
            let _ = remove_dir_all(&root);
            create_dir_all(&root).unwrap();
            Self { root }
        }

        fn root(&self) -> &Path {
            &self.root
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }
    }
}
