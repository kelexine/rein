// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-20
// Purpose: Claude-compatible plugin and marketplace manager backed by git clones under ~/.rein/plugins

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use rein_tools::McpServerConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Official Anthropic Claude plugins marketplace repository URL.
pub const OFFICIAL_MARKETPLACE_NAME: &str = "claude-plugins-official";
/// Official Anthropic Claude plugins git clone URL.
pub const OFFICIAL_MARKETPLACE_URL: &str =
    "https://github.com/anthropics/claude-plugins-official.git";

/// Registered plugin marketplace source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    /// Short marketplace identifier (e.g. `claude-plugins-official`).
    pub name: String,
    /// Git clone URL or local path.
    pub url: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Last recorded sync unix timestamp (0 if never synced locally).
    #[serde(default)]
    pub last_synced_secs: u64,
}

/// Source specification inside `.claude-plugin/marketplace.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginSourceSpec {
    /// Relative path inside the marketplace repo (e.g. `./plugins/code-review`).
    LocalPath(String),
    /// Structured external source (`{ "source": "github" | "url", "repo": "...", "url": "..." }`).
    Object {
        /// Source kind (`"github"`, `"url"`, `"local"`).
        #[serde(default)]
        source: String,
        /// GitHub `owner/repo` slug.
        #[serde(default)]
        repo: Option<String>,
        /// Full git clone URL.
        #[serde(default)]
        url: Option<String>,
        /// Relative path inside the marketplace repo.
        #[serde(default)]
        path: Option<String>,
    },
}

/// Raw entry in `.claude-plugin/marketplace.json`.
#[derive(Debug, Clone, Deserialize)]
struct RawMarketplaceManifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    plugins: Vec<RawMarketplacePlugin>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawMarketplacePlugin {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    category: String,
    source: Option<PluginSourceSpec>,
}

/// Raw `.claude-plugin/plugin.json` manifest.
#[derive(Debug, Clone, Default, Deserialize)]
struct RawPluginManifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

/// Catalog entry surfaced in `/plugins` Discover tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverPluginEntry {
    /// Plugin name.
    pub name: String,
    /// Marketplace identifier where this plugin was found.
    pub marketplace: String,
    /// Short description.
    pub description: String,
    /// Declared version string.
    pub version: String,
    /// Optional category tag.
    pub category: String,
    /// Source specification for installation.
    pub source: Option<PluginSourceSpec>,
    /// Whether this plugin is currently installed locally.
    pub installed: bool,
    /// Whether this plugin is currently enabled (when installed).
    pub enabled: bool,
}

/// Persisted installed plugin metadata under `~/.rein/plugins/installed.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledPluginRecord {
    /// Plugin name.
    pub name: String,
    /// Marketplace it came from (`local` if manually placed).
    pub marketplace: String,
    /// Version string from `plugin.json` or marketplace catalog.
    #[serde(default)]
    pub version: String,
    /// Description from `plugin.json` or marketplace catalog.
    #[serde(default)]
    pub description: String,
    /// Whether the plugin's commands, agents, skills, and MCP servers are active.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Local directory containing `.claude-plugin/plugin.json`.
    pub install_path: PathBuf,
    /// Unix timestamp when installed.
    #[serde(default)]
    pub installed_at_secs: u64,
}

fn default_true() -> bool {
    true
}

/// Slash command discovered from `<plugin>/commands/*.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// Owning plugin name.
    pub plugin: String,
    /// Namespaced command name (`<plugin>:<cmd>`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// Markdown prompt body.
    pub template: String,
}

/// Agent definition discovered from `<plugin>/agents/*.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAgent {
    /// Owning plugin name.
    pub plugin: String,
    /// Namespaced agent name (`<plugin>:<agent>`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// System instructions from markdown body.
    pub instructions: String,
}

/// Skill discovered from `<plugin>/skills/<skill>/SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSkill {
    /// Owning plugin name.
    pub plugin: String,
    /// Namespaced skill name (`<plugin>:<skill>`).
    pub name: String,
    /// Short description.
    pub description: String,
    /// Path to `SKILL.md`.
    pub path: PathBuf,
}

