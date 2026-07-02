//! Manage `<workspace>/.cursor/mcp.json` around a `cursor-agent` spawn.
//!
//! `cursor-agent` has no `--mcp-config` flag; it discovers MCP servers only from
//! `.cursor/mcp.json` in its workspace (or `~/.cursor/mcp.json`). To point a
//! spawned agent at the omnia-hosted MCP server, the backend merges a server
//! entry into the workspace file before the spawn and restores the prior state
//! afterwards.
//!
//! Completions can run concurrently against the same workspace, so a
//! process-wide, ref-counted registry keyed by workspace path writes the file
//! once (on the first guard) and restores it once (when the last guard drops).
//! The URL is deployment-stable, so the written content is identical regardless
//! of ordering.

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};

/// Ensure `path` exists and return its canonical form for stable registry keys.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or canonicalized.
pub fn prepare_workspace(path: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating workspace {}", path.display()))?;
    path.canonicalize().with_context(|| format!("canonicalizing workspace {}", path.display()))
}

/// Per-workspace state protecting `.cursor/mcp.json` while guards are live.
struct Entry {
    /// Number of live guards for this workspace.
    refcount: usize,
    /// Original file bytes to restore, or `None` if there was no file.
    original: Option<Vec<u8>>,
}

/// Ref-counted registry of workspaces whose `mcp.json` the backend has patched.
static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// An RAII guard that keeps `<workspace>/.cursor/mcp.json` advertising the omnia
/// MCP server while held, restoring the prior state when the last guard for the
/// workspace drops.
pub struct McpConfigGuard {
    /// Canonical workspace path; the registry key.
    workspace: PathBuf,
    /// The `.cursor/mcp.json` path under `workspace`.
    path: PathBuf,
}

impl McpConfigGuard {
    /// Merge the omnia MCP server at `url` into `<workspace>/.cursor/mcp.json`.
    ///
    /// # Errors
    ///
    /// Returns an error if `.cursor/mcp.json` exists but is not a JSON object,
    /// or if the `.cursor` directory or file cannot be read or written.
    // The registry lock is intentionally held across the file I/O so a
    // concurrent install against the same workspace cannot race on the file.
    #[allow(clippy::significant_drop_tightening)]
    pub fn install(workspace: &Path, url: &str) -> Result<Self> {
        let workspace = prepare_workspace(workspace)?;
        let path = workspace.join(".cursor").join("mcp.json");

        let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(entry) = registry.get_mut(&workspace) {
            entry.refcount += 1;
            return Ok(Self { workspace, path });
        }

        let original = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let merged = merge(original.as_deref(), url)?;
        fs::write(&path, merged).with_context(|| format!("writing {}", path.display()))?;

        registry.insert(
            workspace.clone(),
            Entry {
                refcount: 1,
                original,
            },
        );
        Ok(Self { workspace, path })
    }
}

impl Drop for McpConfigGuard {
    // The lock is held across the restore so a concurrent install cannot observe
    // a half-restored file.
    #[allow(clippy::significant_drop_tightening)]
    fn drop(&mut self) {
        let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);

        let remaining = match registry.get_mut(&self.workspace) {
            Some(entry) => {
                entry.refcount -= 1;
                entry.refcount
            }
            None => return,
        };
        if remaining > 0 {
            return;
        }

        let Some(entry) = registry.remove(&self.workspace) else {
            return;
        };
        let restore = match entry.original {
            Some(bytes) => fs::write(&self.path, bytes),
            None => match fs::remove_file(&self.path) {
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(error) = restore {
            tracing::warn!(path = %self.path.display(), %error, "failed to restore mcp.json");
        }
    }
}

/// Merge the omnia server entry into existing `mcp.json` bytes, or into a fresh
/// document when there was no file.
fn merge(original: Option<&[u8]>, url: &str) -> Result<Vec<u8>> {
    let mut root = match original {
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .context("existing .cursor/mcp.json is not valid JSON")?,
        None => json!({}),
    };

    let root = root.as_object_mut().context("existing .cursor/mcp.json is not a JSON object")?;
    let servers = root.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let servers =
        servers.as_object_mut().context("`mcpServers` in .cursor/mcp.json is not an object")?;
    servers.insert("omnia".to_owned(), json!({ "url": url }));

    let mut bytes = serde_json::to_vec_pretty(&Value::Object(root.clone()))
        .context("serializing .cursor/mcp.json")?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::{McpConfigGuard};

    /// A fresh, empty temp directory unique to this process and `label`.
    fn temp_workspace(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omnia-cursor-mcp-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating temp workspace");
        dir
    }

    fn read_servers(path: &Path) -> Value {
        let bytes = fs::read(path).expect("reading mcp.json");
        let value: Value = serde_json::from_slice(&bytes).expect("mcp.json is JSON");
        value["mcpServers"].clone()
    }

    #[test]
    fn creates_and_removes_when_absent() {
        let workspace = temp_workspace("absent");
        let path = workspace.join(".cursor/mcp.json");
        let guard = McpConfigGuard::install(&workspace, "http://127.0.0.1:8080/mcp/docs").unwrap();
        assert_eq!(read_servers(&path)["omnia"]["url"], "http://127.0.0.1:8080/mcp/docs");
        drop(guard);
        assert!(!path.exists(), "a file we created is removed on drop");
    }

    #[test]
    fn merges_and_restores_existing() {
        let workspace = temp_workspace("existing");
        let cursor_dir = workspace.join(".cursor");
        fs::create_dir_all(&cursor_dir).unwrap();
        let path = cursor_dir.join("mcp.json");
        let original = json!({ "mcpServers": { "other": { "url": "http://example" } } });
        fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();

        let guard = McpConfigGuard::install(&workspace, "http://127.0.0.1:9/x").unwrap();
        let servers = read_servers(&path);
        assert_eq!(servers["omnia"]["url"], "http://127.0.0.1:9/x");
        assert_eq!(servers["other"]["url"], "http://example", "existing servers survive");
        drop(guard);

        let restored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored, original, "the original file is restored verbatim");
    }

    #[test]
    fn refcount_keeps_file_until_last_guard_drops() {
        let workspace = temp_workspace("refcount");
        let path = workspace.join(".cursor/mcp.json");
        let first = McpConfigGuard::install(&workspace, "http://127.0.0.1:8080/mcp/docs").unwrap();
        let second = McpConfigGuard::install(&workspace, "http://127.0.0.1:8080/mcp/docs").unwrap();

        drop(first);
        assert!(path.exists(), "the file survives while a guard is still held");
        assert_eq!(read_servers(&path)["omnia"]["url"], "http://127.0.0.1:8080/mcp/docs");

        drop(second);
        assert!(!path.exists(), "the file is removed once the last guard drops");
    }
}
