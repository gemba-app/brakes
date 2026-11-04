//! The scope job's shell run with `bash` against real temporary Git repositories.
//! Every case asserts the tier, the skippable list and the per-app change flags
//! exactly as a workflow would read them from `$GITHUB_OUTPUT`.
use brakes::modules::ci_baseline::scope::{
    DEFAULT_INERT_PATHS, DEFAULT_PROTECTED_PATHS, FRAGMENT, INPUTS, SKIPPABLE, SkipWhen, TIER_DOCS,
    TIER_FULL, docs_skippable, script,
};
use brakes::remote::scope_tier;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};
use tempfile::TempDir;

struct Repo {
    temp: TempDir,
    root: PathBuf,
    script: PathBuf,
}

impl Repo {
    /// A repository with one base commit holding `files` (path, content).
    fn with_base(files: &[(&str, &str)]) -> Self {
        Self::with_script(files, FRAGMENT)
    }
    /// `with_base`, running `text` as the scope script.
    fn with_script(files: &[(&str, &str)], text: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        fs::create_dir(&root).unwrap();
        let script = temp.path().join("scope.sh");
        fs::write(&script, text).unwrap();
        let repo = Self { temp, root, script };
        repo.git(&["init", "-q"]);
        repo.git(&["config", "user.email", "ci@example.invalid"]);
        repo.git(&["config", "user.name", "ci"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.commit(files, &[], "base");
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    /// Writes `files`, deletes `removed`, commits and returns the new SHA.
    fn commit(&self, files: &[(&str, &str)], removed: &[&str], message: &str) -> String {
        for (name, text) in files {
            let path = self.root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        for name in removed {
            fs::remove_file(self.root.join(name)).unwrap();
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
    fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"])
    }
    fn run(&self, env: &[(&str, &str)]) -> Run {
        let output_file = self.temp.path().join(format!(
            "output-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&output_file, "").unwrap();
        let mut command = Command::new("bash");
        command
            .arg(&self.script)
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("GITHUB_OUTPUT", &output_file);
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            output.status.success(),
            "scope.sh failed: {stdout}\n{stderr}"
        );
        let outputs = fs::read_to_string(&output_file)
            .unwrap()
            .lines()
            .map(|line| {
                let (key, value) = line.split_once('=').unwrap();
                (key.to_owned(), value.to_owned())
            })
            .collect();
        Run {
            outputs,
            stdout,
            stderr,
        }
    }
}

struct Run {
    outputs: BTreeMap<String, String>,
    stdout: String,
    stderr: String,
}

impl Run {
    fn tier(&self) -> &str {
        &self.outputs["tier"]
    }
    fn skippable(&self) -> Vec<String> {
        let value: Value = serde_json::from_str(&self.outputs["skippable"]).unwrap();
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }
    fn protected(&self) -> Vec<String> {
        let value: Value = serde_json::from_str(&self.outputs["protected"]).unwrap();
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }
    fn changed(&self, app: &str) -> &str {
        &self.outputs[&format!("changed-{app}")]
    }
}

fn pr(base: &str, head: &str) -> Vec<(&'static str, String)> {
    vec![
        ("EVENT", "pull_request".to_owned()),
        ("BASE_SHA", base.to_owned()),
        ("HEAD_SHA", head.to_owned()),
        ("INERT_PATHS", DEFAULT_INERT_PATHS.join("\n")),
        ("DOCS_SKIPPABLE", docs_skippable(1).join("\n")),
        ("PROTECTED_PATHS", DEFAULT_PROTECTED_PATHS.join("\n")),
    ]
}

fn scope(repo: &Repo, env: Vec<(&str, String)>) -> Run {
    let borrowed: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    repo.run(&borrowed)
}

const BASE: &[(&str, &str)] = &[
    ("README.md", "# Base\n"),
    ("docs/guide.md", "guide\n"),
    ("docs/old.md", "old\n"),
    ("tasks/plan.md", "plan\n"),
    ("src/main.rs", "fn main() {}\n"),
    ("src/old.rs", "pub fn old() {}\n"),
    (".github/workflows/ci.yml", "name: CI\n"),
];

#[test]
fn docs_only_pull_request_is_tier_docs_with_the_skippable_list() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            ("README.md", "# Changed\n"),
            ("docs/guide.md", "changed\n"),
            ("docs/deep/nested/page.md", "new\n"),
            ("tasks/plan.md", "changed\n"),
            ("src/NOTES.md", "a markdown file anywhere\n"),
        ],
        &["docs/old.md"],
        "docs",
    );
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(run.skippable(), ["checks", "workflow-lint", "macos"]);
    assert!(run.stdout.contains("tier=docs\n"), "{}", run.stdout);
    assert!(
        run.stdout
            .contains("skippable=[\"checks\",\"workflow-lint\",\"macos\"]\n"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_mixed_change_is_tier_full_and_keeps_workflow_lint_skippable() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            ("docs/guide.md", "changed\n"),
            ("src/main.rs", "fn main() { run() }\n"),
        ],
        &[],
        "mixed",
    );
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_deleted_source_file_is_tier_full() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[], &["src/old.rs"], "delete");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_workflow_change_is_tier_full_and_workflow_lint_is_not_skippable() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/ci.yml", "name: CI2\n")], &[], "wf");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["macos"]);
}

#[test]
fn a_markdown_file_under_workflows_is_never_inert() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/README.md", "docs\n")], &[], "wf-md");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["macos"]);
}

#[test]
fn any_github_path_keeps_workflow_lint_running_even_under_docs_tier() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/PULL_REQUEST_TEMPLATE.md", "x\n")], &[], "gh-md");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(run.skippable(), ["checks", "macos"]);
}

