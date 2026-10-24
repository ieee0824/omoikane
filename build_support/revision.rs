//! Build-time Git identity. No Git invocation is needed by the running browser.
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) struct Revision {
    pub(crate) commit: String,
    pub(crate) tracked_dirty: Option<bool>,
    pub(crate) watch: Vec<PathBuf>,
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        // `status` must not refresh index stat data: the index is itself a
        // watched input, and a refresh would force an otherwise no-op rebuild.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    String::from_utf8(git(root, args)?)
        .ok()
        .map(|s| s.trim().to_owned())
}

pub(crate) fn collect(root: &Path) -> Revision {
    let mut value = Revision {
        commit: "unknown".into(),
        tracked_dirty: None,
        watch: Vec::new(),
    };
    // An extracted source archive inside another checkout must not inherit the
    // unrelated parent's revision. Worktrees still resolve their own root.
    let Some(top) = git_text(root, &["rev-parse", "--show-toplevel"]) else {
        return value;
    };
    if root.canonicalize().ok() != Path::new(&top).canonicalize().ok() {
        return value;
    }
    if let Some(commit) = git_text(root, &["rev-parse", "--verify", "HEAD"])
        && matches!(commit.len(), 40 | 64)
        && commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        value.commit = commit;
    }
    value.tracked_dirty = git(root, &["status", "--porcelain=v1", "--untracked-files=no"])
        .map(|bytes| !bytes.is_empty());

    // rev-parse resolves .git indirection correctly for linked worktrees.
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git_text(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        ) {
            let path = PathBuf::from(path);
            if path.exists() {
                value.watch.push(path);
            }
        }
    }
    if let Some(branch) = git_text(root, &["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git_text(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", &branch],
        )
    {
        let path = PathBuf::from(path);
        if path.exists() {
            value.watch.push(path);
        }
    }
    if let Some(files) = git(root, &["ls-files", "-z"]) {
        for file in files.split(|b| *b == 0).filter(|s| !s.is_empty()) {
            if let Ok(file) = std::str::from_utf8(file)
                && !file.contains(['\n', '\r'])
            {
                value.watch.push(root.join(file));
            }
        }
    }
    value.watch.sort();
    value.watch.dedup();
    value
}

pub(crate) fn emit(root: &Path) {
    let revision = collect(root);
    println!("cargo::rustc-env=OMOIKANE_BUILD_COMMIT={}", revision.commit);
    println!(
        "cargo::rustc-env=OMOIKANE_BUILD_TRACKED_DIRTY={}",
        match revision.tracked_dirty {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        }
    );
    for path in revision.watch {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    for (cargo, output) in [
        ("PROFILE", "PROFILE"),
        ("OPT_LEVEL", "OPT_LEVEL"),
        ("TARGET", "TARGET"),
    ] {
        println!(
            "cargo::rustc-env=OMOIKANE_BUILD_{output}={}",
            std::env::var(cargo).unwrap_or_else(|_| "unknown".into())
        );
    }
}
