use std::fs;

use super::*;

fn result(path: &str) -> SearchResult {
    SearchResult {
        path: path.into(),
        section: String::new(),
        text: String::new(),
        ordinal: 0,
        score: 0.0,
    }
}

#[test]
fn loads_a_golden_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden.yaml");
    fs::write(
        &path,
        "# comment\n- query: \"как задеплоить\"\n  expect: [\"skills/deploy/SKILL.md\"]\n",
    )
    .unwrap();

    assert_eq!(
        load_golden(&path).unwrap(),
        [Case {
            query: "как задеплоить".into(),
            expect: vec!["skills/deploy/SKILL.md".into()]
        }]
    );
}

#[test]
fn rejects_empty_and_incomplete_sets() {
    let dir = tempfile::tempdir().unwrap();
    for (content, want) in [
        ("# only comments\n", "no cases"),
        (
            "- query: q\n  expect: []\n",
            "case 1 needs both query and expect",
        ),
        ("- query: [broken", ""),
    ] {
        let path = dir.path().join("golden.yaml");
        fs::write(&path, content).unwrap();
        let err = load_golden(&path).unwrap_err().to_string();
        assert!(err.contains(want), "{content:?}: {err}");
    }
}

#[test]
fn golden_files_are_sorted_yaml_only() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["b.yml", "a.yaml", "notes.txt"] {
        fs::write(dir.path().join(name), "").unwrap();
    }

    let files = golden_files(dir.path()).unwrap();

    assert_eq!(files, [dir.path().join("a.yaml"), dir.path().join("b.yml")]);
    assert!(golden_files(&dir.path().join("missing")).is_err());
}

#[test]
fn first_expected_rank_matches_suffixes() {
    let results = [
        result("/w/a.md"),
        result("/w/skills/kit-pr/SKILL.md"),
        result("/w/kit-pr/SKILL.md"),
    ];

    assert_eq!(
        first_expected_rank(&results, &["kit-pr/SKILL.md".into()]),
        Some((2, "/w/skills/kit-pr/SKILL.md"))
    );
    assert_eq!(first_expected_rank(&results, &["missing.md".into()]), None);
}

#[test]
fn score_recall_and_mrr() {
    let mut score = Score::default();
    for rank in [Some(1), Some(2), None, Some(4)] {
        score.add(rank);
    }
    assert_eq!((score.cases, score.hits), (4, 3));
    assert_eq!(score.recall(), 0.75);
    assert_eq!(score.mrr(), (1.0 + 0.5 + 0.25) / 4.0);

    let mut total = Score::default();
    total.merge(score);
    total.merge(score);
    assert_eq!((total.cases, total.mrr()), (8, score.mrr()));
    assert_eq!(Score::default().recall(), 0.0);
}