#[test]
fn a_rename_into_an_inert_path_is_tier_full() {
    // With renames detected `--name-only` would list only `docs/main.md`, which
    // is inert; the script disables renames so the deleted source file counts.
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    repo.git(&["mv", "src/main.rs", "docs/main.md"]);
    let head = repo.commit(&[], &[], "rename");
    assert!(
        repo.git(&["diff", "--name-only", "-M", &base, &head])
            .lines()
            .count()
            == 1,
        "the fixture must be a detectable rename"
    );
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_rename_out_of_an_inert_path_is_tier_full() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    repo.git(&["mv", "docs/guide.md", "src/guide.txt"]);
    let head = repo.commit(&[], &[], "rename-out");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
}

#[test]
/// No verification job is skippable; only the push-only `macos`, which never
/// runs on a pull request (#784).
fn an_empty_diff_is_tier_full_with_only_macos_skippable() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[], &[], "empty");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["macos"]);
    assert_eq!(run.outputs["skippable"], r#"["macos"]"#);
}

#[test]
/// No verification job is skippable; only the push-only `macos` (#784).
fn an_unreadable_sha_is_tier_full_with_only_macos_skippable() {
    let repo = Repo::with_base(BASE);
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    for base in ["0000000000000000000000000000000000000000", "not-a-sha", ""] {
        let run = scope(&repo, pr(base, &head));
        assert_eq!(run.tier(), TIER_FULL, "base {base:?}");
        assert_eq!(run.outputs["skippable"], r#"["macos"]"#, "base {base:?}");
    }
    let run = scope(&repo, pr(&head, ""));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.outputs["skippable"], r#"["macos"]"#);
}

#[test]
fn a_push_is_tier_full_and_macos_is_not_skippable() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let mut env = pr(&base, &head);
    env[0] = ("EVENT", "push".to_owned());
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.outputs["skippable"], "[]");
}

#[test]
fn any_other_event_is_tier_full_and_only_macos_is_skippable() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    for event in ["workflow_dispatch", "merge_group", ""] {
        let mut env = pr(&base, &head);
        env[0] = ("EVENT", event.to_owned());
        let run = scope(&repo, env);
        assert_eq!(run.tier(), TIER_FULL, "event {event:?}");
        assert_eq!(run.skippable(), ["macos"], "event {event:?}");
    }
}

#[test]
fn sharded_ids_and_contributed_jobs_are_skippable_under_docs_only() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let docs = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let full = repo.commit(&[("src/main.rs", "fn main() { run() }\n")], &[], "src");
    let mut ids = docs_skippable(3);
    ids.push("deps-scan".to_owned());
    assert_eq!(ids[..2], ["lint", "shard"]);
    let mut env = pr(&base, &docs);
    env[4] = ("DOCS_SKIPPABLE", ids.join("\n"));
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(
        run.skippable(),
        ["lint", "shard", "deps-scan", "workflow-lint", "macos"]
    );
    let mut env = pr(&docs, &full);
    env[4] = ("DOCS_SKIPPABLE", ids.join("\n"));
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_negated_inert_path_is_never_evaluated_and_means_full() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let mut env = pr(&base, &head);
    env[3] = ("INERT_PATHS", "docs/**\n!docs/api/**\n".to_owned());
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
    assert!(run.stdout.contains("::warning::"), "{}", run.stdout);
    assert!(run.stdout.contains("!docs/api/**"), "{}", run.stdout);
}

#[test]
fn an_empty_inert_list_means_full() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let mut env = pr(&base, &head);
    env[3] = ("INERT_PATHS", String::new());
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
}