/// Aggregated components from all enabled plugins.
#[derive(Debug, Clone, Default)]
pub struct EnabledPluginBundle {
    /// Custom slash commands.
    pub commands: Vec<PluginCommand>,
    /// Agent definitions surfaced in `@` mentions and subagents.
    pub agents: Vec<PluginAgent>,
    /// Skill references.
    pub skills: Vec<PluginSkill>,
    /// MCP server configurations from `.mcp.json`.
    pub mcp_servers: Vec<McpServerConfig>,
    /// Non-fatal parse/load warnings surfaced in `/plugins` Errors tab.
    pub errors: Vec<String>,
}

/// Thread-safe plugin and marketplace manager.
#[derive(Debug, Clone)]
pub struct PluginManager {
    root: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl PluginManager {
    /// Creates a [`PluginManager`] rooted at `root` (typically `~/.rein/plugins`).
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        let mgr = Self {
            root,
            lock: Arc::new(Mutex::new(())),
        };
        mgr.ensure_layout();
        mgr
    }

    /// Returns the plugin root directory (`~/.rein/plugins`).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn marketplaces_file(&self) -> PathBuf {
        self.root.join("marketplaces.json")
    }

    fn marketplaces_dir(&self) -> PathBuf {
        self.root.join("marketplaces")
    }

    fn installed_file(&self) -> PathBuf {
        self.root.join("installed.json")
    }

    fn installed_dir(&self) -> PathBuf {
        self.root.join("installed")
    }

    fn ensure_layout(&self) {
        let _ = std::fs::create_dir_all(self.marketplaces_dir());
        let _ = std::fs::create_dir_all(self.installed_dir());
        let mp_path = self.marketplaces_file();
        if !mp_path.exists() {
            let defaults = vec![MarketplaceEntry {
                name: OFFICIAL_MARKETPLACE_NAME.to_owned(),
                url: OFFICIAL_MARKETPLACE_URL.to_owned(),
                description: "Official Anthropic Claude Code plugin marketplace".to_owned(),
                last_synced_secs: 0,
            }];
            if let Ok(raw) = serde_json::to_string_pretty(&defaults) {
                let _ = std::fs::write(&mp_path, format!("{raw}\n"));
            }
        }
        let inst_path = self.installed_file();
        if !inst_path.exists() {
            let _ = std::fs::write(&inst_path, "[]\n");
        }
    }

    /// Lists all configured marketplaces.
    #[must_use]
    pub fn list_marketplaces(&self) -> Vec<MarketplaceEntry> {
        let _guard = self.lock.lock();
        self.read_marketplaces_unlocked()
    }

    fn read_marketplaces_unlocked(&self) -> Vec<MarketplaceEntry> {
        std::fs::read_to_string(self.marketplaces_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_marketplaces_unlocked(&self, entries: &[MarketplaceEntry]) -> Result<(), String> {
        let encoded = serde_json::to_string_pretty(entries)
            .map_err(|e| format!("failed to serialize marketplaces: {e}"))?;
        std::fs::write(self.marketplaces_file(), format!("{encoded}\n"))
            .map_err(|e| format!("failed to write marketplaces.json: {e}"))
    }

    /// Adds a marketplace by GitHub `owner/repo` shorthand or full git URL.
    pub fn add_marketplace(&self, spec: &str) -> Result<MarketplaceEntry, String> {
        let trimmed = spec.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err("marketplace repo or URL cannot be empty".to_owned());
        }
        let (name, url) = if trimmed.contains("://") || trimmed.starts_with("git@") {
            let base = trimmed
                .rsplit('/')
                .next()
                .unwrap_or("custom-marketplace")
                .trim_end_matches(".git")
                .to_owned();
            (base, trimmed.to_owned())
        } else if trimmed.contains('/') {
            let base = trimmed
                .rsplit('/')
                .next()
                .unwrap_or("custom-marketplace")
                .to_owned();
            (base, format!("https://github.com/{trimmed}.git"))
        } else {
            return Err("expected 'owner/repo' or git URL".to_owned());
        };

        let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
        let mut list = self.read_marketplaces_unlocked();
        if list.iter().any(|m| m.name == name) {
            return Err(format!("marketplace '{name}' is already registered"));
        }
        let entry = MarketplaceEntry {
            name,
            url,
            description: "Custom git plugin marketplace".to_owned(),
            last_synced_secs: 0,
        };
        list.push(entry.clone());
        self.write_marketplaces_unlocked(&list)?;
        Ok(entry)
    }

    /// Removes a configured marketplace and its local git clone.
    pub fn remove_marketplace(&self, name: &str) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
        let mut list = self.read_marketplaces_unlocked();
        let before = list.len();
        list.retain(|m| m.name != name);
        if list.len() == before {
            return Err(format!("unknown marketplace '{name}'"));
        }
        self.write_marketplaces_unlocked(&list)?;
        let clone_dir = self.marketplaces_dir().join(name);
        if clone_dir.exists() {
            let _ = std::fs::remove_dir_all(&clone_dir);
        }
        Ok(())
    }

    /// Clones or updates a marketplace via `git` and returns the number of plugins indexed.
    pub fn refresh_marketplace(&self, name: &str) -> Result<usize, String> {
        let entry = {
            let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
            self.read_marketplaces_unlocked()
                .into_iter()
                .find(|m| m.name == name)
                .ok_or_else(|| format!("unknown marketplace '{name}'"))?
        };
        let target_dir = self.marketplaces_dir().join(&entry.name);
        if target_dir.join(".git").exists() {
            run_git(&["-C", &target_dir.to_string_lossy(), "pull", "--ff-only"])?;
        } else {
            if target_dir.exists() {
                let _ = std::fs::remove_dir_all(&target_dir);
            }
            run_git(&[
                "clone",
                "--depth",
                "1",
                &entry.url,
                &target_dir.to_string_lossy(),
            ])?;
        }

        let now = crate::agent::now_unix_secs();
        {
            let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
            let mut list = self.read_marketplaces_unlocked();
            if let Some(item) = list.iter_mut().find(|m| m.name == entry.name) {
                item.last_synced_secs = now;
            }
            let _ = self.write_marketplaces_unlocked(&list);
        }

        let plugins = parse_marketplace_dir(&entry.name, &target_dir, &[])?;
        Ok(plugins.len())
    }

    /// Lists installed plugins from `installed.json`.
    #[must_use]
    pub fn installed_plugins(&self) -> Vec<InstalledPluginRecord> {
        let _guard = self.lock.lock();
        self.read_installed_unlocked()
    }

    fn read_installed_unlocked(&self) -> Vec<InstalledPluginRecord> {
        std::fs::read_to_string(self.installed_file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn write_installed_unlocked(&self, records: &[InstalledPluginRecord]) -> Result<(), String> {
        let encoded = serde_json::to_string_pretty(records)
            .map_err(|e| format!("failed to serialize installed plugins: {e}"))?;
        std::fs::write(self.installed_file(), format!("{encoded}\n"))
            .map_err(|e| format!("failed to write installed.json: {e}"))
    }

    /// Returns discoverable plugins from locally cloned marketplaces.
    #[must_use]
    pub fn discover_catalog(&self, marketplace_filter: Option<&str>) -> Vec<DiscoverPluginEntry> {
        let _guard = self.lock.lock();
        let marketplaces = self.read_marketplaces_unlocked();
        let installed = self.read_installed_unlocked();
        let mut out = Vec::new();
        for mp in marketplaces {
            if let Some(filter) = marketplace_filter
                && mp.name != filter
            {
                continue;
            }
            let dir = self.marketplaces_dir().join(&mp.name);
            if !dir.exists() {
                continue;
            }
            if let Ok(mut items) = parse_marketplace_dir(&mp.name, &dir, &installed) {
                out.append(&mut items);
            }
        }
        out
    }

    /// Installs a plugin from a marketplace entry using `git` (for remote sources) or copying from the cloned marketplace.
    pub fn install_plugin(
        &self,
        entry: &DiscoverPluginEntry,
    ) -> Result<InstalledPluginRecord, String> {
        let dest_dir = self.installed_dir().join(&entry.name);
        if dest_dir.exists() {
            std::fs::remove_dir_all(&dest_dir)
                .map_err(|e| format!("failed to clear existing plugin dir: {e}"))?;
        }

        let mp_dir = self.marketplaces_dir().join(&entry.marketplace);
        match &entry.source {
            Some(PluginSourceSpec::LocalPath(rel)) => {
                let src = mp_dir.join(rel.trim_start_matches("./"));
                if !src.exists() {
                    return Err(format!(
                        "plugin source '{}' not found in marketplace",
                        src.display()
                    ));
                }
                copy_dir_recursive(&src, &dest_dir)?;
            }
            Some(PluginSourceSpec::Object {
                source,
                repo,
                url,
                path,
            }) => {
                let git_url = if let Some(u) = url.as_deref().filter(|s| !s.is_empty()) {
                    u.to_owned()
                } else if let Some(r) = repo.as_deref().filter(|s| !s.is_empty()) {
                    format!("https://github.com/{r}.git")
                } else if source == "local"
                    && let Some(p) = path.as_deref()
                {
                    let src = mp_dir.join(p.trim_start_matches("./"));
                    copy_dir_recursive(&src, &dest_dir)?;
                    String::new()
                } else {
                    return Err(format!(
                        "unsupported plugin source '{source}' for '{}'",
                        entry.name
                    ));
                };
                if !git_url.is_empty() {
                    run_git(&[
                        "clone",
                        "--depth",
                        "1",
                        &git_url,
                        &dest_dir.to_string_lossy(),
                    ])?;
                }
            }
            None => {
                // Conventional fallback inside marketplace repo: `plugins/<name>`.
                let candidate = mp_dir.join("plugins").join(&entry.name);
                if !candidate.exists() {
                    return Err(format!(
                        "plugin '{}' has no source field and '{}' does not exist",
                        entry.name,
                        candidate.display()
                    ));
                }
                copy_dir_recursive(&candidate, &dest_dir)?;
            }
        }

        let manifest = read_plugin_manifest(&dest_dir);
        let record = InstalledPluginRecord {
            name: if manifest.name.is_empty() {
                entry.name.clone()
            } else {
                manifest.name
            },
            marketplace: entry.marketplace.clone(),
            version: if manifest.version.is_empty() {
                entry.version.clone()
            } else {
                manifest.version
            },
            description: if manifest.description.is_empty() {
                entry.description.clone()
            } else {
                manifest.description
            },
            enabled: true,
            install_path: dest_dir,
            installed_at_secs: crate::agent::now_unix_secs(),
        };

        let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
        let mut list = self.read_installed_unlocked();
        list.retain(|p| p.name != record.name);
        list.push(record.clone());
        self.write_installed_unlocked(&list)?;
        Ok(record)
    }

    /// Toggles `enabled` on an installed plugin and returns the new state.
    pub fn toggle_plugin_enabled(&self, name: &str) -> Result<bool, String> {
        let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
        let mut list = self.read_installed_unlocked();
        let rec = list
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("plugin '{name}' is not installed"))?;
        rec.enabled = !rec.enabled;
        let next = rec.enabled;
        self.write_installed_unlocked(&list)?;
        Ok(next)
    }

    /// Uninstalls a plugin by removing it from `installed.json` and deleting its directory.
    pub fn uninstall_plugin(&self, name: &str) -> Result<(), String> {
        let _guard = self.lock.lock().map_err(|_| "plugin lock poisoned")?;
        let mut list = self.read_installed_unlocked();
        let idx = list
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("plugin '{name}' is not installed"))?;
        let removed = list.remove(idx);
        self.write_installed_unlocked(&list)?;
        if removed.install_path.exists() {
            let _ = std::fs::remove_dir_all(&removed.install_path);
        }
        Ok(())
    }

    /// Collects commands, agents, skills, and MCP servers from all enabled plugins.
    #[must_use]
    pub fn collect_enabled_components(&self) -> EnabledPluginBundle {
        let installed = self.installed_plugins();
        let mut bundle = EnabledPluginBundle::default();
        for rec in installed {
            if !rec.enabled {
                continue;
            }
            if !rec.install_path.exists() {
                bundle.errors.push(format!(
                    "{}: install path missing ({})",
                    rec.name,
                    rec.install_path.display()
                ));
                continue;
            }
            load_plugin_components(&rec, &mut bundle);
        }
        bundle
    }
}

