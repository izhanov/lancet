//! End-to-end: the real `lt` binary on a temp database with the fake
//! embedder, so no Ollama is needed.

use std::io::Write;
use std::process::{Command, Output, Stdio};

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let env = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        env.write(
            "notes/deploy.md",
            "# Deploy\n\nbook the deploy slot on preprod\n\n## Slack\n\nping the team in slack\n",
        );
        env.write("notes/people.csv", "name,city\nAigerim,Almaty\n");
        env
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// As stored: `lt` absolutizes against its working directory, which on
    /// macOS resolves /var to /private/var.
    fn path(&self, rel: &str) -> String {
        let dir = std::fs::canonicalize(self.dir.path()).unwrap();
        dir.join(rel).to_string_lossy().into_owned()
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_lt"));
        cmd.args(args)
            .current_dir(self.dir.path())
            .env("LANCET_DB", self.dir.path().join("db.lance"))
            .env("LANCET_EVALS", self.dir.path().join("evals"))
            .env("LANCET_LANGUAGES", self.dir.path().join("languages"))
            .env("LANCET_EMBEDDER", "fake")
            .env("NO_COLOR", "1")
            .env("HOME", self.dir.path());
        cmd
    }

    fn lt(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Runs `lt mcp` through one session: the handshake, then `calls` as
    /// `tools/call` requests. Returns each call's text payload and isError,
    /// in order; closing stdin ends the server.
    fn mcp(&self, calls: &[serde_json::Value]) -> Vec<(String, bool)> {
        self.mcp_session(calls).1
    }

    /// Like `mcp`, and also returns the server's instructions.
    fn mcp_session(&self, calls: &[serde_json::Value]) -> (String, Vec<(String, bool)>) {
        let mut child = self
            .command(&["mcp"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut send = |msg: serde_json::Value| writeln!(stdin, "{msg}").unwrap();
        send(
            serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "e2e", "version": "0"}}}),
        );
        send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        for (i, params) in calls.iter().enumerate() {
            send(
                serde_json::json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call", "params": params}),
            );
        }
        drop(stdin);

        let out = stdout(&child.wait_with_output().unwrap());
        let mut instructions = String::new();
        let mut results = vec![None; calls.len()];
        for line in out.lines() {
            let msg: serde_json::Value = serde_json::from_str(line).unwrap();
            let id = msg["id"].as_u64().unwrap() as usize;
            if id == 0 {
                assert_eq!(msg["result"]["serverInfo"]["name"], "lancet", "{line}");
                instructions = msg["result"]["instructions"].as_str().unwrap().to_string();
                continue;
            }
            let result = &msg["result"];
            let text = result["content"][0]["text"].as_str().unwrap().to_string();
            results[id - 1] = Some((text, result["isError"] == true));
        }
        let results = results
            .into_iter()
            .map(|r| r.expect("a response to every call"))
            .collect();
        (instructions, results)
    }
}

fn stdout(out: &Output) -> String {
    assert!(
        out.status.success(),
        "lt failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn index_is_incremental() {
    let env = Env::new();

    let first = stdout(&env.lt(&["index", "notes"]));
    assert!(
        first.contains("notes/deploy.md — 2 chunks indexed"),
        "{first}"
    );
    assert!(
        first.ends_with("3 chunks indexed, 3 in the database\n"),
        "{first}"
    );

    let again = stdout(&env.lt(&["index", "notes"]));
    assert_eq!(again, "nothing to do: 2 files unchanged, 0 stale removed\n");

    let reindexed = stdout(&env.lt(&["--quiet", "reindex", "notes"]));
    assert_eq!(reindexed, "3 chunks indexed, 3 in the database\n");
}

#[test]
fn index_without_supported_files_fails() {
    let env = Env::new();
    env.write("pics/photo.png", "binary");

    let out = env.lt(&["index", "pics"]);

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no supported files found"));
}

#[test]
fn search_json_with_exclusions() {
    let env = Env::new();
    stdout(&env.lt(&["index", "notes"]));

    let out = stdout(&env.lt(&["search", "team -preprod", "--mode", "text", "--json"]));

    let results: serde_json::Value = serde_json::from_str(&out).unwrap();
    let results = results.as_array().unwrap();
    assert_eq!(results.len(), 1, "{out}");
    assert_eq!(results[0]["path"], env.path("notes/deploy.md"));
    assert_eq!(results[0]["section"], "Deploy > Slack");
    assert_eq!(results[0]["score"], 1.0);
}

#[test]
fn search_human_output() {
    let env = Env::new();
    stdout(&env.lt(&["index", "notes"]));

    let out = stdout(&env.lt(&["search", "book the deploy slot on preprod", "--limit", "1"]));

    assert!(out.starts_with("1  "), "{out}");
    assert!(
        out.contains(
            "notes/deploy.md\n         Deploy\n         book the deploy slot on preprod\n"
        ),
        "{out}"
    );
}

#[test]
fn search_an_empty_database_explains() {
    let env = Env::new();
    let out = env.lt(&["search", "anything"]);
    assert!(String::from_utf8_lossy(&out.stderr).contains("run `lt index <path>` first"));
}

#[test]
fn eval_reports_recall_and_mrr() {
    let env = Env::new();
    stdout(&env.lt(&["index", "notes"]));
    env.write(
        "evals/golden.yaml",
        "- query: \"book the deploy slot on preprod\"\n  expect: [\"notes/deploy.md\"]\n\
         - query: \"lunch\"\n  expect: [\"notes/missing.md\"]\n",
    );

    let out = stdout(&env.lt(&["eval"]));

    assert!(
        out.contains("1  hit@1  book the deploy slot on preprod"),
        "{out}"
    );
    assert!(
        out.contains("2  miss   lunch\n          want notes/missing.md"),
        "{out}"
    );
    assert!(
        out.contains("recall@5 0.50 (1/2)   MRR 0.500   mode=hybrid"),
        "{out}"
    );
}

#[test]
fn mcp_tools() {
    let env = Env::new();
    stdout(&env.lt(&["index", "notes"]));

    let replies = env.mcp(&[
        serde_json::json!({"name": "search_notes", "arguments": {
            "query": "team", "mode": "text", "exclude": ["preprod"]}}),
        serde_json::json!({"name": "list_files", "arguments": {}}),
        serde_json::json!({"name": "index_status", "arguments": {}}),
        serde_json::json!({"name": "search_notes", "arguments": {"query": "x", "mode": "fuzzy"}}),
    ]);

    let (found, failed) = &replies[0];
    assert!(!failed, "{found}");
    let found: serde_json::Value = serde_json::from_str(found).unwrap();
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found[0]["section"], "Deploy > Slack");

    let files: serde_json::Value = serde_json::from_str(&replies[1].0).unwrap();
    let paths: Vec<_> = files
        .as_array()
        .unwrap()
        .iter()
        .map(|f| &f["path"])
        .collect();
    assert_eq!(
        paths,
        [&env.path("notes/deploy.md"), &env.path("notes/people.csv")]
    );
    assert!(
        files[0]["modified"].as_str().unwrap().ends_with('Z'),
        "{files}"
    );

    let status: serde_json::Value = serde_json::from_str(&replies[2].0).unwrap();
    assert_eq!(
        status,
        serde_json::json!({"chunks": 3, "embedder": "fake/32"})
    );

    assert_eq!(
        replies[3],
        (
            "unknown mode \"fuzzy\" (want hybrid, vector or text)".to_string(),
            true
        )
    );
}