const MONOREPO: &[(&str, &str)] = &[
    ("README.md", "# Mono\n"),
    ("apps/backend/src/app.py", "print(1)\n"),
    ("apps/backend/README.md", "backend\n"),
    ("apps/web/src/index.ts", "export {}\n"),
    ("shared/lib.py", "x = 1\n"),
];
const APPS: &str = "backend=apps/backend\nweb=apps/web/\n";

fn monorepo_pr(base: &str, head: &str) -> Vec<(&'static str, String)> {
    let mut env = pr(base, head);
    env.push(("APPS", APPS.to_owned()));
    env
}

#[test]
fn an_app_change_marks_only_that_app_and_skips_the_others_checks() {
    let repo = Repo::with_base(MONOREPO);
    let base = repo.head();
    let head = repo.commit(&[("apps/backend/src/app.py", "print(2)\n")], &[], "backend");
    let run = scope(&repo, monorepo_pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.changed("backend"), "true");
    assert_eq!(run.changed("web"), "false");
    assert_eq!(run.skippable(), ["workflow-lint", "macos", "web-checks"]);
    assert!(
        run.stdout.contains("changed-backend=true\n"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("changed-web=false\n"), "{}", run.stdout);
}

#[test]
fn a_path_outside_every_app_runs_every_app() {
    let repo = Repo::with_base(MONOREPO);
    let base = repo.head();
    let head = repo.commit(&[("shared/lib.py", "x = 2\n")], &[], "shared");
    let run = scope(&repo, monorepo_pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.changed("backend"), "true");
    assert_eq!(run.changed("web"), "true");
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn an_app_prefix_does_not_match_a_sibling_directory() {
    let repo = Repo::with_base(MONOREPO);
    let base = repo.head();
    let head = repo.commit(&[("apps/backend-tools/x.py", "x\n")], &[], "sibling");
    let run = scope(&repo, monorepo_pr(&base, &head));
    assert_eq!(run.changed("backend"), "true");
    assert_eq!(run.changed("web"), "true");
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_docs_only_change_inside_one_app_keeps_the_tier_and_the_app_flags() {
    let repo = Repo::with_base(MONOREPO);
    let base = repo.head();
    let head = repo.commit(&[("apps/backend/README.md", "changed\n")], &[], "app-docs");
    let run = scope(&repo, monorepo_pr(&base, &head));
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(run.changed("backend"), "true");
    assert_eq!(run.changed("web"), "false");
    assert_eq!(
        run.skippable(),
        ["checks", "workflow-lint", "macos", "web-checks"]
    );
}

#[test]
fn an_unlisted_change_set_runs_every_app() {
    let repo = Repo::with_base(MONOREPO);
    let head = repo.commit(&[("apps/web/src/index.ts", "export {}\n\n")], &[], "web");
    // A pull request whose files cannot be listed skips nothing but the
    // push-only macOS job, which never runs on it (#784); a push skips nothing.
    for (env, skippable) in [
        (monorepo_pr("not-a-sha", &head), r#"["macos"]"#),
        (
            {
                let mut env = monorepo_pr(&head, &head);
                env[0] = ("EVENT", "push".to_owned());
                env
            },
            "[]",
        ),
    ] {
        let run = scope(&repo, env);
        assert_eq!(run.tier(), TIER_FULL);
        assert_eq!(run.changed("backend"), "true");
        assert_eq!(run.changed("web"), "true");
        assert_eq!(run.outputs["skippable"], skippable);
    }
}

#[test]
fn the_printed_claim_is_what_the_doctor_parses() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let docs = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let full = repo.commit(&[("src/main.rs", "fn main() { run() }\n")], &[], "src");
    for (env, expected) in [(pr(&base, &docs), TIER_DOCS), (pr(&docs, &full), TIER_FULL)] {
        let run = scope(&repo, env);
        let claim = run
            .stdout
            .lines()
            .find(|line| line.starts_with("tier="))
            .unwrap();
        let jobs = json!([{"name": "scope", "steps": [{"name": claim}]}]);
        assert_eq!(
            scope_tier(jobs.as_array().unwrap()).as_deref(),
            Some(expected)
        );
        assert_eq!(run.tier(), expected);
        assert!(run.stderr.is_empty(), "{}", run.stderr);
    }
}

#[test]
fn the_fragment_is_hardened_shell_and_names_the_same_inputs_and_jobs_as_the_table() {
    assert!(FRAGMENT.starts_with("#!/usr/bin/env bash\n"));
    assert!(FRAGMENT.contains("set -euo pipefail\n"));
    for forbidden in ["|| true", "grep -v", "continue-on-error", "dorny"] {
        assert!(
            !FRAGMENT.contains(forbidden),
            "fragment contains {forbidden:?}"
        );
    }
    assert!(FRAGMENT.contains("diff.renames=false"));
    for (name, _) in INPUTS {
        assert!(
            FRAGMENT.contains(&format!("${{{name}")),
            "fragment never reads ${name}"
        );
    }
    let docs_ids: Vec<&str> = SKIPPABLE
        .iter()
        .filter(|(_, when)| *when == SkipWhen::TierDocs)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        docs_ids,
        ["checks", "lint", "shard", "heldout", "heldout-properties"]
    );
    for (id, when) in SKIPPABLE {
        match when {
            SkipWhen::TierDocs => {}
            SkipWhen::AppUnchanged => assert!(FRAGMENT.contains("-checks")),
            _ => assert!(FRAGMENT.contains(id), "fragment never names {id}"),
        }
    }
    assert_eq!(docs_skippable(0), ["checks"]);
    assert_eq!(docs_skippable(1), ["checks"]);
    assert_eq!(docs_skippable(2), ["lint", "shard"]);
    assert_eq!(DEFAULT_INERT_PATHS, ["docs/**", "tasks/**", "**/*.md"]);
}

// ---- T14 (#943): the `protected` output ----

#[test]
fn the_default_protected_paths_follow_the_answer_table() {
    assert_eq!(
        DEFAULT_PROTECTED_PATHS,
        [
            ".github/workflows/**",
            ".github/rulesets/**",
            ".github/CODEOWNERS",
            ".agent-setup/project.json",
            "tests/heldout/**",
        ]
    );
}

#[test]
/// Decision 2026-11-03 "Only a critical change waits for a person": the brakes
/// lock is regenerated by the tool and merges on green.
fn the_lock_is_not_protected() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".agent-setup/lock.json", "{}\n")], &[], "lock");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.protected(), Vec::<String>::new());
}

