//! Language plugins: what makes source code indexable. A plugin is a
//! directory, so adding a language needs no new `lt` build:
//!
//! ```text
//! <languages dir>/ruby/
//!   language.toml   name (the grammar's own name), file extensions, source
//!   grammar.wasm    the tree-sitter grammar compiled to WebAssembly
//!   tags.scm        the grammar's tree-sitter tags query
//! ```
//!
//! `lt` knows no languages itself. Tree-sitter grammars publish all three
//! pieces: `tree-sitter.json` (name and file types) and `queries/tags.scm`
//! in the repository, the `.wasm` in their releases — so [`add`] installs
//! any of them straight from GitHub. The tags query marks definitions
//! (`@definition.method`, `@definition.class`, ...), their names (`@name`)
//! and doc comments (`@doc`): exactly what chunking code by symbol needs,
//! so a plugin carries no rules of its own. A hand-made directory works
//! the same way, for a grammar that publishes no `.wasm`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter::wasmtime::Engine;
use tree_sitter::{Query, WasmStore};

use crate::{Error, Result};

pub const MANIFEST: &str = "language.toml";
pub const GRAMMAR: &str = "grammar.wasm";
pub const TAGS: &str = "tags.scm";

/// One engine per process: it compiles grammar modules, and every parser's
/// [`WasmStore`] must come from the same engine as the languages it runs.
pub(crate) static ENGINE: LazyLock<Engine> = LazyLock::new(Engine::default);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Manifest {
    /// The grammar's name as compiled into the module (`tree_sitter_<name>`).
    pub name: String,
    /// Lowercase, with the dot: ".rb".
    pub extensions: Vec<String>,
    /// Where `lt lang add` got it: "tree-sitter/tree-sitter-ruby@v0.23.1".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grammar_sha256: Option<String>,
}

/// A loaded plugin: the grammar ready to parse and the compiled tags query.
pub struct Language {
    pub manifest: Manifest,
    pub(crate) grammar: tree_sitter::Language,
    pub(crate) tags: Query,
    /// Owns the grammar's compiled module.
    _store: WasmStore,
}

impl Language {
    /// Loads the plugin in `dir`. Patterns of the tags query that mark no
    /// definition (`@reference.call` and the like) are switched off: they
    /// match nearly every identifier and chunking does not need them.
    pub fn load(dir: &Path) -> Result<Self> {
        let failed = |message: String| Error::Language {
            path: dir.display().to_string(),
            message,
        };
        let manifest = read_manifest(dir)?;
        let wasm = std::fs::read(dir.join(GRAMMAR)).map_err(|e| Error::Io {
            path: dir.join(GRAMMAR).display().to_string(),
            source: e,
        })?;

        let mut store = WasmStore::new(&ENGINE).map_err(|e| failed(e.message))?;
        let grammar = store
            .load_language(&manifest.name, &wasm)
            .map_err(|e| failed(format!("{GRAMMAR}: {}", e.message)))?;
        let mut tags = Query::new(&grammar, &read(&dir.join(TAGS))?)
            .map_err(|e| failed(format!("{TAGS}: {e}")))?;
        let definitions: Vec<usize> = tags
            .capture_names()
            .iter()
            .enumerate()
            .filter(|(_, name)| name.starts_with("definition."))
            .map(|(i, _)| i)
            .collect();
        if definitions.is_empty() {
            return Err(failed(format!("{TAGS} has no @definition.* captures")));
        }
        for pattern in 0..tags.pattern_count() {
            let quantifiers = tags.capture_quantifiers(pattern);
            if !definitions
                .iter()
                .any(|&i| quantifiers[i] != tree_sitter::CaptureQuantifier::Zero)
            {
                tags.disable_pattern(pattern);
            }
        }

        Ok(Self {
            manifest,
            grammar,
            tags,
            _store: store,
        })
    }
}

/// Every plugin installed in `dir`. A missing directory means no plugins;
/// a broken plugin is an error rather than a silently unindexed language.
pub fn load_all(dir: &Path) -> Result<Vec<Language>> {
    plugin_dirs(dir)?
        .iter()
        .map(|d| Language::load(d))
        .collect()
}

