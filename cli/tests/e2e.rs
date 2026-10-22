//! End-to-end: the real `lt` binary on a temp database with the fake
//! embedder, so no Ollama is needed.

use std::process::{Command, Output};

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

    fn lt(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lt"))
            .args(args)
            .current_dir(self.dir.path())
            .env("LANCET_DB", self.dir.path().join("db.lance"))
            .env("LANCET_EVALS", self.dir.path().join("evals"))
            .env("LANCET_EMBEDDER", "fake")
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
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