#[test]
/// Decision 2026-11-03: any workflow may produce a required check or publish, so
/// a hand edit to any of them, whatever its name or extension, waits for a person.
fn a_hand_edit_to_any_workflow_is_protected() {
    for path in [
        ".github/workflows/test.yml",
        ".github/workflows/build.yaml",
        ".github/workflows/Release.yml",
        ".github/workflows/nightly.yml",
        ".github/workflows/nested/x.yml",
    ] {
        let repo = Repo::with_base(BASE);
        let base = repo.head();
        let head = repo.commit(&[(path, "changed\n")], &[], "one");
        let run = scope(&repo, pr(&base, &head));
        assert_eq!(run.protected(), [path], "{path}");
    }
}

// ---- Decision 2026-11-03 point 6: a verified action bump is routine ----

const OLD_PIN: &str = "1111111111111111111111111111111111111111";

/// A stand-in for the code host: one repository per action, served to `git
/// ls-remote https://github.com/<owner>/<repo>` through `url.<root>.insteadOf`.
struct Host {
    _temp: TempDir,
    root: PathBuf,
}

impl Host {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_path_buf();
        Self { _temp: temp, root }
    }
    fn git(dir: &PathBuf, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
    /// Publishes `action` with `tag` on a fresh commit and returns that commit.
    fn tag(&self, action: &str, tag: &str, annotated: bool) -> String {
        let dir = self.root.join(action);
        if !dir.exists() {
            fs::create_dir_all(&dir).unwrap();
            Self::git(&dir, &["init", "-q"]);
            Self::git(&dir, &["config", "user.email", "ci@example.invalid"]);
            Self::git(&dir, &["config", "user.name", "ci"]);
            Self::git(&dir, &["config", "commit.gpgsign", "false"]);
            Self::git(&dir, &["config", "tag.gpgsign", "false"]);
        }
        Self::git(&dir, &["commit", "-q", "--allow-empty", "-m", tag]);
        if annotated {
            Self::git(&dir, &["tag", "-a", tag, "-m", tag]);
        } else {
            Self::git(&dir, &["tag", tag]);
        }
        Self::git(&dir, &["rev-parse", "HEAD"])
    }
    /// The pull request environment with the code host redirected here.
    fn pr(&self, base: &str, head: &str) -> Vec<(&'static str, String)> {
        let mut env = pr(base, head);
        env.extend([
            ("GIT_CONFIG_COUNT", "1".to_owned()),
            (
                "GIT_CONFIG_KEY_0",
                format!("url.{}/.insteadOf", self.root.display()),
            ),
            ("GIT_CONFIG_VALUE_0", "https://github.com/".to_owned()),
            ("REPOSITORY_OWNER", "example-owner".to_owned()),
        ]);
        env
    }
}

