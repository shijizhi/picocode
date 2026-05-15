use std::{
    collections::HashSet,
    fs, io,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_ENTRY_LIMIT: usize = 200;
const DEFAULT_READ_LIMIT: usize = 4000;
const DEFAULT_FIND_LIMIT: usize = 100;
const DEFAULT_GREP_LIMIT: usize = 100;
static EDIT_CHECKPOINT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    ignore: IgnoreRules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceOptions {
    pub respect_gitignore: bool,
}

impl Default for WorkspaceOptions {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
        }
    }
}

impl Workspace {
    #[allow(dead_code)]
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        Self::new_with_options(root, WorkspaceOptions::default())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn new_with_options(
        root: impl Into<PathBuf>,
        options: WorkspaceOptions,
    ) -> io::Result<Self> {
        let root = root.into().canonicalize()?;
        let ignore = IgnoreRules::load(&root, options.respect_gitignore)?;
        Ok(Self { root, ignore })
    }

    pub fn list(&self, path: &str, limit: usize) -> Result<ListResult, WorkspaceError> {
        let limit = nonzero_or_default(limit, DEFAULT_ENTRY_LIMIT);
        let dir = self.resolve_existing_path(path)?;
        if !dir.is_dir() {
            return Err(WorkspaceError::NotDirectory(self.relative_path(&dir)));
        }

        let mut entries = Vec::new();
        for entry in fs::read_dir(&dir).map_err(WorkspaceError::Io)? {
            let entry = entry.map_err(WorkspaceError::Io)?;
            let path = entry.path();
            if self
                .ignore
                .is_ignored(&self.relative_path(&path), path.is_dir())
            {
                continue;
            }

            entries.push(WorkspaceEntry {
                path: self.relative_path(&path),
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: path.is_dir(),
            });
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        let truncated = entries.len() > limit;
        entries.truncate(limit);

        Ok(ListResult {
            path: self.relative_path(&dir),
            entries,
            truncated,
            entry_limit: limit,
        })
    }

    pub fn read(
        &self,
        path: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ReadResult, WorkspaceError> {
        let limit = nonzero_or_default(limit, DEFAULT_READ_LIMIT);
        let file = self.resolve_existing_path(path)?;
        if !file.is_file() {
            return Err(WorkspaceError::NotFile(self.relative_path(&file)));
        }
        if self.ignore.is_ignored(&self.relative_path(&file), false) {
            return Err(WorkspaceError::Ignored(self.relative_path(&file)));
        }

        let content = fs::read_to_string(&file).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                WorkspaceError::NonUtf8(self.relative_path(&file))
            } else {
                WorkspaceError::Io(error)
            }
        })?;

        if content.contains('\0') {
            return Err(WorkspaceError::Binary(self.relative_path(&file)));
        }

        let total_chars = content.chars().count();
        let content = content.chars().skip(offset).take(limit).collect::<String>();
        let next_offset = offset.saturating_add(content.chars().count());
        let truncated = next_offset < total_chars;

