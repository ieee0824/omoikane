#!/usr/bin/env python3
"""Tell CI whether a change can use the documentation-only path."""

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
    run_full = not is_documentation_only(paths)
    print(f"Changed files: {len(paths) if paths is not None else 'unknown'}; run_full={run_full}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"run_full={str(run_full).lower()}\n")


if __name__ == "__main__":
    main()
