//! Manage `<workspace>/.cursor/mcp.json` around a `cursor-agent` spawn.
//!
//! `cursor-agent` has no `--mcp-config` flag; it discovers MCP servers only from
//! `.cursor/mcp.json` in its workspace (or `~/.cursor/mcp.json`). To point a
//! spawned agent at the omnia-hosted MCP servers a prompt granted, the backend
//! merges the granted server entries into the workspace file before the spawn
//! and restores the prior state afterwards.
//!
//! Completions can run concurrently against the same workspace and may grant
//! overlapping or disjoint server sets, so a process-wide registry keyed by
//! workspace tracks per-server refcounts: each server is written once (on its
//! first guard) and removed once (when its last guard drops); the file is
//! restored to its captured original once no omnia servers remain. Server URLs
//! are deployment-stable, so the written content is identical regardless of
//! ordering.

use std::collections::{BTreeMap, HashMap, hash_map};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};

use anyhow::{Context as _, Result};
use serde_json::{Map, Value, json};

static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Workspace>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// Per-workspace MCP state: the file content before omnia first touched it, plus
// the omnia servers currently installed, each with its own refcount.
struct Workspace {
    original: Option<Vec<u8>>,
    servers: HashMap<String, ServerState>,
}

struct ServerState {
    refcount: usize,
    url: String,
}

/// Restores `<workspace>/.cursor/mcp.json` when the last guard for each installed
/// server drops.
pub struct McpGuard {
    workspace: PathBuf,
    path: PathBuf,
    names: Vec<String>,
}

impl McpGuard {
    /// Merge each `name -> url` server into `<workspace>/.cursor/mcp.json`,
    /// refcounting per server so concurrent completions granting overlapping or
    /// disjoint sets merge correctly and unwind cleanly.
    pub fn install(workspace: &Path, servers: &BTreeMap<String, String>) -> Result<Self> {
        let workspace = workspace.to_path_buf();
        let path = workspace.join(".cursor").join("mcp.json");
        let names: Vec<String> = servers.keys().cloned().collect();

        let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = install_into(&mut registry, &workspace, &path, servers) {
            unwind(&mut registry, &workspace, &names);
            return Err(error);
        }

        Ok(Self {
            workspace,
            path,
            names,
        })
    }
}

// Record `servers` in the workspace's registry entry and rewrite mcp.json.
// Refcounts are incremented only on the write path, so `unwind` after any
// failure decrements exactly what was counted.
fn install_into(
    registry: &mut HashMap<PathBuf, Workspace>, workspace: &Path, path: &Path,
    servers: &BTreeMap<String, String>,
) -> Result<()> {
    let state = match registry.entry(workspace.to_path_buf()) {
        hash_map::Entry::Occupied(occupied) => occupied.into_mut(),
        hash_map::Entry::Vacant(vacant) => {
            let original = match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(error).with_context(|| format!("reading {}", path.display()));
                }
            };
            vacant.insert(Workspace {
                original,
                servers: HashMap::new(),
            })
        }
    };

    for (name, url) in servers {
        let server = state.servers.entry(name.clone()).or_insert_with(|| ServerState {
            refcount: 0,
            url: url.clone(),
        });
        debug_assert_eq!(
            server.url, *url,
            "MCP server URLs are deployment-stable; a same-name grant with a \
             different URL would be silently ignored"
        );
        server.refcount += 1;
    }

    write_mcp_json(path, state)
}

// Decrement `names` in the workspace's entry, dropping the entry when empty.
fn unwind(registry: &mut HashMap<PathBuf, Workspace>, workspace: &Path, names: &[String]) {
    let Some(state) = registry.get_mut(workspace) else {
        return;
    };
    decrement_servers(&mut state.servers, names);
    if state.servers.is_empty() {
        registry.remove(workspace);
    }
}

fn write_mcp_json(path: &Path, workspace: &Workspace) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let merged = merge(workspace.original.as_deref(), &workspace.servers)?;
    fs::write(path, merged).with_context(|| format!("writing {}", path.display()))
}

fn decrement_servers(servers: &mut HashMap<String, ServerState>, names: &[String]) {
    for name in names {
        let Some(state) = servers.get_mut(name) else {
            continue;
        };
        state.refcount -= 1;
        if state.refcount == 0 {
            servers.remove(name);
        }
    }
}

impl Drop for McpGuard {
    // The registry lock is deliberately held across the mcp.json write so that
    // concurrent guards' rewrites of the shared file serialize.
    #[allow(clippy::significant_drop_tightening)]
    fn drop(&mut self) {
        let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(state) = registry.get_mut(&self.workspace) else {
            return;
        };
        decrement_servers(&mut state.servers, &self.names);

        // A re-merge is computed while `state` is borrowed; `None` means the
        // last guard dropped and the entry is removed below.
        let merged = if state.servers.is_empty() {
            None
        } else {
            Some(merge(state.original.as_deref(), &state.servers))
        };

        let restore = match merged {
            None => {
                let state = registry.remove(&self.workspace).expect("the entry was just seen");
                match state.original {
                    Some(bytes) => fs::write(&self.path, bytes),
                    None => match fs::remove_file(&self.path) {
                        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                        other => other,
                    },
                }
            }
            Some(Ok(bytes)) => fs::write(&self.path, bytes),
            Some(Err(error)) => {
                tracing::warn!(path = %self.path.display(), %error, "failed to re-merge mcp.json");
                return;
            }
        };
        if let Err(error) = restore {
            tracing::warn!(path = %self.path.display(), %error, "failed to restore mcp.json");
        }
    }
}

