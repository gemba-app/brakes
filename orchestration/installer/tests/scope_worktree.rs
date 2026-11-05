//! #59: a linked worktree that sits inside its own main checkout is the same repository,
//! not a nested one; a different repository inside another is still refused.
use brakes::personal_scope;
use std::{fs, path::Path, process::Command};

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@example.invalid", "-c", "user.name=t"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// A repository with one commit at `<temp>/main`.
fn main_checkout(temp: &Path) -> std::path::PathBuf {
    let main = temp.join("main");
    fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q"]);
    fs::write(main.join("README.md"), "# project\n").unwrap();
    git(&main, &["add", "README.md"]);
    git(&main, &["commit", "-qm", "one"]);
    main
}

#[test]
fn a_linked_worktree_inside_its_main_checkout_is_not_a_nested_repository() {
    let temp = tempfile::tempdir().unwrap();
    let main = main_checkout(temp.path());
    let inside = main.join(".claude/worktrees/task");
    fs::create_dir_all(inside.parent().unwrap()).unwrap();
    git(&main, &["worktree", "add", "-q", inside.to_str().unwrap()]);
    assert_eq!(
        personal_scope(&inside, true).unwrap(),
        inside.canonicalize().unwrap()
    );
    // A linked worktree outside the main checkout was always accepted.
    let outside = temp.path().join("outside");
    git(&main, &["worktree", "add", "-q", outside.to_str().unwrap()]);
    assert!(personal_scope(&outside, true).is_ok());
}

#[test]
fn a_different_repository_inside_another_is_still_refused() {
    let temp = tempfile::tempdir().unwrap();
    let main = main_checkout(temp.path());
    let child = main.join("child");
    fs::create_dir(&child).unwrap();
    git(&child, &["init", "-q"]);
    let refusal = personal_scope(&child, true).unwrap_err().to_string();
    assert!(
        refusal.starts_with("Nested repository refused"),
        "{refusal}"
    );
    // A worktree of the nested repository is not made the main checkout's by that.
    git(&child, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let inner = child.join("wt");
    git(&child, &["worktree", "add", "-q", inner.to_str().unwrap()]);
    assert!(personal_scope(&inner, true).is_err());
}