fn run_git(args: &[&str]) -> Result<(), String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("git {:?} exited with {}", args, out.status)
        } else {
            format!("git error: {stderr}")
        });
    }
    Ok(())
}

fn parse_marketplace_dir(
    marketplace_name: &str,
    dir: &Path,
    installed: &[InstalledPluginRecord],
) -> Result<Vec<DiscoverPluginEntry>, String> {
    let manifest_path = dir.join(".claude-plugin/marketplace.json");
    if !manifest_path.exists() {
        return Err(format!(
            "marketplace '{marketplace_name}' missing .claude-plugin/marketplace.json"
        ));
    }
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed reading {}: {e}", manifest_path.display()))?;
    let parsed: RawMarketplaceManifest = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid marketplace.json in '{marketplace_name}': {e}"))?;
    let _ = parsed.name;

    Ok(parsed
        .plugins
        .into_iter()
        .map(|p| {
            let inst = installed.iter().find(|i| i.name == p.name);
            DiscoverPluginEntry {
                name: p.name,
                marketplace: marketplace_name.to_owned(),
                description: p.description,
                version: p.version,
                category: p.category,
                source: p.source,
                installed: inst.is_some(),
                enabled: inst.is_some_and(|i| i.enabled),
            }
        })
        .collect())
}

