use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityKind {
    Extension,
    Skill,
}

impl CapabilityKind {
    fn label(self) -> &'static str {
        match self {
            Self::Extension => "extension",
            Self::Skill => "skill",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilitySource {
    Global,
    Project,
}

impl CapabilitySource {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityEntry {
    pub kind: CapabilityKind,
    pub source: CapabilitySource,
    pub name: String,
    pub summary: Option<String>,
    pub path: PathBuf,
}

impl CapabilityEntry {
    pub fn stable_id(&self) -> String {
        format!(
            "{}|{}|{}",
            self.source.label(),
            self.kind.label(),
            self.path.display()
        )
    }

    pub fn compact_label(&self) -> String {
        let summary = self.summary.as_deref().unwrap_or("-");
        format!(
            "{} {} {} - {}",
            self.source.label(),
            self.kind.label(),
            self.name,
            summary
        )
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.name.to_lowercase().contains(&query)
            || self
                .summary
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&query)
            || self.path.to_string_lossy().to_lowercase().contains(&query)
            || self.source.label().contains(&query)
            || self.kind.label().contains(&query)
    }

    pub fn detail_text(&self) -> io::Result<String> {
        match self.kind {
            CapabilityKind::Extension => {
                let manifest_path = self.path.join("manifest.toml");
                if !manifest_path.exists() {
                    return Ok(self.compact_label());
                }
                let content = fs::read_to_string(manifest_path)?;
                Ok(format!(
                    "path: {}\nkind: extension\nsource: {}\nname: {}\n{}\n{}\n",
                    self.path.display(),
                    self.source.label(),
                    self.name,
                    self.summary
                        .as_deref()
                        .map(|summary| format!("summary: {summary}"))
                        .unwrap_or_else(|| "summary: -".to_owned()),
                    limit_preview(&content, 12)
                ))
            }
            CapabilityKind::Skill => {
                let skill_path = self.path.join("SKILL.md");
                if !skill_path.exists() {
                    return Ok(self.compact_label());
                }
                let content = fs::read_to_string(skill_path)?;
                Ok(format!(
                    "path: {}\nkind: skill\nsource: {}\nname: {}\n{}\n{}\n",
                    self.path.display(),
                    self.source.label(),
                    self.name,
                    self.summary
                        .as_deref()
                        .map(|summary| format!("summary: {summary}"))
                        .unwrap_or_else(|| "summary: -".to_owned()),
                    limit_preview(&content, 16)
                ))
            }
        }
    }

    pub fn detail_text_with_enabled(&self, enabled: bool) -> io::Result<String> {
        let status = if enabled { "enabled" } else { "disabled" };
        let base = self.detail_text()?;
        Ok(format!("enabled: {status}\n{base}"))
    }

    pub fn skill_context_text(&self, enabled: bool) -> io::Result<String> {
        if self.kind != CapabilityKind::Skill {
            return self.detail_text_with_enabled(enabled);
        }
        let status = if enabled { "enabled" } else { "disabled" };
        let skill_path = self.path.join("SKILL.md");
        if !skill_path.exists() {
            return Ok(self.detail_text_with_enabled(enabled)?);
        }
        let content = fs::read_to_string(skill_path)?;
        Ok(format!(
            "skill loaded: {}\nenabled: {}\npath: {}\nsource: {}\nname: {}\n{}\n",
            self.name,
            status,
            self.path.display(),
            self.source.label(),
            self.name,
            content.trim_end()
        ))
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CapabilityIndex {
    pub entries: Vec<CapabilityEntry>,
}

impl CapabilityIndex {
    pub fn discover(project_root: impl AsRef<Path>) -> io::Result<Self> {
        let project_root = project_root.as_ref();
        let home_root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".picocode"));
        Self::discover_from_roots(home_root.as_deref(), project_root)
    }

    pub fn discover_from_roots(home_root: Option<&Path>, project_root: &Path) -> io::Result<Self> {
        let mut entries = Vec::new();
        if let Some(home_root) = home_root {
            entries.extend(discover_root(home_root, CapabilitySource::Global)?);
        }
        let project_root = project_root.canonicalize()?;
        let project_capabilities = project_root.join(".picocode");
        entries.extend(discover_root(
            &project_capabilities,
            CapabilitySource::Project,
        )?);
        entries.sort_by(|left, right| {
            (
                left.source.label(),
                left.kind.label(),
                left.name.as_str(),
                left.path.to_string_lossy(),
            )
                .cmp(&(
                    right.source.label(),
                    right.kind.label(),
                    right.name.as_str(),
                    right.path.to_string_lossy(),
                ))
        });
        Ok(Self { entries })
    }

    pub fn enabled_entries<'a>(
        &'a self,
        preferences: &CapabilityPreferences,
    ) -> Vec<&'a CapabilityEntry> {
        self.entries
            .iter()
            .filter(|entry| preferences.is_enabled(entry))
            .collect()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CapabilityPreferences {
    global_disabled: BTreeSet<String>,
    project_disabled: BTreeSet<String>,
    project_enabled: BTreeSet<String>,
    project_settings_path: PathBuf,
}

impl CapabilityPreferences {
    pub fn load(project_root: impl AsRef<Path>) -> io::Result<Self> {
        let project_root = project_root.as_ref();
        let project_settings_path = project_root.join(".picocode/capabilities.toml");
        let home_settings_path = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".picocode/capabilities.toml"));

        let global = match home_settings_path.as_deref() {
            Some(path) => read_capability_settings(path)?.unwrap_or_default(),
            None => CapabilitySettingsToml::default(),
        };
        let project = read_capability_settings(&project_settings_path)?.unwrap_or_default();

        Ok(Self {
            global_disabled: global.disabled.into_iter().collect(),
            project_disabled: project.disabled.into_iter().collect(),
            project_enabled: project.enabled.into_iter().collect(),
            project_settings_path,
        })
    }