/// Manifests of the installed plugins, without loading their grammars.
pub fn installed(dir: &Path) -> Result<Vec<Manifest>> {
    plugin_dirs(dir)?.iter().map(|d| read_manifest(d)).collect()
}

/// Removes an installed plugin; `false` when there was none.
pub fn remove(dir: &Path, name: &str) -> Result<bool> {
    // A name is one path component: "../notes" must not reach outside.
    if !valid_name(name) {
        return Ok(false);
    }
    let target = dir.join(name);
    if !target.join(MANIFEST).is_file() {
        return Ok(false);
    }
    std::fs::remove_dir_all(&target).map_err(|e| Error::Io {
        path: target.display().to_string(),
        source: e,
    })?;
    Ok(true)
}

/// Where grammars are fetched from. Plain web URLs, not the GitHub API:
/// no token, no anonymous rate limit.
pub struct GitHub {
    web: String,
    raw: String,
    client: reqwest::Client,
}

impl Default for GitHub {
    fn default() -> Self {
        Self::with_base("https://github.com", "https://raw.githubusercontent.com")
    }
}

impl GitHub {
    pub fn with_base(web: &str, raw: &str) -> Self {
        Self {
            web: web.trim_end_matches('/').to_string(),
            raw: raw.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// The body, or `None` on a 404.
    async fn get(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let failed = |message: String| Error::Download {
            url: url.to_string(),
            message,
        };
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| failed(e.to_string()))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp.error_for_status().map_err(|e| failed(e.to_string()))?;
        let body = resp.bytes().await.map_err(|e| failed(e.to_string()))?;
        Ok(Some(body.to_vec()))
    }

    /// The latest release's tag: `/releases/latest` redirects to it.
    async fn latest_tag(&self, repo: &str) -> Result<Option<String>> {
        let url = format!("{}/{repo}/releases/latest", self.web);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Download {
                url: url.clone(),
                message: e.to_string(),
            })?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        Ok(resp
            .url()
            .path()
            .rsplit_once("/releases/tag/")
            .map(|(_, tag)| tag.to_string()))
    }
}

/// `tree-sitter.json`, the part of it a plugin needs.
#[derive(Deserialize)]
struct GrammarConfig {
    grammars: Vec<GrammarEntry>,
}

#[derive(Deserialize)]
struct GrammarEntry {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, rename = "file-types")]
    file_types: Vec<String>,
    #[serde(default)]
    tags: Option<OneOrMany>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

