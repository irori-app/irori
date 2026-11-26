#!/usr/bin/env python3
"""Check a pull request's branch, title, base, body, and Rust diff size.

The rules are documented in AGENTS.md ("Branches, commits and pull requests"):

- Branch: ``<type>/<short-kebab-description>``, for example ``fix/epic-login``.
- Title: a Conventional Commit header, ``<type>(<scope>): <summary>`` or
  ``<type>: <summary>``, with ``!`` before the colon for a breaking change.
- The branch's type and the title's type are the same.
- The base is ``main``, or another convention-named branch for stacked work.
- Body: must begin with ``## Plain-language summary`` (plain prose for end
  users, no fenced code blocks) followed by ``## Technical details``.
- Pull requests adding more than 1500 lines across ``*.rs`` files require the
  ``large-pr-approved`` label.
- Added or modified ``*.rs`` files over 800 lines emit a non-blocking warning
  pointing to ``.agents/skills/irori-submodule-refactor/SKILL.md``.

Bot branches (Dependabot, Renovate) are exempt from the branch, body,
diff-size, and large-file rules.

Usage, with the values GitHub provides:

    PR_TITLE=... PR_HEAD=... PR_BASE=... PR_BODY=... python3 .github/scripts/pr_rules.py

or ``pr_rules.py --title ... --head ... --base ... --body-file ...`` locally.
Exits 1 and prints every problem when a rule is broken.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Mapping
import os
from pathlib import Path
import re
import subprocess
import sys

# What each type is for; AGENTS.md has the same table.
TYPES = {
    "feat": "a new user-facing capability",
    "fix": "a bug fix",
    "perf": "a performance or memory improvement with no behaviour change",
    "refactor": "restructuring with no behaviour change",
    "docs": "documentation only (README, docs/, AGENTS.md, doc comments)",
    "test": "tests only",
    "i18n": "translations and locale catalogs only",
    "build": "Cargo manifests, dependencies, packaging/",
    "ci": ".github/ workflows and scripts",
    "chore": "maintenance that fits no other type",
    "revert": "reverting an earlier change",
}

MAIN = "main"
TITLE_MAX = 72
BOT_PREFIXES = ("dependabot/", "renovate/")
RS_ADDED_MAX = 1500
LARGE_PR_LABEL = "large-pr-approved"
LARGE_RS_WARN_LINES = 800

BODY_SUMMARY_HEADING = "Plain-language summary"
BODY_TECHNICAL_HEADING = "Technical details"
BODY_SUMMARY_MIN_CHARS = 30
BODY_SUMMARY_MIN_WORDS = 5
BODY_TECHNICAL_MIN_CHARS = 20

_TYPE = "|".join(TYPES)
BRANCH_RE = re.compile(rf"^(?P<type>{_TYPE})/(?P<name>[a-z0-9]+(?:[-.][a-z0-9]+)*)$")
TITLE_RE = re.compile(
    rf"^(?P<type>{_TYPE})(?:\((?P<scope>[a-z0-9]+(?:[-/][a-z0-9]+)*)\))?(?P<breaking>!)?: (?P<summary>.+)$"
)
HTML_COMMENT_RE = re.compile(r"<!--.*?-->", re.DOTALL)
FENCE_OPEN_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})([^\n]*)$")
H2_HEADING_RE = re.compile(r"^ {0,3}##[ \t]+(.+?)(?:[ \t]+#+[ \t]*)?$")


def branch_type(branch: str) -> str | None:
    match = BRANCH_RE.match(branch)
    return match["type"] if match else None


def rs_added_from_numstat(output: str) -> int:
    """Sum added lines across ``git diff --numstat`` entries."""
    total = 0
    for line in output.splitlines():
        if not line.strip():
            continue
        added = line.split("\t", 1)[0]
        if added.isdigit():
            total += int(added)
    return total


def git_rs_added(base_sha: str, head_sha: str) -> int:
    """Count added lines across ``*.rs`` files between two commits."""
    proc = subprocess.run(
        ["git", "diff", "--numstat", f"{base_sha}...{head_sha}", "--", "*.rs"],
        check=True,
        capture_output=True,
        text=True,
    )
    return rs_added_from_numstat(proc.stdout)


def check_large_rs_files(
    file_line_counts: Mapping[str, int],
    limit: int = LARGE_RS_WARN_LINES,
) -> list[str]:
    """Return non-blocking advisory warnings for touched ``*.rs`` files over ``limit`` lines."""
    warnings: list[str] = []
    for path, count in sorted(file_line_counts.items()):
        if count > limit:
            warnings.append(
                f"'{path}' has {count} lines (recommended module ceiling is {limit}). "
                "Consider splitting it into a thin facade plus submodules using "
                ".agents/skills/irori-submodule-refactor/SKILL.md."
            )
    return warnings


def git_large_rs_warnings(
    base_sha: str,
    head_sha: str,
    repo_root: Path | None = None,
    limit: int = LARGE_RS_WARN_LINES,
) -> list[str]:
    """Inspect added or modified ``*.rs`` files in git diff and warn when over ``limit`` lines."""
    root = (repo_root or Path.cwd()).resolve()
    proc = subprocess.run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=AM",
            f"{base_sha}...{head_sha}",
            "--",
            "*.rs",
        ],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    )
    counts: dict[str, int] = {}
    for rel in proc.stdout.splitlines():
        rel = rel.strip()
        if not rel:
            continue
        file_path = root / rel
        if file_path.is_file():
            try:
                counts[rel] = len(file_path.read_text(encoding="utf-8").splitlines())
            except OSError:
                continue
    return check_large_rs_files(counts, limit=limit)


def _extract_h2_sections(
    body: str,
) -> tuple[list[str], list[tuple[str, list[str], bool]]]:
    """Split a Markdown PR body into preamble lines and level-2 heading sections.

    Each section entry is ``(heading_text, content_lines, has_fenced_code)``.
    HTML comments are stripped first, and fenced code blocks are respected so
    ``## `` lines inside code snippets never count as top-level headings.
    """
    cleaned = HTML_COMMENT_RE.sub("", body)
    preamble: list[str] = []
    sections: list[tuple[str, list[str], bool]] = []

    current_heading: str | None = None
    current_lines: list[str] = []
    current_has_fence = False

    in_fence = False
    fence_char = ""
    fence_len = 0

    for line in cleaned.splitlines():
        if not in_fence:
            fence_match = FENCE_OPEN_RE.match(line)
            if fence_match and not (
                fence_match.group(1)[0] == "`" and "`" in fence_match.group(2)
            ):
                in_fence = True
                fence_char = fence_match.group(1)[0]
                fence_len = len(fence_match.group(1))
                if current_heading is None:
                    preamble.append(line)
                else:
                    current_has_fence = True
                    current_lines.append(line)
                continue

            h2_match = H2_HEADING_RE.match(line)
            if h2_match:
                if current_heading is not None:
                    sections.append((current_heading, current_lines, current_has_fence))
                current_heading = h2_match.group(1).strip()
                current_lines = []
                current_has_fence = False
                continue

            if current_heading is None:
                preamble.append(line)
            else:
                current_lines.append(line)
        else:
            stripped = line.lstrip(" ")
            leading_spaces = len(line) - len(stripped)
            if (
                leading_spaces <= 3
                and stripped.startswith(fence_char * fence_len)
                and stripped.rstrip(" \t") == fence_char * len(stripped.rstrip(" \t"))
            ):
                in_fence = False
            if current_heading is None:
                preamble.append(line)
            else:
                current_lines.append(line)

    if current_heading is not None:
        sections.append((current_heading, current_lines, current_has_fence))

    return preamble, sections


def _readable_prose(lines: list[str]) -> str:
    """Flatten Markdown prose lines to measure substantive text content."""
    cleaned_lines: list[str] = []
    for raw in lines:
        line = raw.strip()
        if not line:
            continue
        line = re.sub(r"^(?:[-*+]|\d+\.)\s+", "", line)
        line = re.sub(r"^>\s*", "", line)
        if line:
            cleaned_lines.append(line)
    return " ".join(cleaned_lines)


def check_body(body: str) -> list[str]:
    """Validate that a PR body opens with a plain-language summary before technical details."""
    problems: list[str] = []
    preamble, sections = _extract_h2_sections(body)

    # Allow only optional leading blockquote notices (such as a Draft PR callout).
    non_blockquote_preamble = [
        line for line in preamble if line.strip() and not line.lstrip().startswith(">")
    ]
    if non_blockquote_preamble:
        problems.append(
            f"The PR body must start with '## {BODY_SUMMARY_HEADING}' before any other "
            "text or section (only an optional leading blockquote notice is allowed)."
        )

    if not sections or sections[0][0].casefold() != BODY_SUMMARY_HEADING.casefold():
        problems.append(
            f"The PR body must start with '## {BODY_SUMMARY_HEADING}' explaining what "
            f"changes for end users before '## {BODY_TECHNICAL_HEADING}'."
        )

    summary_idx: int | None = None
    technical_idx: int | None = None
    for idx, (heading, _, _) in enumerate(sections):
        lowered = heading.casefold()
        if lowered == BODY_SUMMARY_HEADING.casefold() and summary_idx is None:
            summary_idx = idx
        elif lowered == BODY_TECHNICAL_HEADING.casefold() and technical_idx is None:
            technical_idx = idx

    if summary_idx is not None:
        _, summary_lines, summary_has_fence = sections[summary_idx]
        if summary_has_fence:
            problems.append(
                f"'## {BODY_SUMMARY_HEADING}' must not contain fenced code blocks; "
                f"keep code snippets in '## {BODY_TECHNICAL_HEADING}'."
            )
        prose = _readable_prose(summary_lines)
        if len(prose) < BODY_SUMMARY_MIN_CHARS or len(prose.split()) < BODY_SUMMARY_MIN_WORDS:
            problems.append(
                f"'## {BODY_SUMMARY_HEADING}' must explain what changes for end users in "
                f"plain language (at least {BODY_SUMMARY_MIN_WORDS} words and "
                f"{BODY_SUMMARY_MIN_CHARS} characters outside HTML comments)."
            )

    if technical_idx is None:
        problems.append(
            f"The PR body must include a '## {BODY_TECHNICAL_HEADING}' section after "
            f"'## {BODY_SUMMARY_HEADING}'."
        )
    else:
        if summary_idx is not None and technical_idx < summary_idx:
            problems.append(
                f"'## {BODY_TECHNICAL_HEADING}' must come after '## {BODY_SUMMARY_HEADING}'."
            )
        _, technical_lines, _ = sections[technical_idx]
        tech_text = _readable_prose(technical_lines)
        if len(tech_text) < BODY_TECHNICAL_MIN_CHARS:
            problems.append(
                f"'## {BODY_TECHNICAL_HEADING}' must describe the technical change "
                f"(at least {BODY_TECHNICAL_MIN_CHARS} characters outside HTML comments)."
            )

    return problems


def check(
    title: str,
    head: str,
    base: str,
    rs_added: int | None = None,
    labels: Iterable[str] = (),
    body: str | None = None,
) -> list[str]:
    """Return every rule the pull request breaks (empty when it follows them)."""
    problems: list[str] = []
    types = ", ".join(TYPES)

    bot = head.startswith(BOT_PREFIXES)
    head_type = branch_type(head)
    if not bot and head_type is None:
        problems.append(
            f"Branch '{head}' must be named <type>/<short-kebab-description>, "
            f"for example 'fix/epic-login'. Types: {types}."
        )

    title_match = TITLE_RE.match(title)
    if title_match is None:
        problems.append(
            f"Title '{title}' must be '<type>(<scope>): <summary>' or '<type>: <summary>', "
            f"for example 'fix(epic): keep the session after a token refresh'. Types: {types}."
        )
    else:
        summary = title_match["summary"]
        if summary != summary.strip() or not summary.strip():
            problems.append("The title's summary must not start or end with spaces.")
        if summary.endswith("."):
            problems.append("The title's summary must not end with a period.")
        if summary[:1].isupper() and not summary.split(" ", 1)[0].isupper():
            problems.append(
                "The title's summary must start in lower case ('fix(epic): keep ...'); "
                "only an acronym in capitals (GOG, UI) may lead."
            )
        if head_type and title_match["type"] != head_type:
            problems.append(
                f"The title's type '{title_match['type']}' must match the branch's type "
                f"'{head_type}' ('{head}')."
            )

    if len(title) > TITLE_MAX:
        problems.append(f"The title is {len(title)} characters; keep it to {TITLE_MAX}.")

    if base != MAIN and branch_type(base) is None:
        problems.append(
            f"The base '{base}' must be '{MAIN}', or a convention-named branch when "
            "stacking pull requests. Work merged into any other branch never reaches main."
        )

    label_set = {label.strip() for label in labels if label.strip()}
    if not bot and rs_added is not None and rs_added > RS_ADDED_MAX and LARGE_PR_LABEL not in label_set:
        problems.append(
            f"This pull request adds {rs_added} lines across *.rs files (limit is {RS_ADDED_MAX}). "
            f"Split it into smaller stacked pull requests or ask a maintainer to add the "
            f"'{LARGE_PR_LABEL}' label."
        )

    if not bot and body is not None:
        problems.extend(check_body(body))

    return problems


def _parse_labels(raw: str | None) -> list[str]:
    if not raw:
        return []
    return [part.strip() for part in raw.split(",") if part.strip()]


def main(argv: list[str]) -> int:
    env_rs_added = os.environ.get("PR_RS_ADDED")
    default_rs_added = int(env_rs_added) if env_rs_added and env_rs_added.strip() else None

    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--title", default=os.environ.get("PR_TITLE"))
    parser.add_argument("--head", default=os.environ.get("PR_HEAD"))
    parser.add_argument("--base", default=os.environ.get("PR_BASE", MAIN))
    parser.add_argument("--rs-added", type=int, default=default_rs_added)
    parser.add_argument("--labels", default=os.environ.get("PR_LABELS", ""))
    parser.add_argument("--base-sha", default=os.environ.get("PR_BASE_SHA"))
    parser.add_argument("--head-sha", default=os.environ.get("PR_HEAD_SHA"))
    parser.add_argument("--body", default=os.environ.get("PR_BODY"))
    parser.add_argument("--body-file", type=Path, default=None)
    args = parser.parse_args(argv)
    if args.title is None or args.head is None:
        parser.error("give --title and --head, or set PR_TITLE and PR_HEAD")

    body = args.body
    if args.body_file is not None:
        body = args.body_file.read_text(encoding="utf-8")

    rs_added = args.rs_added
    if rs_added is None and args.base_sha and args.head_sha:
        rs_added = git_rs_added(args.base_sha, args.head_sha)

    problems = check(
        args.title,
        args.head,
        args.base,
        rs_added=rs_added,
        labels=_parse_labels(args.labels),
        body=body,
    )

    is_gha = bool(os.environ.get("GITHUB_ACTIONS"))
    bot = args.head.startswith(BOT_PREFIXES)
    if not bot and args.base_sha and args.head_sha:
        warn_prefix = "::warning::" if is_gha else "warning: "
        for warning in git_large_rs_warnings(args.base_sha, args.head_sha):
            print(f"{warn_prefix}{warning}")

    err_prefix = "::error::" if is_gha else "error: "
    for problem in problems:
        # GitHub shows ::error lines as annotations on the run.
        print(f"{err_prefix}{problem}")
    if not problems:
        print(f"OK: '{args.title}' from '{args.head}' into '{args.base}'")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