    pub fn is_enabled(&self, entry: &CapabilityEntry) -> bool {
        let id = entry.stable_id();
        self.project_enabled.contains(&id)
            || (!self.global_disabled.contains(&id) && !self.project_disabled.contains(&id))
    }

    pub fn set_enabled(&mut self, entry: &CapabilityEntry, enabled: bool) -> io::Result<()> {
        let id = entry.stable_id();
        if enabled {
            self.project_disabled.remove(&id);
            self.project_enabled.insert(id);
        } else {
            self.project_enabled.remove(&id);
            self.project_disabled.insert(id);
        }
        self.save_project_settings()
    }

    fn save_project_settings(&self) -> io::Result<()> {
        let mut disabled = self.project_disabled.iter().cloned().collect::<Vec<_>>();
        disabled.sort();
        let mut enabled = self.project_enabled.iter().cloned().collect::<Vec<_>>();
        enabled.sort();
        let settings = CapabilitySettingsToml { disabled, enabled };
        let content = toml::to_string_pretty(&settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = self.project_settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.project_settings_path, content)
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CapabilitySettingsToml {
    #[serde(default)]
    disabled: Vec<String>,
    #[serde(default)]
    enabled: Vec<String>,
}

fn read_capability_settings(path: &Path) -> io::Result<Option<CapabilitySettingsToml>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    Ok(toml::from_str::<CapabilitySettingsToml>(&content).ok())
}

fn discover_root(root: &Path, source: CapabilitySource) -> io::Result<Vec<CapabilityEntry>> {
    let mut entries = Vec::new();
    for kind in [CapabilityKind::Extension, CapabilityKind::Skill] {
        let kind_dir = root.join(format!("{}s", kind.label()));
        if !kind_dir.exists() {
            continue;
        }
        for item in fs::read_dir(&kind_dir)? {
            let item = item?;
            let path = item.path();
            if !path.is_dir() {
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            match kind {
                CapabilityKind::Extension => {
                    let manifest_path = path.join("manifest.toml");
                    let summary = read_extension_manifest_summary(&manifest_path)?;
                    entries.push(CapabilityEntry {
                        kind,
                        source,
                        name: summary
                            .as_ref()
                            .and_then(|manifest| manifest.name.clone())
                            .unwrap_or(name),
                        summary: summary.and_then(|manifest| manifest.description),
                        path,
                    });
                }
                CapabilityKind::Skill => {
                    let skill_path = path.join("SKILL.md");
                    if !skill_path.exists() {
                        continue;
                    }
                    let summary = read_skill_summary(&skill_path)?;
                    entries.push(CapabilityEntry {
                        kind,
                        source,
                        name,
                        summary,
                        path,
                    });
                }
            }
        }
    }
    Ok(entries)
}

#[derive(Debug, Deserialize)]
struct ExtensionManifest {
    name: Option<String>,
    description: Option<String>,
}

fn read_extension_manifest_summary(path: &Path) -> io::Result<Option<ExtensionManifest>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    Ok(toml::from_str::<ExtensionManifest>(&content).ok())
}

fn read_skill_summary(path: &Path) -> io::Result<Option<String>> {
    let content = fs::read_to_string(path)?;
    let mut lines = content.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = line.strip_prefix("# ") {
            return Ok(Some(stripped.trim().to_owned()));
        }
        return Ok(Some(line.to_owned()));
    }
    Ok(None)
}

