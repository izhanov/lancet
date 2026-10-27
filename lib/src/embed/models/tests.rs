use std::collections::HashSet;

use super::*;

#[test]
fn matches_base_name() {
    for name in [
        "nomic-embed-text",
        "nomic-embed-text:latest",
        "nomic-embed-text:v1.5",
        "library/nomic-embed-text",
        "Nomic-Embed-Text",
    ] {
        assert_eq!(
            profile(name).map(|p| p.name),
            Some("nomic-embed-text"),
            "{name}"
        );
    }
}

#[test]
fn tag_does_not_pick_another_model() {
    assert_eq!(
        profile("snowflake-arctic-embed:s").unwrap().name,
        "snowflake-arctic-embed"
    );
    assert_eq!(
        profile("snowflake-arctic-embed2:568m").unwrap().name,
        "snowflake-arctic-embed2"
    );
}

#[test]
fn unknown_model() {
    assert_eq!(profile("hf.co/gpustack/bge-m3-GGUF:Q8_0"), None);
    assert_eq!(profile(""), None);
}

#[test]
fn prompt_by_input_type() {
    let p = profile("nomic-embed-text").unwrap();
    assert_eq!(p.prompt(InputType::Query), "search_query: ");
    assert_eq!(p.prompt(InputType::Document), "search_document: ");
    assert!(!p.is_symmetric());
    assert!(profile("bge-m3").unwrap().is_symmetric());
}

#[test]
fn names_are_unique() {
    let mut seen = HashSet::new();
    for p in PROFILES {
        assert!(seen.insert(p.name), "{} listed twice", p.name);
        assert_eq!(base_name(p.name), p.name, "{} must be a base name", p.name);
    }
}

#[test]
fn fingerprint_is_stable() {
    // A changed value here means every database built with this profile
    // stops opening: change it only together with the prompt.
    assert_eq!(
        profile("nomic-embed-text").unwrap().fingerprint(),
        "30757326"
    );
}

#[test]
fn fingerprint_tells_query_from_document() {
    let a = Profile {
        name: "a",
        query: "x",
        document: "",
    };
    let b = Profile {
        name: "a",
        query: "",
        document: "x",
    };
    assert_ne!(a.fingerprint(), b.fingerprint());
}