        Ok(ReadResult {
            path: self.relative_path(&file),
            content,
            truncated,
            next_offset: truncated.then_some(next_offset),
        })
    }

    pub fn propose_edit(
        &self,
        path: &str,
        find: &str,
        replace: &str,
    ) -> Result<EditPreviewResult, WorkspaceError> {
        let target = self.load_edit_target(path, find)?;

        Ok(EditPreviewResult {
            path: target.relative_path.clone(),
            base_hash: target.base_hash.clone(),
            diff: build_edit_preview(
                &target.content,
                find,
                replace,
                &target.relative_path,
                target.match_index,
            ),
        })
    }

    pub fn propose_edits(
        &self,
        edits: &[EditRequest],
    ) -> Result<EditBatchPreviewResult, WorkspaceError> {
        let mut previews = Vec::new();
        let mut seen = HashSet::new();

        for edit in edits {
            if !seen.insert(edit.path.clone()) {
                return Err(WorkspaceError::EditConflict {
                    path: edit.path.clone(),
                    reason: "duplicate edit for the same file in one batch".to_owned(),
                });
            }
            let target = self.load_edit_target(&edit.path, &edit.find)?;
            previews.push(EditPreviewResult {
                path: target.relative_path.clone(),
                base_hash: target.base_hash.clone(),
                diff: build_edit_preview(
                    &target.content,
                    &edit.find,
                    &edit.replace,
                    &target.relative_path,
                    target.match_index,
                ),
            });
        }

        Ok(EditBatchPreviewResult { previews })
    }

    pub fn apply_edit(
        &self,
        path: &str,
        find: &str,
        replace: &str,
    ) -> Result<EditApplyResult, WorkspaceError> {
        let target = self.load_edit_target(path, find)?;
        self.ensure_edit_target_still_valid(&target)?;
        let updated_content = target.content.replacen(find, replace, 1);
        let relative_path = target.relative_path.clone();
        let diff = build_edit_preview(
            &target.content,
            find,
            replace,
            &relative_path,
            target.match_index,
        );
        let checkpoint_id = new_edit_checkpoint_id();
        let base_hash = content_hash(&target.content);
        let result_hash = content_hash(&updated_content);
        let current_content = self.read_full_file(&target.file)?;
        let current_hash = content_hash(&current_content);
        if current_hash != target.base_hash {
            return Err(WorkspaceError::EditCheckpointMismatch {
                path: relative_path,
                expected_hash: target.base_hash.clone(),
                actual_hash: current_hash,
            });
        }
        self.write_edit_atomically(&target.file, &updated_content)?;

        Ok(EditApplyResult {
            checkpoint: EditCheckpoint {
                checkpoint_id,
                path: relative_path,
                base_hash,
                result_hash,
                before_content: target.content,
                after_content: updated_content,
                diff,
            },
        })
    }

    pub fn apply_edits(
        &self,
        edits: &[EditRequest],
    ) -> Result<EditBatchApplyResult, WorkspaceError> {
        if edits.is_empty() {
            return Ok(EditBatchApplyResult { edits: Vec::new() });
        }

        let mut seen = HashSet::new();
        let mut targets = Vec::new();

        for edit in edits {
            if !seen.insert(edit.path.clone()) {
                return Err(WorkspaceError::EditConflict {
                    path: edit.path.clone(),
                    reason: "duplicate edit for the same file in one batch".to_owned(),
                });
            }
            let target = self.load_edit_target(&edit.path, &edit.find)?;
            targets.push((edit.clone(), target));
        }

        let mut applied = Vec::new();
        for (edit, target) in targets {
            let updated_content = target.content.replacen(&edit.find, &edit.replace, 1);
            let relative_path = target.relative_path.clone();
            let diff = build_edit_preview(
                &target.content,
                &edit.find,
                &edit.replace,
                &relative_path,
                target.match_index,
            );
            let checkpoint_id = new_edit_checkpoint_id();
            let base_hash = target.base_hash.clone();
            let result_hash = content_hash(&updated_content);
            let current_content = self.read_full_file(&target.file)?;
            let current_hash = content_hash(&current_content);
            let expected_hash = edit.expected_hash.as_ref().unwrap_or(&target.base_hash);
            if current_hash != *expected_hash {
                self.restore_batch_edits(&applied);
                return Err(WorkspaceError::EditCheckpointMismatch {
                    path: target.relative_path.clone(),
                    expected_hash: expected_hash.clone(),
                    actual_hash: current_hash,
                });
            }

            if let Err(error) = self.write_edit_atomically(&target.file, &updated_content) {
                self.restore_batch_edits(&applied);
                return Err(error);
            }

            applied.push(EditCheckpoint {
                checkpoint_id,
                path: relative_path,
                base_hash,
                result_hash,
                before_content: target.content,
                after_content: updated_content,
                diff,
            });
        }

        Ok(EditBatchApplyResult { edits: applied })
    }

    pub fn rewind_edit(
        &self,
        checkpoint: &EditCheckpoint,
    ) -> Result<EditRewindResult, WorkspaceError> {
        let file = self.resolve_existing_path(&checkpoint.path)?;
        if !file.is_file() {
            return Err(WorkspaceError::NotFile(self.relative_path(&file)));
        }
        if self.ignore.is_ignored(&self.relative_path(&file), false) {
            return Err(WorkspaceError::Ignored(self.relative_path(&file)));
        }

        let current_content = self.read_full_file(&file)?;
        let current_hash = content_hash(&current_content);
        if current_hash != checkpoint.result_hash {
            return Err(WorkspaceError::EditCheckpointMismatch {
                path: self.relative_path(&file),
                expected_hash: checkpoint.result_hash.clone(),
                actual_hash: current_hash,
            });
        }

        let restored_content = checkpoint.before_content.clone();
        let rewind_checkpoint = EditCheckpoint {
            checkpoint_id: new_edit_checkpoint_id(),
            path: checkpoint.path.clone(),
            base_hash: checkpoint.result_hash.clone(),
            result_hash: content_hash(&restored_content),
            before_content: current_content,
            after_content: restored_content.clone(),
            diff: build_content_diff(
                &checkpoint.after_content,
                &restored_content,
                &checkpoint.path,
            ),
        };

        self.write_edit_atomically(&file, &restored_content)?;

        Ok(EditRewindResult {
            rewound_checkpoint_id: checkpoint.checkpoint_id.clone(),
            checkpoint: rewind_checkpoint,
        })
    }

    pub fn find(
        &self,
        query: &str,
        path: &str,
        limit: usize,
    ) -> Result<FindResult, WorkspaceError> {
        let limit = nonzero_or_default(limit, DEFAULT_FIND_LIMIT);
        let search_root = self.resolve_existing_path(path)?;
        if !search_root.is_dir() {
            return Err(WorkspaceError::NotDirectory(
                self.relative_path(&search_root),
            ));
        }
        let root_relative = self.relative_path(&search_root);
        if !root_relative.is_empty() && self.ignore.is_ignored(&root_relative, true) {
            return Err(WorkspaceError::Ignored(root_relative));
        }

        let query = query.trim().to_lowercase();
        let mut matches = Vec::new();
        self.find_in_dir(&search_root, &query, &mut matches)?;
        matches.sort_by(|left, right| {
            left.score
                .cmp(&right.score)
                .then_with(|| left.entry.path.cmp(&right.entry.path))
        });
        let truncated = matches.len() > limit;
        let matches = matches
            .into_iter()
            .take(limit)
            .map(|candidate| candidate.entry)
            .collect::<Vec<_>>();

        Ok(FindResult {
            path: self.relative_path(&search_root),
            matches,
            truncated,
            match_limit: limit,
        })
    }

    fn find_in_dir(
        &self,
        dir: &Path,
        query: &str,
        matches: &mut Vec<FindCandidate>,
    ) -> Result<(), WorkspaceError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir).map_err(WorkspaceError::Io)? {
            entries.push(entry.map_err(WorkspaceError::Io)?);
        }
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(WorkspaceError::Io)?;
            let is_dir = file_type.is_dir();
            let relative_path = self.relative_path(&path);

            if self.ignore.is_ignored(&relative_path, is_dir) {
                continue;
            }

            if query.is_empty() || relative_path.to_lowercase().contains(query) {
                matches.push(FindCandidate {
                    entry: WorkspaceEntry {
                        path: relative_path.clone(),
                        name: entry.file_name().to_string_lossy().into_owned(),
                        is_dir,
                    },
                    score: find_score(&relative_path, is_dir, query),
                });
            }

            if is_dir {
                self.find_in_dir(&path, query, matches)?;
            }
        }

        Ok(())
    }

    pub fn grep(
        &self,
        query: &str,
        path: &str,
        limit: usize,
        ignore_case: bool,
    ) -> Result<GrepResult, WorkspaceError> {
        let limit = nonzero_or_default(limit, DEFAULT_GREP_LIMIT);
        let search_root = self.resolve_existing_path(path)?;
        let root_relative = self.relative_path(&search_root);
        if !root_relative.is_empty() && self.ignore.is_ignored(&root_relative, search_root.is_dir())
        {
            return Err(WorkspaceError::Ignored(root_relative));
        }

        let needle = if ignore_case {
            query.to_lowercase()
        } else {
            query.to_owned()
        };
        let mut matches = Vec::new();
        let truncated = if search_root.is_file() {
            self.grep_file(&search_root, &needle, limit, ignore_case, &mut matches)?
        } else if search_root.is_dir() {
            self.grep_dir(&search_root, &needle, limit, ignore_case, &mut matches)?
        } else {
            return Err(WorkspaceError::NotFile(root_relative));
        };

        Ok(GrepResult {
            path: self.relative_path(&search_root),
            matches,
            truncated,
            match_limit: limit,
        })
    }

    fn grep_dir(
        &self,
        dir: &Path,
        needle: &str,
        limit: usize,
        ignore_case: bool,
        matches: &mut Vec<GrepMatch>,
    ) -> Result<bool, WorkspaceError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(dir).map_err(WorkspaceError::Io)? {
            entries.push(entry.map_err(WorkspaceError::Io)?);
        }
        entries.sort_by_key(|entry| entry.path());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type().map_err(WorkspaceError::Io)?;
            let is_dir = file_type.is_dir();
            let relative_path = self.relative_path(&path);

            if self.ignore.is_ignored(&relative_path, is_dir) {
                continue;
            }

            if is_dir {
                if self.grep_dir(&path, needle, limit, ignore_case, matches)? {
                    return Ok(true);
                }
            } else if file_type.is_file()
                && self.grep_file(&path, needle, limit, ignore_case, matches)?
            {
                return Ok(true);
            }
        }

        Ok(false)
    }

    fn grep_file(
        &self,
        file: &Path,
        needle: &str,
        limit: usize,
        ignore_case: bool,
        matches: &mut Vec<GrepMatch>,
    ) -> Result<bool, WorkspaceError> {
        let content = match fs::read_to_string(file) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => return Ok(false),
            Err(error) => return Err(WorkspaceError::Io(error)),
        };
        if content.contains('\0') {
            return Ok(false);
        }

        for (line_index, line) in content.lines().enumerate() {
            let haystack = if ignore_case {
                line.to_lowercase()
            } else {
                line.to_owned()
            };
            if haystack.contains(needle) {
                if matches.len() >= limit {
                    return Ok(true);
                }
                matches.push(GrepMatch {
                    path: self.relative_path(file),
                    line_number: line_index + 1,
                    line: line.to_owned(),
                });
            }
        }

        Ok(false)
    }

    fn resolve_existing_path(&self, input: &str) -> Result<PathBuf, WorkspaceError> {
        let path = self.resolve_path(input)?;
        path.canonicalize()
            .map_err(|_| WorkspaceError::NotFound(clean_input(input)))
            .and_then(|path| self.ensure_inside_root(path))
    }

    fn load_edit_target(&self, path: &str, find: &str) -> Result<EditTarget, WorkspaceError> {
        if find.trim().is_empty() {
            return Err(WorkspaceError::EditPatternEmpty);
        }

        let file = self.resolve_existing_path(path)?;
        if !file.is_file() {
            return Err(WorkspaceError::NotFile(self.relative_path(&file)));
        }
        if self.ignore.is_ignored(&self.relative_path(&file), false) {
            return Err(WorkspaceError::Ignored(self.relative_path(&file)));
        }

        let content = fs::read_to_string(&file).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                WorkspaceError::NonUtf8(self.relative_path(&file))
            } else {
                WorkspaceError::Io(error)
            }
        })?;
        if content.contains('\0') {
            return Err(WorkspaceError::Binary(self.relative_path(&file)));
        }

        let relative_path = self.relative_path(&file);
        let Some(match_index) = content.find(find) else {
            return Err(WorkspaceError::EditTargetNotFound(relative_path));
        };
        let base_hash = content_hash(&content);

        Ok(EditTarget {
            file,
            relative_path,
            content,
            match_index,
            base_hash,
        })
    }

    fn ensure_edit_target_still_valid(&self, target: &EditTarget) -> Result<(), WorkspaceError> {
        let current_content = self.read_full_file(&target.file)?;
        let current_hash = content_hash(&current_content);
        if current_hash != target.base_hash {
            return Err(WorkspaceError::EditCheckpointMismatch {
                path: target.relative_path.clone(),
                expected_hash: target.base_hash.clone(),
                actual_hash: current_hash,
            });
        }
        Ok(())
    }

    fn write_edit_atomically(&self, file: &Path, content: &str) -> Result<(), WorkspaceError> {
        let parent = file
            .parent()
            .ok_or_else(|| WorkspaceError::NotFound(file.display().to_string()))?;
        let temp_path = parent.join(format!(
            ".{}.picocode-tmp-{}",
            file.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("edit"),
            epoch_millis()
        ));
        fs::write(&temp_path, content).map_err(WorkspaceError::Io)?;
        match fs::rename(&temp_path, file) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&temp_path);
                Err(WorkspaceError::Io(error))
            }
        }
    }

    fn resolve_path(&self, input: &str) -> Result<PathBuf, WorkspaceError> {
        let input = clean_input(input);
        if input.is_empty() {
            return Ok(self.root.clone());
        }

        let path = PathBuf::from(&input);
        reject_parent_segments(&path)?;

        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.root.join(path))
        }
    }

    fn ensure_inside_root(&self, path: PathBuf) -> Result<PathBuf, WorkspaceError> {
        if path == self.root || path.starts_with(&self.root) {
            return Ok(path);
        }
        Err(WorkspaceError::PathEscapesRoot(path.display().to_string()))
    }

    fn relative_path(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .trim_start_matches('/')
            .to_owned()
    }

    fn read_full_file(&self, file: &Path) -> Result<String, WorkspaceError> {
        fs::read_to_string(file).map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidData {
                WorkspaceError::NonUtf8(self.relative_path(file))
            } else {
                WorkspaceError::Io(error)
            }
        })
    }

    fn restore_batch_edits(&self, applied: &[EditCheckpoint]) {
        for checkpoint in applied.iter().rev() {
            let file = self.root.join(&checkpoint.path);
            let _ = self.write_edit_atomically(&file, &checkpoint.before_content);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}

impl WorkspaceEntry {
    pub fn display_name(&self) -> String {
        if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListResult {
    pub path: String,
    pub entries: Vec<WorkspaceEntry>,
    pub truncated: bool,
    pub entry_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadResult {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindResult {
    pub path: String,
    pub matches: Vec<WorkspaceEntry>,
    pub truncated: bool,
    pub match_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepResult {
    pub path: String,
    pub matches: Vec<GrepMatch>,
    pub truncated: bool,
    pub match_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPreviewResult {
    pub path: String,
    pub base_hash: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRequest {
    pub path: String,
    pub find: String,
    pub replace: String,
    pub expected_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditApplyResult {
    pub checkpoint: EditCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBatchPreviewResult {
    pub previews: Vec<EditPreviewResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditBatchApplyResult {
    pub edits: Vec<EditCheckpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRewindResult {
    pub rewound_checkpoint_id: String,
    pub checkpoint: EditCheckpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCheckpoint {
    pub path: String,
    pub checkpoint_id: String,
    pub base_hash: String,
    pub result_hash: String,
    pub before_content: String,
    pub after_content: String,
    pub diff: String,
}

#[allow(dead_code)]
pub type EditRollbackResult = EditRewindResult;

#[derive(Debug)]
pub enum WorkspaceError {
    Io(io::Error),
    PathEscapesRoot(String),
    NotFound(String),
    NotDirectory(String),
    NotFile(String),
    Ignored(String),
    NonUtf8(String),
    Binary(String),
    EditPatternEmpty,
    EditTargetNotFound(String),
    EditCheckpointMismatch {
        path: String,
        expected_hash: String,
        actual_hash: String,
    },
    EditConflict {
        path: String,
        reason: String,
    },
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "workspace io failed: {error}"),
            Self::PathEscapesRoot(path) => write!(formatter, "path escapes workspace root: {path}"),
            Self::NotFound(path) => write!(formatter, "path not found: {path}"),
            Self::NotDirectory(path) => write!(formatter, "path is not a directory: {path}"),
            Self::NotFile(path) => write!(formatter, "path is not a file: {path}"),
            Self::Ignored(path) => write!(formatter, "path is ignored: {path}"),
            Self::NonUtf8(path) => write!(formatter, "file is not valid UTF-8: {path}"),
            Self::Binary(path) => write!(formatter, "file appears to be binary: {path}"),
            Self::EditPatternEmpty => write!(formatter, "edit preview pattern must not be empty"),
            Self::EditTargetNotFound(path) => {
                write!(formatter, "edit preview target not found in: {path}")
            }
            Self::EditCheckpointMismatch {
                path,
                expected_hash,
                actual_hash,
            } => {
                write!(
                    formatter,
                    "edit checkpoint mismatch for {path}: expected {expected_hash}, found {actual_hash}"
                )
            }
            Self::EditConflict { path, reason } => {
                write!(formatter, "edit conflict for {path}: {reason}")
            }
        }
    }
}

impl std::error::Error for WorkspaceError {}

#[derive(Debug, Clone, Default)]
struct IgnoreRules {
    exact: HashSet<String>,
    prefixes: Vec<String>,
    suffixes: Vec<String>,
}

impl IgnoreRules {
    fn load(root: &Path, respect_gitignore: bool) -> io::Result<Self> {
        let mut rules = Self::default();
        rules.prefixes.extend([
            ".git/".to_owned(),
            "target/".to_owned(),
            ".picocode/".to_owned(),
        ]);

        if !respect_gitignore {
            return Ok(rules);
        }

        let gitignore = root.join(".gitignore");
        if !gitignore.exists() {
            return Ok(rules);
        }

        for line in fs::read_to_string(gitignore)?.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            rules.add_rule(line);
        }

        Ok(rules)
    }

    fn add_rule(&mut self, rule: &str) {
        let rule = rule.trim_start_matches('/').to_owned();
        if rule.ends_with('/') {
            self.prefixes.push(rule);
        } else if let Some(suffix) = rule.strip_prefix("*.") {
            self.suffixes.push(format!(".{suffix}"));
        } else {
            self.exact.insert(rule);
        }
    }

    fn is_ignored(&self, relative_path: &str, is_dir: bool) -> bool {
        let relative_path = relative_path.trim_start_matches('/');
        if relative_path.is_empty() {
            return false;
        }

        if self.exact.contains(relative_path)
            || relative_path
                .rsplit('/')
                .any(|component| self.exact.contains(component))
        {
            return true;
        }

        let dir_path = if is_dir {
            format!("{relative_path}/")
        } else {
            relative_path.to_owned()
        };

        self.prefixes
            .iter()
            .any(|prefix| dir_path == *prefix || dir_path.starts_with(prefix))
            || self
                .suffixes
                .iter()
                .any(|suffix| relative_path.ends_with(suffix))
    }
}

fn clean_input(input: &str) -> String {
    input
        .trim()
        .trim_start_matches('@')
        .trim_start_matches("./")
        .to_owned()
}

fn reject_parent_segments(path: &Path) -> Result<(), WorkspaceError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(WorkspaceError::PathEscapesRoot(path.display().to_string()));
    }
    Ok(())
}

fn build_edit_preview(
    content: &str,
    find: &str,
    replace: &str,
    path: &str,
    match_index: usize,
) -> String {
    let (start_line, start_column) = line_and_column_at(content, match_index);
    let end_index = match_index.saturating_add(find.len());
    let (end_line, _) = line_and_column_at(content, end_index);
    let old_lines = extract_line_block(content, start_line, end_line);
    let new_lines = replace.lines().map(str::to_owned).collect::<Vec<_>>();
    let old_count = old_lines.len().max(1);
    let new_count = new_lines.len().max(1);

    let mut diff = vec![
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        format!(
            "@@ -{},{} +{},{} @@",
            start_line + 1,
            old_count,
            start_line + 1,
            new_count
        ),
        format!(
            "match: line {}, column {}",
            start_line + 1,
            start_column + 1
        ),
    ];
    diff.extend(old_lines.into_iter().map(|line| format!("-{line}")));
    if new_lines.is_empty() {
        diff.push("+".to_owned());
    } else {
        diff.extend(new_lines.into_iter().map(|line| format!("+{line}")));
    }
    diff.join("\n")
}

fn build_content_diff(before: &str, after: &str, path: &str) -> String {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut diff = vec![
        format!("--- a/{path}"),
        format!("+++ b/{path}"),
        "@@".to_owned(),
    ];
    let line_count = before_lines.len().max(after_lines.len());

    for index in 0..line_count {
        match (before_lines.get(index), after_lines.get(index)) {
            (Some(old), Some(new)) if old == new => diff.push(format!(" {old}")),
            (Some(old), Some(new)) => {
                diff.push(format!("-{old}"));
                diff.push(format!("+{new}"));
            }
            (Some(old), None) => diff.push(format!("-{old}")),
            (None, Some(new)) => diff.push(format!("+{new}")),
            (None, None) => {}
        }
    }

    diff.join("\n")
}

fn new_edit_checkpoint_id() -> String {
    format!(
        "checkpoint-{}-{}",
        epoch_millis(),
        EDIT_CHECKPOINT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn content_hash(content: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
struct EditTarget {
    file: PathBuf,
    relative_path: String,
    content: String,
    match_index: usize,
    base_hash: String,
}

fn line_and_column_at(content: &str, byte_index: usize) -> (usize, usize) {
    let mut line = 0;
    let mut column = 0;
    for (index, character) in content.char_indices() {
        if index >= byte_index {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    (line, column)
}

fn extract_line_block(content: &str, start_line: usize, end_line: usize) -> Vec<String> {
    content
        .lines()
        .enumerate()
        .filter(|(index, _)| *index >= start_line && *index <= end_line)
        .map(|(_, line)| line.to_owned())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FindCandidate {
    entry: WorkspaceEntry,
    score: usize,
}

fn find_score(relative_path: &str, is_dir: bool, query: &str) -> usize {
    let normalized = relative_path.to_lowercase();
    let basename = normalized.rsplit('/').next().unwrap_or(&normalized);
    let mut score = if basename == query {
        0
    } else if basename.contains(query) {
        1
    } else if normalized.contains(query) {
        2
    } else {
        3
    };
    if is_dir {
        score += 1;
    }
    score
}

fn nonzero_or_default(value: usize, default: usize) -> usize {
    if value == 0 {
        default
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, create_dir_all, write},
        sync::atomic::{AtomicU64, Ordering},
    };

    #[test]
    fn list_returns_sorted_entries_and_marks_directories() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src")).unwrap();
        write(temp.path("Cargo.toml"), "[package]\n").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.list(".", 10).unwrap();

        assert_eq!(
            result
                .entries
                .iter()
                .map(WorkspaceEntry::display_name)
                .collect::<Vec<_>>(),
            vec!["Cargo.toml", "src/"]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn read_supports_offset_limit_and_next_offset() {
        let temp = TempWorkspace::new();
        write(temp.path("notes.txt"), "abcdef").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.read("notes.txt", 2, 3).unwrap();

        assert_eq!(result.content, "cde");
        assert!(result.truncated);
        assert_eq!(result.next_offset, Some(5));
    }

    #[test]
    fn read_rejects_parent_escape() {
        let temp = TempWorkspace::new();
        let workspace = Workspace::new(temp.root()).unwrap();

        let error = workspace.read("../secret.txt", 0, 10).unwrap_err();

        assert!(matches!(error, WorkspaceError::PathEscapesRoot(_)));
    }

    #[test]
    fn propose_edit_returns_diff_without_changing_file() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let preview = workspace
            .propose_edit(
                "main.rs",
                "println!(\"hello\")",
                "println!(\"hello world\")",
            )
            .unwrap();

        assert_eq!(preview.path, "main.rs");
        assert!(preview.diff.contains("--- a/main.rs"));
        assert!(preview.diff.contains("+++ b/main.rs"));
        assert!(preview.diff.contains("println!(\"hello world\")"));
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
    }

    #[test]
    fn apply_edit_writes_file_and_records_checkpoint() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace
            .apply_edit(
                "main.rs",
                "println!(\"hello\")",
                "println!(\"hello world\")",
            )
            .unwrap();

        assert_eq!(result.checkpoint.path, "main.rs");
        assert!(!result.checkpoint.checkpoint_id.is_empty());
        assert!(result.checkpoint.diff.contains("println!(\"hello world\")"));
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello world\");\n}\n"
        );
        assert_eq!(
            result.checkpoint.before_content,
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
        assert_eq!(
            result.checkpoint.after_content,
            "fn main() {\n    println!(\"hello world\");\n}\n"
        );
    }

    #[test]
    fn rewind_edit_restores_checkpoint_content() {
        let temp = TempWorkspace::new();
        write(
            temp.path("main.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let apply = workspace
            .apply_edit(
                "main.rs",
                "println!(\"hello\")",
                "println!(\"hello world\")",
            )
            .unwrap();
        let rewind = workspace.rewind_edit(&apply.checkpoint).unwrap();

        assert_eq!(rewind.rewound_checkpoint_id, apply.checkpoint.checkpoint_id);
        assert_eq!(
            fs::read_to_string(temp.path("main.rs")).unwrap(),
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
        assert_eq!(
            rewind.checkpoint.before_content,
            "fn main() {\n    println!(\"hello world\");\n}\n"
        );
        assert_eq!(
            rewind.checkpoint.after_content,
            "fn main() {\n    println!(\"hello\");\n}\n"
        );
    }

    #[test]
    fn list_respects_basic_gitignore_rules() {
        let temp = TempWorkspace::new();
        write(temp.path(".gitignore"), "ignored.txt\nbuild/\n*.log\n").unwrap();
        write(temp.path("visible.txt"), "ok").unwrap();
        write(temp.path("ignored.txt"), "no").unwrap();
        write(temp.path("trace.log"), "no").unwrap();
        create_dir_all(temp.path("build")).unwrap();
        write(temp.path("build/output.txt"), "no").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.list(".", 20).unwrap();

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".gitignore", "visible.txt"]
        );
    }

    #[test]
    fn workspace_can_disable_gitignore_rules() {
        let temp = TempWorkspace::new();
        write(temp.path(".gitignore"), "ignored.txt\n").unwrap();
        write(temp.path("visible.txt"), "ok").unwrap();
        write(temp.path("ignored.txt"), "no").unwrap();

        let workspace = Workspace::new_with_options(
            temp.root(),
            WorkspaceOptions {
                respect_gitignore: false,
            },
        )
        .unwrap();
        let result = workspace.list(".", 20).unwrap();

        assert_eq!(
            result
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec![".gitignore", "ignored.txt", "visible.txt"]
        );
    }

    #[test]
    fn find_returns_recursive_path_matches() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src/ui")).unwrap();
        write(temp.path("src/ui/mod.rs"), "ui").unwrap();
        write(temp.path("README.md"), "readme").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.find("ui", ".", 10).unwrap();

        assert_eq!(
            result
                .matches
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/ui", "src/ui/mod.rs"]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn find_respects_ignore_rules_and_limit() {
        let temp = TempWorkspace::new();
        write(temp.path(".gitignore"), "target/\n*.log\n").unwrap();
        create_dir_all(temp.path("src")).unwrap();
        create_dir_all(temp.path("examples")).unwrap();
        create_dir_all(temp.path("target")).unwrap();
        write(temp.path("src/app.rs"), "app").unwrap();
        write(temp.path("examples/app.rs"), "app").unwrap();
        write(temp.path("target/app.rs"), "app").unwrap();
        write(temp.path("debug.log"), "log").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.find("app", ".", 1).unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "examples/app.rs");
        assert!(result.truncated);
    }

    #[test]
    fn grep_returns_line_matches_recursively() {
        let temp = TempWorkspace::new();
        create_dir_all(temp.path("src")).unwrap();
        write(
            temp.path("src/app.rs"),
            "fn main() {}\nlet name = \"pico\";\n",
        )
        .unwrap();
        write(temp.path("README.md"), "picocode\n").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.grep("pico", ".", 10, false).unwrap();

        assert_eq!(
            result
                .matches
                .iter()
                .map(|hit| format!("{}:{}:{}", hit.path, hit.line_number, hit.line))
                .collect::<Vec<_>>(),
            vec!["README.md:1:picocode", "src/app.rs:2:let name = \"pico\";"]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn grep_supports_ignore_case_and_skips_ignored_files() {
        let temp = TempWorkspace::new();
        write(temp.path(".gitignore"), "*.log\n").unwrap();
        write(temp.path("app.rs"), "Picocode\npicocode\n").unwrap();
        write(temp.path("debug.log"), "picocode\n").unwrap();

        let workspace = Workspace::new(temp.root()).unwrap();
        let result = workspace.grep("pico", ".", 1, true).unwrap();

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].path, "app.rs");
        assert!(result.truncated);
    }

    struct TempWorkspace {
        root: PathBuf,
    }

    impl TempWorkspace {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "picocode-workspace-test-{}-{}",
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
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}