fn limit_preview(content: &str, limit: usize) -> String {
    let mut lines = content.lines().collect::<Vec<_>>();
    if lines.len() > limit {
        lines.truncate(limit);
        lines.push("... (collapsed)");
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::create_dir_all,
        fs::write,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_temp_dir() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("picocode-capability-test-{stamp}"))
    }

    #[test]
    fn discover_collects_global_and_project_capabilities() {
        let root = unique_temp_dir();
        let home = root.join("home");
        let project = root.join("project");
        create_dir_all(home.join(".picocode/extensions/hello")).unwrap();
        create_dir_all(home.join(".picocode/skills/guide")).unwrap();
        create_dir_all(project.join(".picocode/extensions/world")).unwrap();
        create_dir_all(project.join(".picocode/skills/usage")).unwrap();
        write(
            home.join(".picocode/extensions/hello/manifest.toml"),
            "name = \"hello\"\ndescription = \"hello extension\"\n",
        )
        .unwrap();
        write(home.join(".picocode/skills/guide/SKILL.md"), "# Guide\n").unwrap();
        write(
            project.join(".picocode/extensions/world/manifest.toml"),
            "name = \"world\"\ndescription = \"world extension\"\n",
        )
        .unwrap();
        write(project.join(".picocode/skills/usage/SKILL.md"), "# Usage\n").unwrap();

        let index =
            CapabilityIndex::discover_from_roots(Some(home.join(".picocode").as_path()), &project)
                .unwrap();

        assert_eq!(index.entries.len(), 4);
        assert!(index.entries.iter().any(|entry| {
            entry.source == CapabilitySource::Global
                && entry.kind == CapabilityKind::Extension
                && entry.name == "hello"
        }));
        assert!(index.entries.iter().any(|entry| {
            entry.source == CapabilitySource::Project
                && entry.kind == CapabilityKind::Skill
                && entry.name == "usage"
        }));
    }

    #[test]
    fn entry_matches_query_on_summary_and_path() {
        let entry = CapabilityEntry {
            kind: CapabilityKind::Skill,
            source: CapabilitySource::Project,
            name: "usage".to_owned(),
            summary: Some("Usage guide".to_owned()),
            path: PathBuf::from("/tmp/project/.picocode/skills/usage"),
        };

        assert!(entry.matches_query("usage"));
        assert!(entry.matches_query("guide"));
        assert!(entry.matches_query("skills/usage"));
        assert!(!entry.matches_query("missing"));
    }

    #[test]
    fn preferences_enable_disable_round_trip_on_project_settings() {
        let root = unique_temp_dir();
        let project = root.join("project");
        create_dir_all(project.join(".picocode/extensions/hello")).unwrap();
        write(
            project.join(".picocode/extensions/hello/manifest.toml"),
            "name = \"hello\"\ndescription = \"hello extension\"\n",
        )
        .unwrap();

        let index = CapabilityIndex::discover_from_roots(None, &project).unwrap();
        let entry = index
            .entries
            .into_iter()
            .find(|entry| entry.name == "hello")
            .unwrap();

        let mut preferences = CapabilityPreferences {
            global_disabled: BTreeSet::new(),
            project_disabled: BTreeSet::new(),
            project_enabled: BTreeSet::new(),
            project_settings_path: project.join(".picocode/capabilities.toml"),
        };

        assert!(preferences.is_enabled(&entry));
        preferences.set_enabled(&entry, false).unwrap();
        assert!(!preferences.is_enabled(&entry));

        let loaded = CapabilityPreferences::load(&project).unwrap();
        assert!(!loaded.is_enabled(&entry));

        preferences.set_enabled(&entry, true).unwrap();
        let loaded = CapabilityPreferences::load(&project).unwrap();
        assert!(loaded.is_enabled(&entry));
    }

    #[test]
    fn preferences_respect_global_disable_and_project_enable_override() {
        let entry = CapabilityEntry {
            kind: CapabilityKind::Extension,
            source: CapabilitySource::Global,
            name: "hello".to_owned(),
            summary: Some("hello extension".to_owned()),
            path: PathBuf::from("/tmp/home/.picocode/extensions/hello"),
        };

        let preferences = CapabilityPreferences {
            global_disabled: [entry.stable_id()].into_iter().collect(),
            project_disabled: BTreeSet::new(),
            project_enabled: BTreeSet::new(),
            project_settings_path: PathBuf::from("/tmp/project/.picocode/capabilities.toml"),
        };

        assert!(!preferences.is_enabled(&entry));
    }

    #[test]
    fn skill_context_text_loads_full_skill_markdown() {
        let root = unique_temp_dir();
        let project = root.join("project");
        create_dir_all(project.join(".picocode/skills/guide")).unwrap();
        write(
            project.join(".picocode/skills/guide/SKILL.md"),
            "# Guide\n\nUse this skill.\n",
        )
        .unwrap();

        let index = CapabilityIndex::discover_from_roots(None, &project).unwrap();
        let entry = index
            .entries
            .into_iter()
            .find(|entry| entry.kind == CapabilityKind::Skill)
            .unwrap();
        let text = entry.skill_context_text(true).unwrap();

        assert!(text.contains("skill loaded: guide"));
        assert!(text.contains("# Guide"));
        assert!(text.contains("Use this skill."));
    }
}
