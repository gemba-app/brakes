//! `actor-check.sh` runs in the `cross-review-model` job under `bash`: it lets the
//! allowed bots past, and any other actor only when it is a person with write or
//! admin access. A `gh` stand-in on `PATH` answers the two reads; nothing reaches a
//! real host. Needs bash.
mod common;

use std::{fs, process::Command};

const CHECK: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/agent-guardrails/actor-check.sh"
);

/// Runs the check as `actor` with `allowed` and a `gh` that reports `kind` for
/// `users/<actor>` and `permission` for the collaborator read.
fn check(actor: &str, allowed: &str, kind: &str, permission: &str) -> (bool, String) {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin/gh");
    fs::create_dir_all(bin.parent().unwrap()).unwrap();
    common::write_executable(
        &bin,
        &format!(
            "#!/usr/bin/env bash\ncase \"$2\" in\n  users/*) echo {kind} ;;\n  *) echo {permission} ;;\nesac\n"
        ),
    );
    let path = format!(
        "{}:{}",
        bin.parent().unwrap().display(),
        std::env::var("PATH").unwrap()
    );
    let mut command = Command::new("bash");
    command.arg(CHECK);
    for (name, value) in [
        ("PATH", path.as_str()),
        ("ACTOR", actor),
        ("ALLOWED_BOT", allowed),
        ("GH_REPO", "example-owner/example"),
    ] {
        command.env(name, value);
    }
    let output = command.output().unwrap();
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn every_bot_of_the_comma_separated_list_passes() {
    let list = "dependabot[bot],example-agents[bot]";
    for actor in ["dependabot[bot]", "example-agents[bot]", "Example-Agents"] {
        let (ok, out) = check(actor, list, "Bot", "none");
        assert!(ok, "{actor}: {out}");
        assert!(out.contains("is the allowed bot"), "{actor}: {out}");
    }
}

#[test]
fn another_bot_is_refused_even_when_a_list_is_set() {
    let (ok, out) = check(
        "other-app[bot]",
        "dependabot[bot],example-agents[bot]",
        "Bot",
        "none",
    );
    assert!(!ok);
    assert!(out.contains("is a Bot, not a person"), "{out}");
}

#[test]
fn a_person_with_write_access_still_passes_and_one_without_is_refused() {
    let list = "dependabot[bot],example-agents[bot]";
    assert!(check("someone", list, "User", "write").0);
    let (ok, out) = check("someone", list, "User", "read");
    assert!(!ok);
    assert!(out.contains("has read access"), "{out}");
}

#[test]
fn an_empty_list_lets_no_bot_past() {
    let (ok, _) = check("dependabot[bot]", "", "Bot", "none");
    assert!(!ok);
}
