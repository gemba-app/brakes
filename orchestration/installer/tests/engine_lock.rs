//! Lock schema 2 and the migration from the schema-1 lock written today (SPEC-engine.md, #650).
mod support;
use brakes::engine::lock::{Lock, Ownership};
use brakes::{HOOKS, config::current_owner};
use serde_json::{Value, json};

fn known(path: &str) -> bool {
    path == "AGENTS.md"
        || path == "CLAUDE.md"
        || path == "docs/agent-setup/decisions-index.md"
        || support::guides()
            .iter()
            .any(|(n, _)| format!("docs/agent-setup/{n}") == path)
        || HOOKS.iter().any(|(n, _)| *n == path)
}

fn v1(files: Value) -> Value {
    json!({
        "schema": 1, "version": "0.3.0", "source_commit": "abc", "source_dirty": false,
        "personal_owner": current_owner(), "files": files
    })
}

#[test]
fn from_v1_refuses_unknown_paths_with_migration_required() {
    let error = Lock::from_v1(&v1(json!({"docs/other.md": "x"})), "1.0.0", &known)
        .unwrap_err()
        .to_string();
    assert_eq!(error, "Unknown managed paths in lock; migration required");
}

#[test]
fn from_v1_refuses_a_lock_without_a_files_object() {
    let error = Lock::from_v1(&json!({"schema": 1, "files": []}), "1.0.0", &known)
        .unwrap_err()
        .to_string();
    assert_eq!(error, "Invalid installed lock");
}

#[test]
fn parse_migrates_schema_1_and_accepts_schema_2() {
    let migrated =
        Lock::parse(&v1(json!({"AGENTS.md": "d1"})).to_string(), "1.0.0", &known).unwrap();
    let serialized = migrated.to_json_string().unwrap();
    let reparsed = Lock::parse(&serialized, "1.0.0", &known).unwrap();
    assert_eq!(reparsed, migrated);
    assert_eq!(reparsed.to_json_string().unwrap(), serialized);
    assert!(serialized.ends_with('\n'));
}

#[test]
fn parse_refuses_unsupported_schemas() {
    for raw in [
        json!({"schema": 3, "files": {}}).to_string(),
        json!({"files": {}}).to_string(),
    ] {
        let error = Lock::parse(&raw, "1.0.0", &known).unwrap_err().to_string();
        assert_eq!(
            error, "Unsupported lock schema; migration required",
            "{raw}"
        );
    }
}

#[test]
fn seed_and_remote_entries_serialize_without_a_digest_and_keys_are_sorted() {
    let mut lock = Lock::from_v1(&v1(json!({"AGENTS.md": "d1"})), "1.0.0", &known).unwrap();
    lock.record("delivery", "1.2.0", Some("a9"));
    lock.insert_file("compose.yaml", "delivery", Ownership::Seed, None);
    lock.insert_file("rulesets/main", "repo-baseline", Ownership::Remote, None);
    lock.insert_file("a.md", "delivery", Ownership::Managed, Some("zz"));
    let text = lock.to_json_string().unwrap();
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["files"]["compose.yaml"],
        json!({"module": "delivery", "class": "seed"})
    );
    assert_eq!(
        value["files"]["rulesets/main"],
        json!({"module": "repo-baseline", "class": "remote"})
    );
    assert_eq!(value["files"]["a.md"]["digest"], "zz");
    assert_eq!(
        value["modules"]["delivery"],
        json!({"version": "1.2.0", "answers_digest": "a9"})
    );
    let keys: Vec<_> = value["files"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert!(text.find("\"modules\"").unwrap() < text.find("\"files\"").unwrap());
}