fn workflow(pins: &[(&str, &str, &str)]) -> String {
    let mut text = String::from("name: CI\non: push\njobs:\n  a:\n    runs-on: x\n    steps:\n");
    for (action, sha, tag) in pins {
        text.push_str(&format!("      - uses: {action}@{sha} # {tag}\n"));
    }
    text
}

fn bump(after: &str) -> (Repo, String, String) {
    let before = workflow(&[
        ("acme/checkout", OLD_PIN, "v1.0.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let repo = Repo::with_base(&[(".github/workflows/ci.yml", &before)]);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/ci.yml", after)], &[], "bump");
    (repo, base, head)
}

#[test]
fn a_workflow_whose_pins_move_to_the_commits_of_their_tags_is_not_protected() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let scan = host.tag("acme/scan", "v2.1.0", true);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.1.0"),
        ("acme/scan/init", &scan, "v2.1.0"),
    ]);
    let (repo, base, head) = bump(&after);
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), Vec::<String>::new(), "{}", run.stderr);
    assert_eq!(run.tier(), TIER_FULL);
}

#[test]
fn a_pin_that_is_not_the_commit_of_its_tag_is_protected() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let _later = host.tag("acme/checkout", "v1.2.0", false);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.2.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let (repo, base, head) = bump(&after);
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
fn a_pin_bump_that_changes_the_action_is_protected() {
    let host = Host::new();
    let other = host.tag("evil/checkout", "v1.1.0", false);
    let after = workflow(&[
        ("evil/checkout", &other, "v1.1.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let (repo, base, head) = bump(&after);
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
fn a_pin_bump_beside_any_other_edit_is_protected() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.1.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ])
    .replace("runs-on: x", "runs-on: y");
    let (repo, base, head) = bump(&after);
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
fn a_pin_bump_that_cannot_be_verified_is_protected() {
    let host = Host::new();
    let after = workflow(&[
        (
            "acme/checkout",
            "2222222222222222222222222222222222222222",
            "v1.1.0",
        ),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let (repo, base, head) = bump(&after);
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

/// The bumped workflow, each case's head text with the stand-in host.
fn bumped(host: &Host, after: &str) -> Run {
    let (repo, base, head) = bump(after);
    scope(&repo, host.pr(&base, &head))
}

#[test]
/// An owner can create any tag on its own actions: such a pin is never routine.
fn a_pin_of_an_action_of_the_same_owner_is_protected() {
    let host = Host::new();
    let own = host.tag("example-owner/shared", "v9.0.0", false);
    let before = workflow(&[("example-owner/shared", OLD_PIN, "v1.0.0")]);
    let after = workflow(&[("example-owner/shared", &own, "v9.0.0")]);
    let repo = Repo::with_base(&[(".github/workflows/ci.yml", &before)]);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/ci.yml", &after)], &[], "own");
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
fn without_the_repository_owner_no_bump_is_routine() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.1.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let (repo, base, head) = bump(&after);
    let env: Vec<_> = host
        .pr(&base, &head)
        .into_iter()
        .filter(|(k, _)| *k != "REPOSITORY_OWNER")
        .collect();
    let run = scope(&repo, env);
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
/// A pin moved back to an older release may reintroduce a fixed flaw.
fn a_pin_moved_to_an_older_or_equal_release_is_protected() {
    for tag in ["v0.9.0", "v1.0.0", "v0.10"] {
        let host = Host::new();
        let older = host.tag("acme/checkout", tag, false);
        let after = workflow(&[
            ("acme/checkout", &older, tag),
            ("acme/scan/init", OLD_PIN, "v2.0.0"),
        ]);
        assert_eq!(
            bumped(&host, &after).protected(),
            [".github/workflows/ci.yml"],
            "{tag}"
        );
    }
}

#[test]
fn a_release_compares_by_number_not_by_text() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.10.0", false);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.10.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    let run = bumped(&host, &after);
    assert_eq!(run.protected(), Vec::<String>::new(), "{}", run.stderr);
}

#[test]
fn a_pin_to_a_tag_that_is_not_a_release_is_protected() {
    let host = Host::new();
    let latest = host.tag("acme/checkout", "latest", false);
    let after = workflow(&[
        ("acme/checkout", &latest, "latest"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ]);
    assert_eq!(
        bumped(&host, &after).protected(),
        [".github/workflows/ci.yml"]
    );
}

#[test]
/// YAML reads only a space before `#` as the start of a comment.
fn a_pin_line_with_any_other_blank_is_protected() {
    for blank in ["\u{c}", "\t", "\u{3000}"] {
        let host = Host::new();
        let checkout = host.tag("acme/checkout", "v1.1.0", false);
        let after = workflow(&[
            ("acme/checkout", &checkout, "v1.1.0"),
            ("acme/scan/init", OLD_PIN, "v2.0.0"),
        ])
        .replace(&format!("{checkout} #"), &format!("{checkout}{blank}#"));
        assert_eq!(
            bumped(&host, &after).protected(),
            [".github/workflows/ci.yml"],
            "{blank:?}"
        );
    }
}

#[test]
fn a_pin_line_ending_in_a_carriage_return_is_protected() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let after = workflow(&[
        ("acme/checkout", &checkout, "v1.1.0"),
        ("acme/scan/init", OLD_PIN, "v2.0.0"),
    ])
    .replace("# v1.1.0\n", "# v1.1.0\r\n");
    assert_eq!(
        bumped(&host, &after).protected(),
        [".github/workflows/ci.yml"]
    );
}

#[test]
fn a_new_workflow_made_only_of_pins_is_protected() {
    let host = Host::new();
    let checkout = host.tag("acme/checkout", "v1.1.0", false);
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let text = workflow(&[("acme/checkout", &checkout, "v1.1.0")]);
    let head = repo.commit(&[(".github/workflows/new.yml", &text)], &[], "new");
    let run = scope(&repo, host.pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/new.yml"]);
}

#[test]
fn every_protected_change_is_listed_in_diff_order() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            (".github/workflows/ci.yml", "name: CI2\n"),
            (".github/workflows/nested/x.yml", "name: X\n"),
            (".github/rulesets/main.json", "{}\n"),
            (".github/CODEOWNERS", "* @owner\n"),
            (".agent-setup/lock.json", "{}\n"),
            (".agent-setup/project.json", "{}\n"),
            ("tests/heldout/test_gate.py", "assert True\n"),
            ("src/main.rs", "fn main() { run() }\n"),
            (".github/dependabot.yml", "version: 2\n"),
            ("docs/guide.md", "changed\n"),
        ],
        &[],
        "protected",
    );
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(
        run.protected(),
        [
            ".agent-setup/project.json",
            ".github/CODEOWNERS",
            ".github/rulesets/main.json",
            ".github/workflows/ci.yml",
            ".github/workflows/nested/x.yml",
            "tests/heldout/test_gate.py",
        ]
    );
    assert_eq!(run.tier(), TIER_FULL);
    assert!(
        run.stdout.contains(
            "protected=[\".agent-setup/project.json\",\".github/CODEOWNERS\",\".github/rulesets/main.json\",\".github/workflows/ci.yml\",\".github/workflows/nested/x.yml\",\"tests/heldout/test_gate.py\"]\n"
        ),
        "{}",
        run.stdout
    );
}

#[test]
fn each_protected_kind_is_listed_alone() {
    for path in [
        ".github/workflows/ci.yml",
        ".github/rulesets/main.json",
        ".github/CODEOWNERS",
        ".agent-setup/project.json",
        "tests/heldout/test_gate.py",
    ] {
        let repo = Repo::with_base(BASE);
        let base = repo.head();
        let head = repo.commit(&[(path, "changed\n")], &[], "one");
        let run = scope(&repo, pr(&base, &head));
        assert_eq!(run.protected(), [path], "{path}");
        assert_eq!(run.tier(), TIER_FULL, "{path}");
    }
}

#[test]
fn a_deleted_protected_file_is_listed() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[], &[".github/workflows/ci.yml"], "delete");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
}

#[test]
/// #1280: the base branch changes protected paths and source after the pull request
/// branched; only what the pull request itself changed counts, so its docs-only
/// change stays tier `docs` with nothing protected.
fn changes_on_the_base_after_the_branch_point_are_not_the_pull_requests() {
    let repo = Repo::with_base(BASE);
    let branch_point = repo.head();
    repo.git(&["checkout", "-q", "-b", "topic"]);
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    repo.git(&["checkout", "-q", &branch_point]);
    let base = repo.commit(
        &[
            (".agent-setup/lock.json", "{}\n"),
            (".github/rulesets/main.json", "{}\n"),
            (".github/workflows/ci.yml", "name: CI2\n"),
            ("src/main.rs", "fn main() { run() }\n"),
        ],
        &[],
        "main moves",
    );
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.protected(), Vec::<String>::new());
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(run.skippable(), ["checks", "workflow-lint", "macos"]);
}