#[test]
fn skill_install_show_uninstall() {
    let env = Env::new();
    let skill = env.dir.path().join(".claude/skills/lancet-search/SKILL.md");

    let out = stdout(&env.lt(&["skill", "install"]));
    assert!(out.starts_with("skill installed: "), "{out}");
    let installed = std::fs::read_to_string(&skill).unwrap();
    assert!(
        installed.starts_with("---\nname: lancet-search\n"),
        "{installed}"
    );
    assert_eq!(stdout(&env.lt(&["skill", "show"])), installed);

    stdout(&env.lt(&["skill", "uninstall"]));
    assert!(!skill.parent().unwrap().exists());
    // Removing what is not there is not an error.
    stdout(&env.lt(&["skill", "uninstall"]));
}

#[test]
fn mcp_instructions() {
    let env = Env::new();
    let (instructions, _) = env.mcp_session(&[]);
    let db = env.dir.path().join("db.lance");
    assert!(
        instructions.contains(&format!("`lt --db {} index <paths>`", db.display())),
        "{instructions}"
    );
    assert!(!instructions.contains("skill"), "{instructions}");

    stdout(&env.lt(&["skill", "install"]));
    let (instructions, _) = env.mcp_session(&[]);
    assert!(
        instructions.contains("Follow the lancet-search skill."),
        "{instructions}"
    );
}

/// Installs the Ruby plugin the way `lt lang add ruby` lays it out, with
/// the pinned grammar from the lib's testdata instead of a download.
fn install_ruby(env: &Env) {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../lib/src/parse/code/testdata/ruby"
    );
    let plugin = env.dir.path().join("languages/ruby");
    std::fs::create_dir_all(&plugin).unwrap();
    for file in ["language.toml", "tags.scm", "grammar.wasm"] {
        std::fs::copy(format!("{fixture}/{file}"), plugin.join(file)).unwrap();
    }
}

#[test]
fn indexes_ruby_by_symbol_once_the_plugin_is_installed() {
    let env = Env::new();
    env.write(
        "app/invoice.rb",
        "class Invoice\n  # Sum of the line items.\n  def total\n    items.sum(&:price)\n  end\nend\n",
    );

    let without = env.lt(&["index", "app"]);
    assert!(String::from_utf8_lossy(&without.stderr).contains("no supported files found"));
    assert!(stdout(&env.lt(&["lang", "list"])).starts_with("no languages installed"));

    install_ruby(&env);
    assert_eq!(
        stdout(&env.lt(&["lang", "list"])),
        "ruby       .rb .rake .gemspec       tree-sitter/tree-sitter-ruby@v0.23.1\n"
    );
    let out = stdout(&env.lt(&["index", "app"]));
    assert!(out.contains("app/invoice.rb — 1 chunks indexed"), "{out}");

    let found = stdout(&env.lt(&["search", "line items", "--mode", "text", "--json"]));
    let found: serde_json::Value = serde_json::from_str(&found).unwrap();
    assert_eq!(found[0]["section"], "class Invoice", "{found}");

    // Without its parser a file drops out of the index like a deleted one.
    assert_eq!(
        stdout(&env.lt(&["lang", "remove", "ruby"])),
        "removed ruby\n"
    );
    assert_eq!(
        stdout(&env.lt(&["index", "app"])),
        "nothing to do: 0 files unchanged, 1 stale removed\n"
    );
}