/// Installs the grammars of one repository into `dir`, replacing previous
/// installs, and returns what was installed. `spec` is a short name
/// ("python": `tree-sitter/tree-sitter-python`, then the community's
/// `tree-sitter-grammars/tree-sitter-python`) or `owner/repo`, either with
/// an optional `@tag`, the latest release by default. `extra_extensions`
/// are claimed on top of the grammar's own file types. Each plugin is
/// loaded once before it counts as installed, so a grammar this `lt`
/// cannot run fails here, not in the middle of indexing.
pub async fn add(
    dir: &Path,
    spec: &str,
    extra_extensions: &[String],
    gh: &GitHub,
) -> Result<Vec<Manifest>> {
    let failed = |message: String| Error::Language {
        path: spec.to_string(),
        message,
    };
    let (name, tag) = match spec.split_once('@') {
        Some((name, tag)) => (name, Some(tag.to_string())),
        None => (spec, None),
    };
    let repos = if name.contains('/') {
        vec![name.to_string()]
    } else {
        vec![
            format!("tree-sitter/tree-sitter-{name}"),
            format!("tree-sitter-grammars/tree-sitter-{name}"),
        ]
    };

    let mut found = None;
    for repo in &repos {
        let tag = match &tag {
            Some(tag) => tag.clone(),
            None => match gh.latest_tag(repo).await? {
                Some(tag) => tag,
                None => continue,
            },
        };
        let url = format!("{}/{repo}/{tag}/tree-sitter.json", gh.raw);
        if let Some(config) = gh.get(&url).await? {
            let config: GrammarConfig =
                serde_json::from_slice(&config).map_err(|e| failed(format!("{url}: {e}")))?;
            found = Some((repo.clone(), tag, config));
            break;
        }
    }
    let (repo, tag, config) = found.ok_or_else(|| {
        failed(format!(
            "no release with a tree-sitter.json in {}",
            repos.join(" or ")
        ))
    })?;

    let mut installed = Vec::new();
    for grammar in &config.grammars {
        if !valid_name(&grammar.name) {
            return Err(failed(format!("bad grammar name {:?}", grammar.name)));
        }
        let base = grammar.path.as_deref().unwrap_or(".").trim_matches('/');
        let tags_files: Vec<String> = match &grammar.tags {
            Some(OneOrMany::One(f)) => vec![f.clone()],
            Some(OneOrMany::Many(fs)) => fs.clone(),
            None if base == "." || base.is_empty() => vec!["queries/tags.scm".into()],
            None => vec![format!("{base}/queries/tags.scm")],
        };
        let mut tags = String::new();
        for file in &tags_files {
            let file = file.trim_start_matches("./");
            let url = format!("{}/{repo}/{tag}/{file}", gh.raw);
            let body = gh.get(&url).await?.ok_or_else(|| {
                failed(format!(
                    "{} has no tags query ({file}); write one into <languages>/{}/{TAGS}",
                    grammar.name, grammar.name
                ))
            })?;
            tags.push_str(&String::from_utf8_lossy(&body));
            tags.push('\n');
        }
        let url = format!(
            "{}/{repo}/releases/download/{tag}/tree-sitter-{}.wasm",
            gh.web, grammar.name
        );
        let wasm = gh.get(&url).await?.ok_or_else(|| {
            failed(format!(
                "release {tag} has no tree-sitter-{}.wasm",
                grammar.name
            ))
        })?;

        let mut extensions: Vec<String> = Vec::new();
        for ext in grammar.file_types.iter().chain(extra_extensions) {
            let ext = format!(".{}", ext.trim_start_matches('.').to_lowercase());
            if !extensions.contains(&ext) {
                extensions.push(ext);
            }
        }
        let manifest = Manifest {
            name: grammar.name.clone(),
            extensions,
            source: Some(format!("{repo}@{tag}")),
            grammar_sha256: Some(format!("{:x}", Sha256::digest(&wasm))),
        };
        install(dir, &manifest, &tags, &wasm)?;
        installed.push(manifest);
    }
    Ok(installed)
}

/// Writes the plugin next to its target and swaps it in whole, once it
/// loads: a failed install leaves the previous one intact.
fn install(dir: &Path, manifest: &Manifest, tags: &str, wasm: &[u8]) -> Result<()> {
    let io = |path: &Path| {
        let path = path.display().to_string();
        move |source| Error::Io { path, source }
    };
    let target = dir.join(&manifest.name);
    let staging = dir.join(format!(".{}.installing", manifest.name));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(io(&staging))?;
    let toml = toml::to_string(manifest).expect("a manifest serializes");
    for (file, bytes) in [
        (MANIFEST, toml.as_bytes()),
        (TAGS, tags.as_bytes()),
        (GRAMMAR, wasm),
    ] {
        std::fs::write(staging.join(file), bytes).map_err(io(&staging.join(file)))?;
    }
    if let Err(err) = Language::load(&staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(io(&target))
}

fn plugin_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(Error::Io {
                path: dir.display().to_string(),
                source: e,
            });
        }
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(MANIFEST).is_file())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn read_manifest(dir: &Path) -> Result<Manifest> {
    toml::from_str(&read(&dir.join(MANIFEST))?).map_err(|e| Error::Language {
        path: dir.display().to_string(),
        message: format!("{MANIFEST}: {e}"),
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).map_err(|e| Error::Io {
        path: path.display().to_string(),
        source: e,
    })
}

/// The Ruby plugin fixture from testdata, loaded once: compiling a grammar
/// module takes a while.
#[cfg(test)]
pub(crate) fn ruby_for_tests() -> &'static Language {
    static RUBY: LazyLock<Language> =
        LazyLock::new(|| Language::load(&ruby_fixture_dir()).unwrap());
    &RUBY
}

#[cfg(test)]
pub(crate) fn ruby_fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/parse/code/testdata/ruby")
}

#[cfg(test)]
mod tests;