fn read_plugin_manifest(dir: &Path) -> RawPluginManifest {
    let path = dir.join(".claude-plugin/plugin.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn load_plugin_components(rec: &InstalledPluginRecord, bundle: &mut EnabledPluginBundle) {
    let root = &rec.install_path;

    // 1. commands/*.md
    let cmd_dir = root.join("commands");
    if let Ok(entries) = std::fs::read_dir(&cmd_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md")
                && let Ok(raw) = std::fs::read_to_string(&path)
            {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("command");
                let (desc, body) = parse_frontmatter_summary(&raw);
                bundle.commands.push(PluginCommand {
                    plugin: rec.name.clone(),
                    name: format!("{}:{stem}", rec.name),
                    description: if desc.is_empty() {
                        format!("Plugin command from {}", rec.name)
                    } else {
                        desc
                    },
                    template: body,
                });
            }
        }
    }

    // 2. agents/*.md
    let agent_dir = root.join("agents");
    if let Ok(entries) = std::fs::read_dir(&agent_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md")
                && let Ok(raw) = std::fs::read_to_string(&path)
            {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("agent");
                let (desc, body) = parse_frontmatter_summary(&raw);
                bundle.agents.push(PluginAgent {
                    plugin: rec.name.clone(),
                    name: format!("{}:{stem}", rec.name),
                    description: if desc.is_empty() {
                        format!("Agent from {}", rec.name)
                    } else {
                        desc
                    },
                    instructions: body,
                });
            }
        }
    }

    // 3. skills/*/SKILL.md
    let skills_dir = root.join("skills");
    if let Ok(entries) = std::fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.is_file()
                && let Ok(raw) = std::fs::read_to_string(&skill_md)
            {
                let skill_name = entry.file_name().to_string_lossy().into_owned();
                let (desc, _) = parse_frontmatter_summary(&raw);
                bundle.skills.push(PluginSkill {
                    plugin: rec.name.clone(),
                    name: format!("{}:{skill_name}", rec.name),
                    description: if desc.is_empty() {
                        format!("Skill from {}", rec.name)
                    } else {
                        desc
                    },
                    path: skill_md,
                });
            }
        }
    }

    // 4. .mcp.json
    let mcp_path = root.join(".mcp.json");
    if mcp_path.is_file() {
        match std::fs::read_to_string(&mcp_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        {
            Some(val) => {
                let servers_obj = val
                    .get("mcpServers")
                    .and_then(Value::as_object)
                    .or_else(|| val.as_object());
                if let Some(map) = servers_obj {
                    let root_str = root.to_string_lossy();
                    for (srv_name, srv_val) in map {
                        let Some(cmd) = srv_val.get("command").and_then(Value::as_str) else {
                            continue;
                        };
                        let args = srv_val
                            .get("args")
                            .and_then(Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(Value::as_str)
                                    .map(|a| a.replace("${CLAUDE_PLUGIN_ROOT}", &root_str))
                                    .collect()
                            })
                            .unwrap_or_default();
                        bundle.mcp_servers.push(McpServerConfig {
                            name: format!("{}:{srv_name}", rec.name),
                            command: cmd.replace("${CLAUDE_PLUGIN_ROOT}", &root_str),
                            args,
                        });
                    }
                }
            }
            None => {
                bundle
                    .errors
                    .push(format!("{}: invalid .mcp.json", rec.name));
            }
        }
    }
}

