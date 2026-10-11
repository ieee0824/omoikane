#!/usr/bin/env python3
"""Classify changes for full checks and development-container builds."""

import json
import os
from pathlib import Path
from urllib.request import Request, urlopen


def is_documentation_only(paths):
    """Only explicitly listed prose files may skip executable checks."""
    return bool(paths) and all(
        path in {"README.md", "AGENTS.md"}
        or (path.startswith("docs/") and path.endswith(".md"))
        for path in paths
    )


DOCKER_PATHS = {"Dockerfile", ".dockerignore", "docker-entrypoint.sh"}


def check_scope(paths):
    """Keep unknown/code changes on full CI; isolate container-only changes."""
    container_only = bool(paths) and all(
        path in DOCKER_PATHS or is_documentation_only([path]) for path in paths
    )
    return {
        "run_full": not container_only,
        "run_docker": paths is None or any(path in DOCKER_PATHS for path in paths)
        or any(path in {".github/workflows/docker-build.yml",
                       ".github/workflows/change-scope.yml",
                       "scripts/ci-change-scope.py"} for path in paths),
    }


def github_json(path, token):
    request = Request(
        f"https://api.github.com/repos/{os.environ['GITHUB_REPOSITORY']}/{path}",
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    with urlopen(request, timeout=20) as response:
        return json.load(response)


def changed_paths(event_name, event, token):
    """Return None when the complete change set cannot be established."""
    def names(files):
        return [name for item in files for name in
                (item["filename"], item.get("previous_filename")) if name]

    if event_name == "pull_request":
        number = event["pull_request"]["number"]
        paths = []
        for page in range(1, 31):
            files = github_json(f"pulls/{number}/files?per_page=100&page={page}", token)
            paths.extend(names(files))
            if len(files) < 100:
                return paths
        return None  # GitHub's PR-files API stops at 3000 entries.
    if event_name == "push" and event.get("ref") == "refs/heads/main":
        before, after = event["before"], event["after"]
        if set(before) == {"0"}:
            return None
        comparison = github_json(f"compare/{before}...{after}", token)
        files = comparison["files"]
        return names(files) if len(files) < 300 else None
    return None  # Schedule, dispatch, tags and unknown events run the full suite.


def main():
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    try:
        paths = changed_paths(os.environ["GITHUB_EVENT_NAME"], event, os.environ["CI_AUTH"])
    except (OSError, KeyError, ValueError) as error:
        print(f"Could not classify the complete change set: {error}")
        paths = None
    checks = check_scope(paths)
    print(f"Changed files: {len(paths) if paths is not None else 'unknown'}; {checks}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for name, enabled in checks.items():
            output.write(f"{name}={str(enabled).lower()}\n")


if __name__ == "__main__":
    main()
