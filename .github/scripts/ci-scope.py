#!/usr/bin/env python3
"""Decides how much of CI a change needs.

Windows x64 is the one system this project builds, checks and releases, so
everything is tested there. A change to Markdown, the site, or the repository's
GitHub settings alone is still tested (the branding test reads the guide) but
skips the Rust lints.

Usage: ci-scope.py --repo <owner/name> --event <name> [--pr <number>]
                   [--before <sha> --after <sha>]
Writes `code` and `platforms` to $GITHUB_OUTPUT.
"""

import argparse
import fnmatch
import json
import os
import subprocess

SYSTEMS = ["windows-latest"]
NO_BUILD = ["docs/*", "*.md", ".github/*"]
FULL = ["Cargo.toml", "Cargo.lock", ".cargo/*", "rust-toolchain*", "build.rs", ".github/workflows/ci.yml"]


def matches(path, globs):
    """fnmatch's * crosses folders, so docs/* is the whole site."""
    return any(fnmatch.fnmatch(path, glob) for glob in globs)


def scope(event, paths, labels):
    """Whether the Rust code needs checking, and the systems to test on.

    `paths` is None when the change could not be read, which checks everything.
    """
    if paths and all(matches(path, NO_BUILD) and not matches(path, FULL) for path in paths):
        return False, SYSTEMS
    return True, SYSTEMS


def api(path, jq):
    out = subprocess.run(
        ["gh", "api", "--paginate", path, "--jq", jq], check=True, capture_output=True, text=True
    ).stdout
    return [line for line in out.splitlines() if line]


def changed(args):
    try:
        if args.event == "pull_request":
            return api(f"repos/{args.repo}/pulls/{args.pr}/files", ".[].filename")
        if args.event == "push" and args.before and set(args.before) != {"0"}:
            return api(f"repos/{args.repo}/compare/{args.before}...{args.after}", ".files[].filename")
    except subprocess.CalledProcessError:
        pass
    return None


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--event", required=True)
    parser.add_argument("--pr")
    parser.add_argument("--before")
    parser.add_argument("--after")
    args = parser.parse_args()
    labels = set()
    if args.event == "pull_request":
        labels = set(api(f"repos/{args.repo}/issues/{args.pr}/labels", ".[].name"))
    code, platforms = scope(args.event, changed(args), labels)
    print(f"Rust checks: {code}. Test systems: {', '.join(platforms)}.")
    with open(os.environ["GITHUB_OUTPUT"], "a") as out:
        out.write(f"code={json.dumps(code)}\nplatforms={json.dumps(platforms)}\n")


if __name__ == "__main__":
    main()