fn parse_frontmatter_summary(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---")
        && let Some(end_idx) = rest.find("\n---")
    {
        let fm = &rest[..end_idx];
        let body = rest[end_idx + 4..].trim().to_owned();
        let mut desc = String::new();
        for line in fm.lines() {
            if let Some(v) = line.trim().strip_prefix("description:") {
                desc = v.trim().trim_matches('"').trim_matches('\'').to_owned();
                break;
            }
        }
        return (desc, body);
    }
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_default()
        .chars()
        .take(100)
        .collect();
    (first_line, raw.to_owned())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("readdir {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let s_path = entry.path();
        let d_path = dst.join(name);
        let ft = entry
            .file_type()
            .map_err(|e| format!("filetype {}: {e}", s_path.display()))?;
        if ft.is_dir() {
            copy_dir_recursive(&s_path, &d_path)?;
        } else if ft.is_file() {
            std::fs::copy(&s_path, &d_path)
                .map_err(|e| format!("copy {}: {e}", s_path.display()))?;
        }
    }
    let _ = BTreeMap::<String, String>::new();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marketplace_and_loads_installed_plugin_components()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("rein_plugins_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mgr = PluginManager::new(root.join("plugins"));

        // Seed a local marketplace clone directly.
        let mp_dir = root.join("plugins/marketplaces/claude-plugins-official");
        std::fs::create_dir_all(mp_dir.join(".claude-plugin"))?;
        std::fs::write(
            mp_dir.join(".claude-plugin/marketplace.json"),
            r#"{
              "name": "claude-plugins-official",
              "plugins": [
                {
                  "name": "code-review",
                  "description": "Automated code review",
                  "version": "1.0.0",
                  "source": "./plugins/code-review"
                }
              ]
            }"#,
        )?;

        let src = mp_dir.join("plugins/code-review");
        std::fs::create_dir_all(src.join(".claude-plugin"))?;
        std::fs::create_dir_all(src.join("commands"))?;
        std::fs::create_dir_all(src.join("agents"))?;
        std::fs::create_dir_all(src.join("skills/lint"))?;
        std::fs::write(
            src.join(".claude-plugin/plugin.json"),
            r#"{"name":"code-review","version":"1.0.0","description":"Automated code review"}"#,
        )?;
        std::fs::write(
            src.join("commands/review.md"),
            "---\ndescription: Run review\n---\nReview the diff carefully.",
        )?;
        std::fs::write(
            src.join("agents/reviewer.md"),
            "---\ndescription: Code reviewer agent\n---\nYou review PRs.",
        )?;
        std::fs::write(
            src.join("skills/lint/SKILL.md"),
            "---\ndescription: Lint skill\n---\n# Lint",
        )?;
        std::fs::write(
            src.join(".mcp.json"),
            r#"{"mcpServers":{"srv":{"command":"node","args":["${CLAUDE_PLUGIN_ROOT}/srv.js"]}}}"#,
        )?;

        let catalog = mgr.discover_catalog(None);
        assert_eq!(catalog.len(), 1);
        assert!(!catalog[0].installed);

        let rec = mgr.install_plugin(&catalog[0]).map_err(|e| e.to_string())?;
        assert!(rec.enabled);

        let bundle = mgr.collect_enabled_components();
        assert_eq!(bundle.commands.len(), 1);
        assert_eq!(bundle.commands[0].name, "code-review:review");
        assert_eq!(bundle.agents.len(), 1);
        assert_eq!(bundle.agents[0].name, "code-review:reviewer");
        assert_eq!(bundle.skills.len(), 1);
        assert_eq!(bundle.skills[0].name, "code-review:lint");
        assert_eq!(bundle.mcp_servers.len(), 1);
        assert!(bundle.mcp_servers[0].args[0].ends_with("installed/code-review/srv.js"));

        let enabled = mgr
            .toggle_plugin_enabled("code-review")
            .map_err(|e| e.to_string())?;
        assert!(!enabled);
        assert!(mgr.collect_enabled_components().commands.is_empty());

        mgr.uninstall_plugin("code-review")
            .map_err(|e| e.to_string())?;
        assert!(mgr.installed_plugins().is_empty());
        let _ = std::fs::remove_dir_all(&root);
        Ok(())
    }
}