#[test]
/// #1280: a protected change the pull request makes is listed even when the base
/// moved since the branch point, and the base's own protected changes are not.
fn a_protected_change_of_the_pull_request_is_listed_when_the_base_moved() {
    let repo = Repo::with_base(BASE);
    let branch_point = repo.head();
    repo.git(&["checkout", "-q", "-b", "topic"]);
    let head = repo.commit(&[(".github/CODEOWNERS", "* @owner\n")], &[], "owners");
    repo.git(&["checkout", "-q", &branch_point]);
    let base = repo.commit(&[(".agent-setup/lock.json", "{}\n")], &[], "main moves");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.protected(), [".github/CODEOWNERS"]);
    assert_eq!(run.tier(), TIER_FULL);
}

#[test]
/// #1280: without a merge base (unrelated histories, or a checkout too shallow to hold
/// it) the files cannot be listed, so the gate fails closed.
fn no_merge_base_means_the_files_cannot_be_listed() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    repo.git(&["checkout", "-q", "--orphan", "unrelated"]);
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "orphan");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.protected(), ["<unknown>"]);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.outputs["skippable"], r#"["macos"]"#);
}

#[test]
fn a_docs_only_change_under_workflows_is_full_and_protected() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/README.md", "docs\n")], &[], "wf-md");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.protected(), [".github/workflows/README.md"]);
}