// Merge the omnia servers into `original`, preserving any user-defined servers.
fn merge(original: Option<&[u8]>, servers: &HashMap<String, ServerState>) -> Result<Vec<u8>> {
    let mut root = match original {
        Some(bytes) => serde_json::from_slice::<Value>(bytes)
            .context("existing .cursor/mcp.json is not valid JSON")?,
        None => json!({}),
    };

    let object = root.as_object_mut().context("existing .cursor/mcp.json is not a JSON object")?;
    let entries = object.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let entries =
        entries.as_object_mut().context("`mcpServers` in .cursor/mcp.json is not an object")?;
    for (name, state) in servers {
        entries.insert(name.clone(), json!({ "url": state.url }));
    }

    let mut bytes = serde_json::to_vec_pretty(&root).context("serializing .cursor/mcp.json")?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::McpGuard;

    /// A fresh, empty temp directory unique to this process and `label`.
    fn temp_workspace(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omnia-cursor-mcp-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creating temp workspace");
        dir
    }

    fn servers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(name, url)| ((*name).to_owned(), (*url).to_owned())).collect()
    }

    fn read_servers(path: &Path) -> Value {
        let bytes = fs::read(path).expect("reading mcp.json");
        let value: Value = serde_json::from_slice(&bytes).expect("mcp.json is JSON");
        value["mcpServers"].clone()
    }

    #[test]
    fn create_remove_absent() {
        let workspace = temp_workspace("absent");
        let path = workspace.join(".cursor/mcp.json");
        let guard =
            McpGuard::install(&workspace, &servers(&[("docs", "http://127.0.0.1:8080/mcp/docs")]))
                .unwrap();
        assert_eq!(read_servers(&path)["docs"]["url"], "http://127.0.0.1:8080/mcp/docs");
        drop(guard);
        assert!(!path.exists(), "a file we created is removed on drop");
    }

    #[test]
    fn merge_restore_existing() {
        let workspace = temp_workspace("existing");
        let cursor_dir = workspace.join(".cursor");
        fs::create_dir_all(&cursor_dir).unwrap();
        let path = cursor_dir.join("mcp.json");
        let original = json!({ "mcpServers": { "other": { "url": "http://example" } } });
        fs::write(&path, serde_json::to_vec_pretty(&original).unwrap()).unwrap();

        let guard =
            McpGuard::install(&workspace, &servers(&[("docs", "http://127.0.0.1:9/x")])).unwrap();
        let entries = read_servers(&path);
        assert_eq!(entries["docs"]["url"], "http://127.0.0.1:9/x");
        assert_eq!(entries["other"]["url"], "http://example", "existing servers survive");
        drop(guard);

        let restored: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored, original, "the original file is restored verbatim");
    }

    #[test]
    fn refcount_last_guard() {
        let workspace = temp_workspace("refcount");
        let path = workspace.join(".cursor/mcp.json");
        let map = servers(&[("docs", "http://127.0.0.1:8080/mcp/docs")]);
        let first = McpGuard::install(&workspace, &map).unwrap();
        let second = McpGuard::install(&workspace, &map).unwrap();

        drop(first);
        assert!(path.exists(), "the file survives while a guard is still held");
        assert_eq!(read_servers(&path)["docs"]["url"], "http://127.0.0.1:8080/mcp/docs");

        drop(second);
        assert!(!path.exists(), "the file is removed once the last guard drops");
    }

    #[test]
    fn install_failure_does_not_leak_refcount() {
        let workspace = temp_workspace("install-fail");
        let path = workspace.join(".cursor/mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{ not json").unwrap();

        assert!(
            McpGuard::install(&workspace, &servers(&[("docs", "http://127.0.0.1:8080/mcp/docs")]))
                .is_err(),
            "invalid existing mcp.json is rejected"
        );
        fs::remove_file(&path).unwrap();

        let guard =
            McpGuard::install(&workspace, &servers(&[("docs", "http://127.0.0.1:8080/mcp/docs")]))
                .unwrap();
        assert_eq!(read_servers(&path)["docs"]["url"], "http://127.0.0.1:8080/mcp/docs");
        drop(guard);
        assert!(!path.exists(), "a leaked refcount would leave the file after drop");
    }

    #[test]
    fn disjoint_grants() {
        let workspace = temp_workspace("disjoint");
        let path = workspace.join(".cursor/mcp.json");
        let docs = McpGuard::install(&workspace, &servers(&[("docs", "http://d/mcp")])).unwrap();
        let wiki = McpGuard::install(&workspace, &servers(&[("wiki", "http://w/mcp")])).unwrap();

        let entries = read_servers(&path);
        assert_eq!(entries["docs"]["url"], "http://d/mcp");
        assert_eq!(entries["wiki"]["url"], "http://w/mcp", "disjoint grants coexist");

        drop(docs);
        let entries = read_servers(&path);
        assert!(entries.get("docs").is_none(), "a dropped server is removed");
        assert_eq!(entries["wiki"]["url"], "http://w/mcp", "the other server survives");

        drop(wiki);
        assert!(!path.exists(), "the file is removed once the last server drops");
    }
}
