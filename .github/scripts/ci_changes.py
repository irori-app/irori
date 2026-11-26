#!/usr/bin/env python3
"""Classify whether a pull request changes code or build inputs requiring Rust CI.

Outputs ``code_changed=true`` or ``code_changed=false`` to ``$GITHUB_OUTPUT``
(and stdout) so ``.github/workflows/ci.yml`` can skip heavy native Rust builds
on docs, skills, issue template, PR script, and website-only pull requests
while still reporting every required Ruleset check green.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable
import os
from pathlib import Path
import subprocess
import sys

DOCS_ONLY_EXACT = frozenset({
    "AGENTS.md",
    "CLAUDE.md",
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
    "MAINTAINERS.md",
    "CODE_OF_CONDUCT.md",
    "THIRD-PARTY-NOTICES.md",
    ".github/pull_request_template.md",
    ".github/copilot-instructions.md",
    ".github/CODEOWNERS",
})

DOCS_ONLY_PREFIXES = (
    "docs/",
    ".agents/",
    ".github/ISSUE_TEMPLATE/",
    ".github/scripts/",
    "website/",
)


def is_docs_only_path(raw_path: str) -> bool:
    """Return True only for paths verified outside Rust/packaging CI."""
    path = raw_path.strip().replace("\\", "/")
    if not path or path.startswith("/") or ".." in Path(path).parts:
        return False
    if path in DOCS_ONLY_EXACT:
        return True
    return any(path.startswith(prefix) for prefix in DOCS_ONLY_PREFIXES)


def needs_code_ci(changed_paths: Iterable[str]) -> bool:
    """Return True unless every changed path is a verified docs/governance path."""
    paths = [p.strip() for p in changed_paths if p.strip()]
    if not paths:
        # Fail safe when no file list is available (for example non-PR triggers).
        return True
    return any(not is_docs_only_path(p) for p in paths)


def git_changed_paths(base_sha: str, head_sha: str) -> list[str]:
    proc = subprocess.run(
        ["git", "diff", "--name-only", f"{base_sha}...{head_sha}"],
        check=True,
        capture_output=True,
        text=True,
    )
    return [line.strip() for line in proc.stdout.splitlines() if line.strip()]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--event", default=os.environ.get("GITHUB_EVENT_NAME", "push"))
    parser.add_argument("--base-sha", default=os.environ.get("PR_BASE_SHA"))
    parser.add_argument("--head-sha", default=os.environ.get("PR_HEAD_SHA"))
    args = parser.parse_args(argv)

    if args.event != "pull_request" or not args.base_sha or not args.head_sha:
        changed = True
    else:
        try:
            changed = needs_code_ci(git_changed_paths(args.base_sha, args.head_sha))
        except subprocess.CalledProcessError as exc:
            print(f"warning: git diff failed ({exc}); defaulting to code_changed=true", file=sys.stderr)
            changed = True

    value = "true" if changed else "false"
    print(f"code_changed={value}")
    github_output = os.environ.get("GITHUB_OUTPUT")
    if github_output:
        with open(github_output, "a", encoding="utf-8") as fh:
            fh.write(f"code_changed={value}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