#[test]
fn a_markdown_file_in_the_heldout_directory_is_never_inert() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("tests/heldout/README.md", "how\n")], &[], "heldout-md");
    let run = scope(&repo, pr(&base, &head));
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.protected(), ["tests/heldout/README.md"]);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn an_answered_protected_glob_is_never_inert_either() {
    // A protected change is never tier docs, whatever the two answers say.
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[("docs/security/policy.md", "x\n")], &[], "policy");
    let mut env = pr(&base, &head);
    env[5] = ("PROTECTED_PATHS", "docs/security/**\n".to_owned());
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(run.protected(), ["docs/security/policy.md"]);
}

#[test]
fn a_change_without_protected_paths_lists_none() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let docs = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    let full = repo.commit(&[("src/main.rs", "fn main() { run() }\n")], &[], "src");
    for (from, to) in [(&base, &docs), (&docs, &full)] {
        let run = scope(&repo, pr(from, to));
        assert_eq!(run.outputs["protected"], "[]");
    }
    let empty = repo.commit(&[], &[], "empty");
    let run = scope(&repo, pr(&full, &empty));
    assert_eq!(run.outputs["protected"], "[]");
}

#[test]
fn a_pull_request_whose_files_cannot_be_listed_is_unknown() {
    let repo = Repo::with_base(BASE);
    let head = repo.commit(&[("docs/guide.md", "changed\n")], &[], "docs");
    for base in ["0000000000000000000000000000000000000000", "not-a-sha", ""] {
        let run = scope(&repo, pr(base, &head));
        assert_eq!(run.outputs["protected"], "[\"<unknown>\"]", "base {base:?}");
    }
    let run = scope(&repo, pr(&head, ""));
    assert_eq!(run.outputs["protected"], "[\"<unknown>\"]");
}

#[test]
fn outside_pull_requests_nothing_is_listed() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/ci.yml", "name: CI2\n")], &[], "wf");
    for event in ["push", "workflow_dispatch", "merge_group", ""] {
        let mut env = pr(&base, &head);
        env[0] = ("EVENT", event.to_owned());
        let run = scope(&repo, env);
        assert_eq!(run.outputs["protected"], "[]", "event {event:?}");
    }
}

#[test]
fn a_negated_protected_glob_is_never_evaluated_and_protects_nothing_less() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(&[(".github/workflows/ci.yml", "name: CI2\n")], &[], "wf");
    let mut env = pr(&base, &head);
    env[5] = (
        "PROTECTED_PATHS",
        ".github/workflows/**\n!.github/workflows/ci.yml\n".to_owned(),
    );
    let run = scope(&repo, env);
    assert_eq!(run.protected(), [".github/workflows/ci.yml"]);
    assert!(run.stdout.contains("::warning::"), "{}", run.stdout);
    assert!(
        run.stdout.contains("!.github/workflows/ci.yml"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_double_star_prefix_glob_matches_at_any_depth() {
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            ("Cargo.lock", "x\n"),
            ("apps/web/Cargo.lock", "y\n"),
            ("src/lib.rs", "z\n"),
        ],
        &[],
        "locks",
    );
    let mut env = pr(&base, &head);
    env[5] = ("PROTECTED_PATHS", "**/Cargo.lock\n".to_owned());
    let run = scope(&repo, env);
    assert_eq!(run.protected(), ["Cargo.lock", "apps/web/Cargo.lock"]);
}

#[test]
fn an_apps_checks_skippable_under_docs_and_unchanged_is_listed_once() {
    let repo = Repo::with_base(MONOREPO);
    let base = repo.head();
    let head = repo.commit(&[("apps/backend/README.md", "changed\n")], &[], "app-docs");
    let mut env = monorepo_pr(&base, &head);
    env.retain(|(k, _)| *k != "DOCS_SKIPPABLE");
    env.push(("DOCS_SKIPPABLE", "backend-checks\nweb-checks".to_owned()));
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_DOCS);
    assert_eq!(
        run.skippable(),
        ["backend-checks", "web-checks", "workflow-lint", "macos"]
    );
}

