#[allow(dead_code)]
#[path = "../build_support/revision.rs"]
mod revision;

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct Repository(PathBuf);

impl Repository {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "omoikane-build-revision-{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::create_dir(path.join("empty-hooks")).unwrap();
        Self(path)
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.0)
            .args([
                "-c",
                "user.name=Build Test",
                "-c",
                "user.email=build-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
            ])
            .arg(format!(
                "core.hooksPath={}",
                self.0.join("empty-hooks").display()
            ))
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(&self) {
        self.git(&["add", "source.rs"]);
        self.git(&["commit", "-qm", "source"]);
    }
}

impl Drop for Repository {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn identity_changes_with_commits_and_tracks_staged_and_unstaged_edits() {
    let repo = Repository::new();
    repo.git(&["init", "-q"]);
    std::fs::write(repo.0.join("source.rs"), "first").unwrap();
    repo.commit();
    let first = revision::collect(&repo.0);
    assert_eq!(first.commit, repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(first.tracked_dirty, Some(false));
    assert!(first.watch.contains(&repo.0.join("source.rs")));
    assert!(first.watch.iter().all(|path| path.exists()));
    std::fs::write(repo.0.join("untracked.log"), "evidence").unwrap();
    assert_eq!(revision::collect(&repo.0).tracked_dirty, Some(false));
    std::fs::write(repo.0.join("source.rs"), "second").unwrap();
    assert_eq!(revision::collect(&repo.0).tracked_dirty, Some(true));
    repo.git(&["add", "source.rs"]);
    assert_eq!(revision::collect(&repo.0).tracked_dirty, Some(true));
    repo.commit();
    let second = revision::collect(&repo.0);
    assert_ne!(first.commit, second.commit);
    assert_eq!(second.tracked_dirty, Some(false));
}

#[test]
fn worktree_watches_its_head_and_common_branch_reference() {
    let repo = Repository::new();
    repo.git(&["init", "-q"]);
    std::fs::write(repo.0.join("source.rs"), "first").unwrap();
    repo.commit();
    let linked = repo.0.join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-qb",
        "build-test",
        linked.to_str().unwrap(),
    ]);
    let metadata = revision::collect(&linked);
    assert_eq!(metadata.commit, repo.git(&["rev-parse", "HEAD"]));
    for name in ["HEAD", "index", "refs/heads/build-test"] {
        let path = Command::new("git")
            .arg("-C")
            .arg(&linked)
            .args(["rev-parse", "--path-format=absolute", "--git-path", name])
            .output()
            .unwrap();
        let path = PathBuf::from(String::from_utf8(path.stdout).unwrap().trim());
        assert!(metadata.watch.contains(&path), "{}", path.display());
    }
}

#[test]
fn source_archive_does_not_claim_an_unrelated_parent_commit() {
    let repo = Repository::new();
    assert_eq!(revision::collect(&repo.0).commit, "unknown");
    assert_eq!(revision::collect(&repo.0).tracked_dirty, None);
    repo.git(&["init", "-q"]);
    std::fs::write(repo.0.join("source.rs"), "source").unwrap();
    repo.commit();
    let archive = repo.0.join("archive");
    std::fs::create_dir(&archive).unwrap();
    assert_eq!(revision::collect(&archive).commit, "unknown");
    assert_eq!(revision::collect(&archive).tracked_dirty, None);
}

#[test]
fn identity_probe_does_not_refresh_the_watched_git_index() {
    let repo = Repository::new();
    repo.git(&["init", "-q"]);
    let source = repo.0.join("source.rs");
    std::fs::write(&source, "source").unwrap();
    repo.commit();
    let before = std::fs::read(repo.0.join(".git/index")).unwrap();
    // A stat-only change makes normal `git status` refresh the cached index,
    // even though the tracked file's contents and dirty state are unchanged.
    let modified =
        std::fs::metadata(&source).unwrap().modified().unwrap() - std::time::Duration::from_secs(5);
    std::fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(revision::collect(&repo.0).tracked_dirty, Some(false));
    assert_eq!(std::fs::read(repo.0.join(".git/index")).unwrap(), before);
}