// ---- qa T8 (#1005): screenshot baselines are protected at any depth ----

#[test]
fn a_seeded_snapshot_is_protected_directly_under_web_dir_and_nested() {
    use brakes::modules::qa::protected::protected_glob;
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            (
                "qa/web/smoke.spec.ts-snapshots/home-chromium-linux.png",
                "a",
            ),
            ("qa/web/sub/x.spec.ts-snapshots/a.png", "b"),
            ("qa/web/smoke.spec.ts", "c"),
            ("qa/other-snapshots/x.png", "d"),
        ],
        &[],
        "baselines",
    );
    let mut env = pr(&base, &head);
    env[5] = (
        "PROTECTED_PATHS",
        format!(
            "{}\n{}",
            DEFAULT_PROTECTED_PATHS.join("\n"),
            protected_glob("qa/web")
        ),
    );
    let run = scope(&repo, env);
    assert_eq!(run.tier(), TIER_FULL);
    assert_eq!(
        run.protected(),
        [
            "qa/web/smoke.spec.ts-snapshots/home-chromium-linux.png",
            "qa/web/sub/x.spec.ts-snapshots/a.png",
        ]
    );
}

#[test]
fn a_project_snapshot_is_protected_at_the_root_and_at_any_depth() {
    use brakes::modules::qa::protected::protected_glob;
    let repo = Repo::with_base(BASE);
    let base = repo.head();
    let head = repo.commit(
        &[
            ("e2e/home.spec.ts-snapshots/home-chromium-linux.png", "a"),
            ("home.spec.ts-snapshots/b.png", "b"),
            ("src/snapshots/c.png", "c"),
        ],
        &[],
        "baselines",
    );
    let mut env = pr(&base, &head);
    env[5] = ("PROTECTED_PATHS", protected_glob("."));
    let run = scope(&repo, env);
    assert_eq!(
        run.protected(),
        [
            "e2e/home.spec.ts-snapshots/home-chromium-linux.png",
            "home.spec.ts-snapshots/b.png",
        ]
    );
}

// --- contributed app jobs (#1097) --------------------------------------------------------

#[test]
fn without_app_jobs_the_script_is_the_fragment() {
    assert_eq!(script(false), FRAGMENT);
    assert!(script(true).contains("APP_JOBS"));
    assert_eq!(script(true).lines().count(), FRAGMENT.lines().count() + 9);
}

#[test]
fn a_contributed_job_of_an_unchanged_app_is_skippable_with_its_app() {
    let repo = Repo::with_script(MONOREPO, &script(true));
    let base = repo.head();
    let head = repo.commit(&[("apps/backend/src/app.py", "print(2)\n")], &[], "backend");
    let mut env = monorepo_pr(&base, &head);
    env.push((
        "APP_JOBS",
        "backend-qa-api=backend\nweb-qa-api=web\nweb-qa-web=web\n".to_owned(),
    ));
    let run = scope(&repo, env);
    assert_eq!(run.changed("web"), "false");
    assert_eq!(
        run.skippable(),
        [
            "workflow-lint",
            "macos",
            "web-checks",
            "web-qa-api",
            "web-qa-web"
        ]
    );
}

#[test]
fn a_contributed_app_job_runs_when_a_path_outside_every_app_changed() {
    let repo = Repo::with_script(MONOREPO, &script(true));
    let base = repo.head();
    let head = repo.commit(&[("shared/lib.py", "x = 2\n")], &[], "shared");
    let mut env = monorepo_pr(&base, &head);
    env.push(("APP_JOBS", "web-qa-api=web\n".to_owned()));
    let run = scope(&repo, env);
    assert_eq!(run.skippable(), ["workflow-lint", "macos"]);
}

#[test]
fn a_docs_only_change_lists_a_docs_skippable_app_job_once() {
    let repo = Repo::with_script(MONOREPO, &script(true));
    let base = repo.head();
    let head = repo.commit(&[("apps/backend/README.md", "changed\n")], &[], "app-docs");
    let mut env = monorepo_pr(&base, &head);
    env.retain(|(k, _)| *k != "DOCS_SKIPPABLE");
    env.push(("DOCS_SKIPPABLE", "checks\nweb-qa-api\n".to_owned()));
    env.push(("APP_JOBS", "web-qa-api=web\n".to_owned()));
    let run = scope(&repo, env);
    assert_eq!(
        run.skippable(),
        [
            "checks",
            "web-qa-api",
            "workflow-lint",
            "macos",
            "web-checks"
        ]
    );
}
