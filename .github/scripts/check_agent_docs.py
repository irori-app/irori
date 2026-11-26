#!/usr/bin/env python3
"""Validate Irori's agent documentation, Mermaid graphs, skills, and code paths.

Runs automatically in CI through ``.github/workflows/pr-rules.yml`` (via
``test_check_agent_docs.py``) and locally from the repository root:

    python3 .github/scripts/check_agent_docs.py

Checks performed:
1. Context-window size budgets on ``AGENTS.md`` (<= 20,000 bytes) and
   ``.github/copilot-instructions.md`` (<= 4,000 bytes).
2. Relative Markdown links and ``#heading-anchor`` fragments across
   ``AGENTS.md``, ``SECURITY.md``, ``.github/copilot-instructions.md``,
   ``docs/*.md``, and ``.agents/skills/*/SKILL.md``.
3. Explicit repository code paths in inline code and fenced blocks
   (``crates/...``, ``packaging/...``, ``.github/...``, ``.agents/...``,
   ``assets/...``, ``docs/...``, ``website/...``, ``tests/...``,
   ``<package>/src/...`` aliases, and ``<subdir>/<file>.rs`` module paths).
4. Bidirectional active workspace crate parity between root ``Cargo.toml``,
   the crate table in ``docs/ARCHITECTURE.md``, and Graph 1 in ``docs/GRAPHS.md``.
5. Mermaid diagram headers, balanced ``subgraph``/sequence blocks, quoted
   parentheses in bracketed and rounded node labels, unescaped ``#`` entity
   hazards, and raw ``<tag>`` substrings stripped by GitHub's DOMPurify HTML
   label sanitizer.
6. Skill directories in ``.agents/skills/<dir>/SKILL.md``: YAML frontmatter
   (single-line or multiline block scalars), non-nested ``<example>``
   (``<input>``/``<output>``) few-shot blocks, and routing links in ``AGENTS.md``.
"""

from __future__ import annotations

import argparse
import glob
import os
from pathlib import Path
import re
import sys
import tomllib

AGENTS_MAX_BYTES = 20_000
COPILOT_MAX_BYTES = 4_000
MIN_GRAPHS_MERMAID_BLOCKS = 5

REQUIRED_DOCS = (
    "AGENTS.md",
    "SECURITY.md",
    ".github/copilot-instructions.md",
    "docs/ARCHITECTURE.md",
    "docs/GRAPHS.md",
    "docs/UI_CONVENTIONS.md",
    "docs/PERFORMANCE.md",
    "docs/TRANSLATING.md",
    "docs/PROMPTING_PLAYBOOK.md",
)

NORMATIVE_PATH_DOCS = (
    "AGENTS.md",
    ".github/copilot-instructions.md",
    "docs/ARCHITECTURE.md",
    "docs/GRAPHS.md",
    "docs/UI_CONVENTIONS.md",
    "docs/PERFORMANCE.md",
    "docs/TRANSLATING.md",
    "docs/PROMPTING_PLAYBOOK.md",
)

REPO_PATH_PREFIXES = (
    "crates/",
    "packaging/",
    ".github/",
    ".agents/",
    "assets/",
    "docs/",
    "website/",
    "tests/",
)

CRATE_SUBPATH_ROOTS = {
    "src",
    "locales",
    "tests",
    "benches",
    "examples",
    "Cargo.toml",
    "README.md",
    "PATCHES.md",
    "build.rs",
}

SKILL_DIR_RE = re.compile(r"^[a-z0-9]+(?:-[a-z0-9]+)*$")
FENCE_OPEN_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})([^\n]*)$")
ATX_HEADING_RE = re.compile(r"^ {0,3}#{1,6}[ \t]+(.+?)(?:[ \t]+#+[ \t]*)?$")
FLOWCHART_HEADER_RE = re.compile(r"^(?:flowchart|graph)\s+(?:TD|TB|BT|RL|LR)\b")
OTHER_MERMAID_HEADER_RE = re.compile(
    r"^(?:sequenceDiagram|stateDiagram(?:-v2)?|classDiagram|erDiagram)\b"
)
SEQUENCE_BLOCK_OPEN_RE = re.compile(
    r"^(?:loop|alt|opt|par|critical|break|rect|box)\b"
)
TEMPLATE_MARKERS_RE = re.compile(r"[*<>\{\}\$\(\?]")
BRACE_GROUP_RE = re.compile(r"\{([a-zA-Z0-9_.,-]+)\}")
FENCED_REPO_PATH_RE = re.compile(
    r"(?<![a-zA-Z0-9_/:.-])(?:crates|packaging|\.github|\.agents|assets|docs|website|tests)/[a-zA-Z0-9_<>/.*{},-]+(?:\.[a-zA-Z0-9_]+)?"
)
RELATIVE_RS_SUBPATH_RE = re.compile(r"^[a-zA-Z0-9_-]+(?:/[a-zA-Z0-9_-]+)+\.rs$")
ARCH_CRATE_TABLE_ROW_RE = re.compile(
    r"^\|\s*`(crates/[a-zA-Z0-9_-]+)`\s*\|\s*`([a-zA-Z0-9_-]+)`\s*\|"
)
MERMAID_RAW_HTML_TAG_RE = re.compile(r"<(?!br\s*/?>)[a-zA-Z/][^>]*>")
MERMAID_BAD_HASH_RE = re.compile(r"#(?!(?:quot|amp|lt|gt|[0-9]+);)")
YAML_BLOCK_SCALAR_RE = re.compile(r"^[|>][+-]?(?:\d+)?$")


class WorkspaceInfo:
    """Active workspace members, package aliases, and `.rs` suffix index."""

    def __init__(
        self,
        active_members: list[tuple[str, str]],
        alias_to_dir: dict[str, str],
        all_crate_pkgs: dict[str, str],
        rs_suffixes: set[str],
    ) -> None:
        self.active_members = active_members
        self.alias_to_dir = alias_to_dir
        self.all_crate_pkgs = all_crate_pkgs
        self.rs_suffixes = rs_suffixes


def _index_crate_rs_suffixes(crate_dir: Path, suffixes: set[str]) -> None:
    src_dir = crate_dir / "src"
    if not src_dir.is_dir():
        return
    for rs_file in src_dir.rglob("*.rs"):
        rel_parts = rs_file.relative_to(crate_dir).parts
        for start_idx in range(len(rel_parts) - 1):
            suffixes.add("/".join(rel_parts[start_idx:]))


def load_workspace_info(root: Path, problems: list[str]) -> WorkspaceInfo:
    cargo_path = root / "Cargo.toml"
    if not cargo_path.is_file():
        problems.append("Cargo.toml: missing workspace root manifest")
        return WorkspaceInfo([], {}, {}, set())

    try:
        manifest = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        problems.append(f"Cargo.toml: failed to parse TOML ({exc})")
        return WorkspaceInfo([], {}, {}, set())

    workspace = manifest.get("workspace", {})
    member_patterns = workspace.get("members", [])
    excluded = {Path(p).as_posix().rstrip("/") for p in workspace.get("exclude", [])}

    member_dirs: set[Path] = set()
    for pattern in member_patterns:
        for match_str in glob.glob(str(root / pattern)):
            match_path = Path(match_str)
            if match_path.is_dir() and (match_path / "Cargo.toml").is_file():
                member_dirs.add(match_path)

    active_members: list[tuple[str, str]] = []
    alias_to_dir: dict[str, str] = {}
    all_crate_pkgs: dict[str, str] = {}
    rs_suffixes: set[str] = set()

    # Map both active and excluded crates under crates/ so `<pkg>/src/...` aliases resolve cleanly.
    crates_dir = root / "crates"
    all_crate_dirs = set(member_dirs)
    if crates_dir.is_dir():
        for child in crates_dir.iterdir():
            if child.is_dir() and (child / "Cargo.toml").is_file():
                all_crate_dirs.add(child)

    for crate_dir in sorted(all_crate_dirs):
        rel = crate_dir.relative_to(root).as_posix()
        alias_to_dir[crate_dir.name] = rel
        _index_crate_rs_suffixes(crate_dir, rs_suffixes)
        try:
            crate_toml = tomllib.loads(
                (crate_dir / "Cargo.toml").read_text(encoding="utf-8")
            )
        except (OSError, tomllib.TOMLDecodeError) as exc:
            problems.append(f"{rel}/Cargo.toml: failed to parse TOML ({exc})")
            continue

        pkg_name = crate_toml.get("package", {}).get("name")
        if not isinstance(pkg_name, str) or not pkg_name:
            problems.append(f"{rel}/Cargo.toml: missing [package].name")
            continue

        alias_to_dir[pkg_name] = rel
        all_crate_pkgs[rel] = pkg_name
        if crate_dir in member_dirs and rel not in excluded:
            active_members.append((rel, pkg_name))

    if not active_members:
        problems.append("Cargo.toml: no active workspace crates discovered")

    return WorkspaceInfo(active_members, alias_to_dir, all_crate_pkgs, rs_suffixes)


def _strip_frontmatter_lines(lines: list[str]) -> int:
    """Return 0-based line index where body starts after optional YAML frontmatter."""
    if not lines or lines[0].strip() != "---":
        return 0
    for idx in range(1, len(lines)):
        if lines[idx].strip() == "---":
            return idx + 1
    return 0


def iter_markdown_structure(
    text: str,
    rel_doc: str | None = None,
    problems: list[str] | None = None,
) -> tuple[list[tuple[int, str]], list[tuple[int, str, list[tuple[int, str]]]]]:
    """Split Markdown into non-fenced lines and fenced blocks.

    Returns ``(non_fenced_lines, fenced_blocks)`` where ``non_fenced_lines`` is a
    list of ``(line_no, line_text)`` and ``fenced_blocks`` is a list of
    ``(open_line_no, info_string, [(line_no, line_text), ...])``.
    """
    non_fenced: list[tuple[int, str]] = []
    fenced_blocks: list[tuple[int, str, list[tuple[int, str]]]] = []

    in_fence = False
    fence_char = ""
    fence_len = 0
    fence_open_line = 0
    fence_info = ""
    fence_lines: list[tuple[int, str]] = []

    for idx, line in enumerate(text.splitlines(), start=1):
        if not in_fence:
            match = FENCE_OPEN_RE.match(line)
            if match:
                marker = match.group(1)
                info = match.group(2).strip()
                # In CommonMark, backtick fences cannot contain backticks in their info string.
                if marker[0] == "`" and "`" in info:
                    non_fenced.append((idx, line))
                    continue
                in_fence = True
                fence_char = marker[0]
                fence_len = len(marker)
                fence_open_line = idx
                fence_info = info.split()[0] if info else ""
                fence_lines = []
            else:
                non_fenced.append((idx, line))
        else:
            stripped = line.lstrip(" ")
            leading_spaces = len(line) - len(stripped)
            if (
                leading_spaces <= 3
                and stripped.startswith(fence_char * fence_len)
                and stripped.rstrip(" \t") == fence_char * len(stripped.rstrip(" \t"))
            ):
                fenced_blocks.append((fence_open_line, fence_info, fence_lines))
                in_fence = False
            else:
                fence_lines.append((idx, line))

    if in_fence and rel_doc is not None and problems is not None:
        problems.append(f"{rel_doc}:{fence_open_line}: unclosed fenced code block")

    return non_fenced, fenced_blocks


def _find_closing_backticks(line: str, start: int, ticks: int) -> int:
    """Find the index of the next backtick run of exact length ``ticks`` (or -1)."""
    n = len(line)
    pos = start
    while pos < n:
        if line[pos] == "`":
            run = 1
            while pos + run < n and line[pos + run] == "`":
                run += 1
            if run == ticks:
                return pos
            pos += run
        else:
            pos += 1
    return -1


def _normalize_inline_code_span(span: str) -> str:
    if len(span) >= 2 and span.startswith(" ") and span.endswith(" ") and span.strip(" "):
        return span[1:-1]
    return span


def scan_line_spans_and_links(line: str) -> tuple[list[str], list[str]]:
    """Extract inline backtick code spans and Markdown link targets from one line.

    Processes left-to-right so `` `[not](a-link)` `` is treated as inline code while
    ``[`code`](real/link.md)`` extracts both the code span ``code`` and target
    ``real/link.md``.
    """
    code_spans: list[str] = []
    links: list[str] = []
    n = len(line)
    i = 0

    while i < n:
        ch = line[i]
        if ch == "\\":
            i += 2
            continue

        if ch == "`":
            ticks = 1
            while i + ticks < n and line[i + ticks] == "`":
                ticks += 1
            end = _find_closing_backticks(line, i + ticks, ticks)
            if end != -1:
                span = _normalize_inline_code_span(line[i + ticks : end])
                code_spans.append(span)
                i = end + ticks
                continue
            i += ticks
            continue

        is_image = ch == "!" and i + 1 < n and line[i + 1] == "["
        if ch == "[" or is_image:
            label_start = i + 2 if is_image else i + 1
            j = label_start
            depth = 1
            inner_spans: list[str] = []
            while j < n and depth > 0:
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == "`":
                    ticks = 1
                    while j + ticks < n and line[j + ticks] == "`":
                        ticks += 1
                    end_tick = _find_closing_backticks(line, j + ticks, ticks)
                    if end_tick != -1:
                        inner_spans.append(
                            _normalize_inline_code_span(line[j + ticks : end_tick])
                        )
                        j = end_tick + ticks
                        continue
                    j += ticks
                    continue
                if line[j] == "[":
                    depth += 1
                elif line[j] == "]":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1

            if depth == 0 and j + 1 < n and line[j + 1] == "(":
                code_spans.extend(inner_spans)
                k = j + 2
                paren_depth = 1
                while k < n and paren_depth > 0:
                    if line[k] == "\\":
                        k += 2
                        continue
                    if line[k] == "(":
                        paren_depth += 1
                    elif line[k] == ")":
                        paren_depth -= 1
                        if paren_depth == 0:
                            break
                    k += 1
                if paren_depth == 0:
                    raw_target = line[j + 2 : k].strip()
                    if raw_target.startswith("<") and ">" in raw_target:
                        raw_target = raw_target[1 : raw_target.index(">")]
                    elif " " in raw_target or "\t" in raw_target:
                        raw_target = raw_target.split()[0]
                    links.append(raw_target)
                    i = k + 1
                    continue

        i += 1

    return code_spans, links


def _github_slugify_heading(raw_heading: str) -> str:
    """Approximate GitHub (`github-slugger`) anchor generation for ATX headings."""
    text = re.sub(r"!\[([^\]]*)\]\([^)]*\)", r"\1", raw_heading)
    text = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", text)
    text = text.replace("`", "")
    text = re.sub(r"<[^>]+>", "", text)
    text = text.strip().lower()
    cleaned = "".join(
        ch for ch in text if ch.isalnum() or ch in (" ", "-", "_")
    )
    return cleaned.replace(" ", "-")


def get_markdown_heading_slugs(
    md_path: Path, cache: dict[Path, set[str]]
) -> set[str]:
    resolved = md_path.resolve()
    cached = cache.get(resolved)
    if cached is not None:
        return cached

    slugs: set[str] = set()
    counts: dict[str, int] = {}
    try:
        raw_text = resolved.read_text(encoding="utf-8")
    except OSError:
        cache[resolved] = slugs
        return slugs

    raw_lines = raw_text.splitlines()
    body_start = _strip_frontmatter_lines(raw_lines)
    body_text = "\n".join(raw_lines[body_start:])
    non_fenced, _ = iter_markdown_structure(body_text)

    for _, line in non_fenced:
        match = ATX_HEADING_RE.match(line)
        if not match:
            continue
        base = _github_slugify_heading(match.group(1))
        if not base:
            continue
        seen = counts.get(base, 0)
        counts[base] = seen + 1
        slugs.add(base if seen == 0 else f"{base}-{seen}")

    cache[resolved] = slugs
    return slugs


def check_relative_link(
    root: Path,
    doc_path: Path,
    line_no: int,
    target: str,
    slug_cache: dict[Path, set[str]],
    problems: list[str],
) -> None:
    rel_doc = doc_path.relative_to(root).as_posix()
    if not target:
        problems.append(f"{rel_doc}:{line_no}: empty Markdown link target")
        return

    lower = target.lower()
    if lower.startswith(("http://", "https://", "mailto:")):
        return
    if lower.startswith("file://"):
        problems.append(
            f"{rel_doc}:{line_no}: local file:// link '{target}' is not portable on GitHub; use a relative path"
        )
        return

    if target.startswith("#"):
        anchor = target[1:]
        if not anchor:
            problems.append(f"{rel_doc}:{line_no}: empty anchor '#' in Markdown link")
            return
        slugs = get_markdown_heading_slugs(doc_path, slug_cache)
        if anchor.lower() not in slugs:
            problems.append(
                f"{rel_doc}:{line_no}: broken heading anchor '#{anchor}' in {rel_doc}"
            )
        return

    without_query = target.split("?", 1)[0]
    path_part, has_hash, anchor = without_query.partition("#")
    if not path_part:
        return

    resolved = (doc_path.parent / path_part).resolve()
    root_resolved = root.resolve()
    try:
        resolved.relative_to(root_resolved)
    except ValueError:
        problems.append(
            f"{rel_doc}:{line_no}: link '{target}' escapes repository root"
        )
        return

    if not resolved.exists():
        problems.append(
            f"{rel_doc}:{line_no}: broken relative link '{target}' (resolved to '{resolved.relative_to(root_resolved).as_posix()}')"
        )
        return

    if has_hash:
        if not anchor:
            problems.append(
                f"{rel_doc}:{line_no}: empty '#' anchor in Markdown link '{target}'"
            )
        elif resolved.is_file() and resolved.suffix.lower() == ".md":
            slugs = get_markdown_heading_slugs(resolved, slug_cache)
            if anchor.lower() not in slugs:
                rel_target = resolved.relative_to(root_resolved).as_posix()
                problems.append(
                    f"{rel_doc}:{line_no}: broken heading anchor '#{anchor}' in '{rel_target}'"
                )


def expand_braces(path_str: str) -> list[str]:
    match = BRACE_GROUP_RE.search(path_str)
    if not match:
        return [path_str]
    prefix = path_str[: match.start()]
    suffix = path_str[match.end() :]
    choices = [part for part in match.group(1).split(",") if part]
    if len(choices) <= 1:
        return [path_str]
    expanded: list[str] = []
    for choice in choices:
        expanded.extend(expand_braces(f"{prefix}{choice}{suffix}"))
    return expanded


def _strip_token_punctuation(raw_token: str) -> str:
    token = raw_token.strip().strip("\"'(),;")
    while token.endswith((":", ".")) and not token.endswith(".."):
        token = token[:-1]
    return token


def normalize_candidate_token(
    raw_token: str, workspace: WorkspaceInfo
) -> str | None:
    """Return a repo-relative path candidate if ``raw_token`` names a repo path."""
    token = _strip_token_punctuation(raw_token)
    if not token or token.startswith(("-", "$", "~", "http://", "https://")):
        return None
    if "://" in token:
        return None

    # Strip Rust item suffix after `.rs` (e.g. `window.rs::helper`) or line anchor.
    if ".rs::" in token:
        token = token.split("::", 1)[0]
    if "#" in token:
        token = token.split("#", 1)[0]
    token = _strip_token_punctuation(token)

    if token.startswith(REPO_PATH_PREFIXES):
        # Two-segment `docs/<kebab-branch>` without an extension or trailing slash
        # is a Conventional Branch name (`docs/agents-md`), not a file in `docs/`.
        if re.fullmatch(r"docs/[a-z0-9]+(?:-[a-z0-9]+)*", token):
            return None
        return token

    # Exact `crates/<name>` reference (such as `crates/core` or `crates/webview`).
    if re.fullmatch(r"crates/[a-zA-Z0-9_-]+", token):
        return token

    # Workspace crate alias: `<pkg-or-dir>/(src|locales|...)`.
    if "/" in token:
        first, rest = token.split("/", 1)
        mapped_dir = workspace.alias_to_dir.get(first)
        if mapped_dir is not None:
            second = rest.split("/", 1)[0]
            if second in CRATE_SUBPATH_ROOTS:
                return f"{mapped_dir}/{rest}"

    return None


def verify_repo_path_candidate(
    root: Path,
    rel_doc: str,
    line_no: int,
    original_span: str,
    candidate: str,
    problems: list[str],
) -> None:
    # Expand brace groups like `crates/core/src/{store,events}.rs` before checking
    # template markers so `{a,b}` is expanded while `{ component }` is treated as a template.
    brace_cleaned = BRACE_GROUP_RE.sub("x", candidate)
    if TEMPLATE_MARKERS_RE.search(brace_cleaned):
        # Validate the static directory prefix before the first wildcard/template token.
        first_special = TEMPLATE_MARKERS_RE.search(candidate)
        assert first_special is not None
        static_prefix = candidate[: first_special.start()]
        if "/" in static_prefix:
            static_dir = static_prefix.rsplit("/", 1)[0]
            if static_dir and not (root / static_dir).is_dir():
                problems.append(
                    f"{rel_doc}:{line_no}: directory '{static_dir}' in template path `{original_span}` does not exist"
                )
        return

    for expanded in expand_braces(candidate):
        wants_dir = expanded.endswith("/")
        clean = expanded.rstrip("/")
        if not clean:
            continue
        target = root / clean
        if not target.exists():
            problems.append(
                f"{rel_doc}:{line_no}: referenced path `{expanded}` (from `{original_span}`) does not exist"
            )
        elif wants_dir and not target.is_dir():
            problems.append(
                f"{rel_doc}:{line_no}: referenced directory path `{expanded}` is a file, not a directory"
            )


def check_code_span_paths(
    root: Path,
    rel_doc: str,
    line_no: int,
    span: str,
    workspace: WorkspaceInfo,
    problems: list[str],
) -> None:
    stripped = span.strip()
    if not stripped:
        return

    if not any(ch.isspace() for ch in stripped):
        cleaned = _strip_token_punctuation(stripped.split("::", 1)[0])
        if (
            normalize_candidate_token(stripped, workspace) is None
            and RELATIVE_RS_SUBPATH_RE.match(cleaned)
            and cleaned not in workspace.rs_suffixes
        ):
            problems.append(
                f"{rel_doc}:{line_no}: relative Rust module path `{cleaned}` (from `{stripped}`) does not exist under any `crates/*/src/` directory"
            )
            return

    tokens = [stripped] if not any(ch.isspace() for ch in stripped) else stripped.split()
    seen: set[str] = set()
    skip_next = False
    for tok in tokens:
        if skip_next:
            skip_next = False
            continue
        if tok in ("--head", "--base", "-b"):
            skip_next = True
            continue
        candidate = normalize_candidate_token(tok, workspace)
        if candidate is not None and candidate not in seen:
            seen.add(candidate)
            verify_repo_path_candidate(
                root, rel_doc, line_no, stripped, candidate, problems
            )


def _check_quoted_mermaid_text(
    rel_doc: str, line_no: int, label_text: str, problems: list[str]
) -> None:
    if MERMAID_BAD_HASH_RE.search(label_text):
        problems.append(
            f"{rel_doc}:{line_no}: unescaped `#` inside Mermaid label `\"{label_text}\"` (use `#35;` or reword without `#`)"
        )
    html_match = MERMAID_RAW_HTML_TAG_RE.search(label_text)
    if html_match:
        problems.append(
            f"{rel_doc}:{line_no}: raw angle-bracket tag `{html_match.group(0)}` in Mermaid label `\"{label_text}\"` is stripped by GitHub's DOMPurify sanitizer"
        )


def check_flowchart_labels(
    rel_doc: str, line_no: int, line: str, problems: list[str]
) -> None:
    """Verify node and edge labels in Mermaid flowcharts are balanced and GitHub-safe."""
    code = line.split("%%", 1)[0]
    n = len(code)
    i = 0
    while i < n:
        if code[i] == '"':
            end_q = code.find('"', i + 1)
            if end_q == -1:
                problems.append(
                    f"{rel_doc}:{line_no}: unclosed double quote in Mermaid statement"
                )
                return
            _check_quoted_mermaid_text(rel_doc, line_no, code[i + 1 : end_q], problems)
            i = end_q + 1
            continue

        if code[i] == "[":
            # Quoted rectangular label `["..."]`
            if i + 1 < n and code[i + 1] == '"':
                end_q = code.find('"', i + 2)
                if end_q == -1 or end_q + 1 >= n or code[end_q + 1] != "]":
                    problems.append(
                        f"{rel_doc}:{line_no}: unclosed quoted Mermaid node label `[\"...\"` (expected closing `\"]`)"
                    )
                    return
                _check_quoted_mermaid_text(
                    rel_doc, line_no, code[i + 2 : end_q], problems
                )
                i = end_q + 2
                continue

            # Quoted composite shapes: `[("...")]`, `[["..."]]`, `[/"..."/]`, `[\"..."\]`
            if i + 2 < n and code[i + 1] in ("(", "[", "/", "\\") and code[i + 2] == '"':
                opener = code[i + 1]
                expected_inner_close = {
                    "(": ")",
                    "[": "]",
                    "/": "/",
                    "\\": "\\",
                }[opener]
                end_q = code.find('"', i + 3)
                if (
                    end_q == -1
                    or end_q + 2 >= n
                    or code[end_q + 1] not in (expected_inner_close, "/", "\\")
                    or code[end_q + 2] != "]"
                ):
                    problems.append(
                        f"{rel_doc}:{line_no}: unclosed composite Mermaid node label starting with `[{opener}\"`"
                    )
                    return
                _check_quoted_mermaid_text(
                    rel_doc, line_no, code[i + 3 : end_q], problems
                )
                i = end_q + 3
                continue

            # Unquoted subroutine `[[...]]`
            if i + 1 < n and code[i + 1] == "[":
                end_sub = code.find("]]", i + 2)
                if end_sub == -1:
                    problems.append(
                        f"{rel_doc}:{line_no}: unclosed `[[` in Mermaid subroutine node"
                    )
                    return
                inner = code[i + 2 : end_sub]
                if "(" in inner or ")" in inner:
                    problems.append(
                        f'{rel_doc}:{line_no}: unquoted parentheses inside Mermaid label `[[{inner}]]`; wrap in quotes as `[["{inner}"]]`'
                    )
                i = end_sub + 2
                continue

            # Unquoted cylinder `[(...)]`
            if i + 1 < n and code[i + 1] == "(":
                end_cyl = code.find(")]", i + 2)
                if end_cyl == -1:
                    problems.append(
                        f"{rel_doc}:{line_no}: unclosed `[(` in Mermaid cylinder node"
                    )
                    return
                inner = code[i + 2 : end_cyl]
                if "(" in inner or ")" in inner:
                    problems.append(
                        f'{rel_doc}:{line_no}: unquoted parentheses inside Mermaid cylinder label `[({inner})]`; wrap in quotes as `[("{inner}")]`'
                    )
                i = end_cyl + 2
                continue

            # Unquoted label `[...]`
            end_b = code.find("]", i + 1)
            if end_b == -1:
                problems.append(
                    f"{rel_doc}:{line_no}: unclosed `[` in Mermaid node definition"
                )
                return
            content = code[i + 1 : end_b]
            if "(" in content or ")" in content:
                problems.append(
                    f'{rel_doc}:{line_no}: unquoted parentheses inside Mermaid label `[{content}]`; wrap in quotes as `["{content}"]`'
                )
            html_match = MERMAID_RAW_HTML_TAG_RE.search(content)
            if html_match:
                problems.append(
                    f"{rel_doc}:{line_no}: raw angle-bracket tag `{html_match.group(0)}` in Mermaid label `[{content}]` is stripped by GitHub's DOMPurify sanitizer"
                )
            i = end_b + 1
            continue

        # Rounded or stadium node `NodeId(...)` outside quotes/brackets
        if code[i] == "(" and i > 0 and (code[i - 1].isalnum() or code[i - 1] in ("_", "-")):
            # Quoted stadium `(["..."])` or quoted double-circle `(("..."))` or quoted rounded `("...")`
            if i + 1 < n and code[i + 1] == '"':
                end_q = code.find('"', i + 2)
                if end_q == -1 or end_q + 1 >= n or code[end_q + 1] != ")":
                    problems.append(
                        f'{rel_doc}:{line_no}: unclosed quoted Mermaid rounded node `("..."` (expected `")`)'
                    )
                    return
                _check_quoted_mermaid_text(
                    rel_doc, line_no, code[i + 2 : end_q], problems
                )
                i = end_q + 2
                continue
            if i + 2 < n and code[i + 1] in ("[", "(") and code[i + 2] == '"':
                inner_close = "]" if code[i + 1] == "[" else ")"
                end_q = code.find('"', i + 3)
                if (
                    end_q == -1
                    or end_q + 2 >= n
                    or code[end_q + 1] != inner_close
                    or code[end_q + 2] != ")"
                ):
                    problems.append(
                        f'{rel_doc}:{line_no}: unclosed composite Mermaid node starting with `({code[i + 1]}"`'
                    )
                    return
                _check_quoted_mermaid_text(
                    rel_doc, line_no, code[i + 3 : end_q], problems
                )
                i = end_q + 3
                continue

            # Unquoted `NodeId(...)` - scan balanced parens and reject nested `(` / `)`
            if not (i + 1 < n and code[i + 1] in ("[", "(")):
                depth = 1
                k = i + 1
                has_nested_paren = False
                while k < n and depth > 0:
                    if code[k] == "(":
                        has_nested_paren = True
                        depth += 1
                    elif code[k] == ")":
                        depth -= 1
                        if depth == 0:
                            break
                    k += 1
                if depth > 0:
                    problems.append(
                        f"{rel_doc}:{line_no}: unclosed `(` in Mermaid rounded node"
                    )
                    return
                if has_nested_paren:
                    inner = code[i + 1 : k]
                    problems.append(
                        f'{rel_doc}:{line_no}: unquoted parentheses inside Mermaid rounded node `({inner})`; wrap in quotes as `("{inner}")`'
                    )
                i = k + 1
                continue

        i += 1


def check_mermaid_block(
    root: Path,
    rel_doc: str,
    open_line: int,
    block_lines: list[tuple[int, str]],
    workspace: WorkspaceInfo,
    problems: list[str],
) -> None:
    active = [
        (ln, raw.strip())
        for ln, raw in block_lines
        if raw.strip() and not raw.strip().startswith("%%")
    ]
    if not active:
        problems.append(f"{rel_doc}:{open_line}: empty mermaid code block")
        return

    first_ln, header = active[0]
    is_flowchart = FLOWCHART_HEADER_RE.match(header) is not None
    is_other = OTHER_MERMAID_HEADER_RE.match(header) is not None
    if not is_flowchart and not is_other:
        problems.append(
            f"{rel_doc}:{first_ln}: unsupported or invalid Mermaid diagram header '{header}'"
        )
        return

    is_sequence = header.startswith("sequenceDiagram")
    depth = 0
    keyword_name = "subgraph" if is_flowchart else "sequence block"

    for ln, stmt in active[1:]:
        stmt_no_comment = stmt.split("%%", 1)[0]
        if is_flowchart:
            if re.match(r"^subgraph\b", stmt_no_comment):
                depth += 1
            elif re.match(r"^end\s*$", stmt_no_comment):
                depth -= 1
                if depth < 0:
                    problems.append(
                        f"{rel_doc}:{ln}: unmatched `end` in Mermaid flowchart"
                    )
                    depth = 0
            check_flowchart_labels(rel_doc, ln, stmt_no_comment, problems)
        elif is_sequence:
            if SEQUENCE_BLOCK_OPEN_RE.match(stmt_no_comment):
                depth += 1
            elif re.match(r"^end\s*$", stmt_no_comment):
                depth -= 1
                if depth < 0:
                    problems.append(
                        f"{rel_doc}:{ln}: unmatched `end` in Mermaid sequenceDiagram"
                    )
                    depth = 0
            html_match = MERMAID_RAW_HTML_TAG_RE.search(stmt_no_comment)
            if html_match:
                problems.append(
                    f"{rel_doc}:{ln}: raw angle-bracket tag `{html_match.group(0)}` in Mermaid sequenceDiagram statement"
                )

    if depth != 0:
        problems.append(
            f"{rel_doc}:{open_line}: Mermaid block has {depth} unclosed {keyword_name}(s) (missing `end`)"
        )


def check_fenced_block_paths(
    root: Path,
    rel_doc: str,
    block_lines: list[tuple[int, str]],
    workspace: WorkspaceInfo,
    problems: list[str],
) -> None:
    """Validate explicit repo paths inside fenced blocks (Mermaid, XML, Bash, Rust, Markdown)."""
    for ln, raw in block_lines:
        seen: set[str] = set()
        for match in FENCED_REPO_PATH_RE.finditer(raw):
            candidate = normalize_candidate_token(match.group(0), workspace)
            if candidate is not None and candidate not in seen:
                seen.add(candidate)
                verify_repo_path_candidate(
                    root, rel_doc, ln, match.group(0), candidate, problems
                )


def parse_simple_frontmatter(
    rel_doc: str, text: str, problems: list[str]
) -> dict[str, str] | None:
    """Parse YAML frontmatter supporting single-line scalars and `|` / `>` block scalars."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        problems.append(f"{rel_doc}:1: missing opening `---` YAML frontmatter delimiter")
        return None

    end_idx: int | None = None
    for idx in range(1, len(lines)):
        if lines[idx].strip() == "---":
            end_idx = idx
            break

    if end_idx is None:
        problems.append(f"{rel_doc}:1: unclosed YAML frontmatter (missing closing `---`)")
        return None

    fields: dict[str, str] = {}
    idx = 1
    while idx < end_idx:
        raw = lines[idx]
        line_no = idx + 1
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            idx += 1
            continue
        if raw[0].isspace() or ":" not in raw:
            problems.append(
                f"{rel_doc}:{line_no}: invalid YAML frontmatter line '{raw}'"
            )
            idx += 1
            continue

        key, value = raw.split(":", 1)
        key = key.strip()
        value = value.strip()
        if not key:
            problems.append(f"{rel_doc}:{line_no}: empty YAML frontmatter key")
            idx += 1
            continue
        if key in fields:
            problems.append(
                f"{rel_doc}:{line_no}: duplicate YAML frontmatter key '{key}'"
            )
            idx += 1
            continue

        # Support literal (`|`) and folded (`>`) multiline block scalars, or empty key followed by indented lines.
        if YAML_BLOCK_SCALAR_RE.match(value) or value == "":
            fold = value.startswith(">") or value == ""
            cont_lines: list[str] = []
            next_idx = idx + 1
            base_indent: int | None = None
            while next_idx < end_idx:
                cand = lines[next_idx]
                if not cand.strip():
                    cont_lines.append("")
                    next_idx += 1
                    continue
                indent = len(cand) - len(cand.lstrip(" "))
                if indent == 0:
                    break
                if base_indent is None:
                    base_indent = indent
                cont_lines.append(cand[min(indent, base_indent) :].rstrip())
                next_idx += 1
            if fold:
                joined = " ".join(part.strip() for part in cont_lines if part.strip())
            else:
                joined = "\n".join(cont_lines).strip()
            fields[key] = joined
            idx = next_idx
            continue

        if len(value) >= 2 and value[0] == value[-1] and value[0] in ("'", '"'):
            value = value[1:-1]

        # Also fold indented plain-scalar continuation lines if present.
        cont_parts = [value]
        next_idx = idx + 1
        while next_idx < end_idx:
            cand = lines[next_idx]
            if cand.strip() and cand[0].isspace() and not cand.lstrip().startswith("#"):
                cont_parts.append(cand.strip())
                next_idx += 1
            else:
                break
        fields[key] = " ".join(cont_parts)
        idx = next_idx

    return fields


def _mask_inline_backticks(line: str) -> str:
    out: list[str] = []
    n = len(line)
    i = 0
    while i < n:
        if line[i] == "\\":
            out.append(line[i : i + 2])
            i += 2
            continue
        if line[i] == "`":
            ticks = 1
            while i + ticks < n and line[i + ticks] == "`":
                ticks += 1
            end = _find_closing_backticks(line, i + ticks, ticks)
            if end != -1:
                out.append("[inline-code]")
                i = end + ticks
                continue
            out.append("`" * ticks)
            i += ticks
            continue
        out.append(line[i])
        i += 1
    return "".join(out)


def mask_fenced_blocks_as_placeholders(text: str) -> str:
    """Replace fenced and inline code spans with non-empty tokens while preserving XML tags in prose."""
    out_lines: list[str] = []
    in_fence = False
    fence_char = ""
    fence_len = 0

    for line in text.splitlines():
        if not in_fence:
            match = FENCE_OPEN_RE.match(line)
            if match and not (match.group(1)[0] == "`" and "`" in match.group(2)):
                in_fence = True
                fence_char = match.group(1)[0]
                fence_len = len(match.group(1))
                out_lines.append("[fenced-code-block]")
            else:
                out_lines.append(_mask_inline_backticks(line))
        else:
            stripped = line.lstrip(" ")
            leading_spaces = len(line) - len(stripped)
            if (
                leading_spaces <= 3
                and stripped.startswith(fence_char * fence_len)
                and stripped.rstrip(" \t") == fence_char * len(stripped.rstrip(" \t"))
            ):
                in_fence = False
    return "\n".join(out_lines)


def check_skill_examples(rel_doc: str, text: str, problems: list[str]) -> None:
    joined = mask_fenced_blocks_as_placeholders(text)
    opens = len(re.findall(r"<example>", joined))
    closes = len(re.findall(r"</example>", joined))
    if opens == 0:
        problems.append(
            f"{rel_doc}:1: skill must include at least one `<example><input>...</input><output>...</output></example>` block"
        )
        return
    if opens != closes:
        problems.append(
            f"{rel_doc}:1: unbalanced `<example>` tags ({opens} opening vs {closes} closing)"
        )
        return

    blocks = re.findall(r"<example>(.*?)</example>", joined, flags=re.DOTALL)
    if len(blocks) != opens or any("<example>" in b for b in blocks):
        problems.append(
            f"{rel_doc}:1: nested or misordered `<example>` tags are not allowed"
        )
        return

    for idx, block in enumerate(blocks, start=1):
        in_opens = len(re.findall(r"<input>", block))
        in_closes = len(re.findall(r"</input>", block))
        out_opens = len(re.findall(r"<output>", block))
        out_closes = len(re.findall(r"</output>", block))
        in_matches = re.findall(r"<input>(.*?)</input>", block, flags=re.DOTALL)
        out_matches = re.findall(r"<output>(.*?)</output>", block, flags=re.DOTALL)
        if (
            in_opens != 1
            or in_closes != 1
            or len(in_matches) != 1
            or not in_matches[0].strip()
        ):
            problems.append(
                f"{rel_doc}:1: `<example>` #{idx} must contain exactly one non-empty `<input>...</input>` block"
            )
        if (
            out_opens != 1
            or out_closes != 1
            or len(out_matches) != 1
            or not out_matches[0].strip()
        ):
            problems.append(
                f"{rel_doc}:1: `<example>` #{idx} must contain exactly one non-empty `<output>...</output>` block"
            )


def check_skills(
    root: Path, agents_links: set[str], problems: list[str]
) -> list[Path]:
    skills_root = root / ".agents" / "skills"
    if not skills_root.is_dir():
        problems.append(".agents/skills: missing agent skills directory")
        return []

    skill_files: list[Path] = []
    for entry in sorted(skills_root.iterdir()):
        if not entry.is_dir():
            continue
        dir_name = entry.name
        if not SKILL_DIR_RE.match(dir_name):
            problems.append(
                f".agents/skills/{dir_name}: skill directory must be lower-case kebab-case"
            )
        skill_md = entry / "SKILL.md"
        if not skill_md.is_file():
            problems.append(f".agents/skills/{dir_name}: missing SKILL.md")
            continue

        skill_files.append(skill_md)
        rel_skill = skill_md.relative_to(root).as_posix()
        text = skill_md.read_text(encoding="utf-8")
        fields = parse_simple_frontmatter(rel_skill, text, problems)
        if fields is not None:
            name = fields.get("name", "")
            desc = fields.get("description", "")
            if name != dir_name:
                problems.append(
                    f"{rel_skill}:1: frontmatter `name` is '{name}', expected '{dir_name}'"
                )
            if len(desc) < 20:
                problems.append(
                    f"{rel_skill}:1: frontmatter `description` must be at least 20 characters"
                )

        check_skill_examples(rel_skill, text, problems)

        if rel_skill not in agents_links:
            problems.append(
                f"AGENTS.md: missing on-demand routing link to `{rel_skill}`"
            )

    if not skill_files:
        problems.append(".agents/skills: expected at least one skill directory")

    return skill_files


def check_workspace_crate_coverage(
    root: Path, workspace: WorkspaceInfo, problems: list[str]
) -> None:
    arch_path = root / "docs" / "ARCHITECTURE.md"
    graphs_path = root / "docs" / "GRAPHS.md"
    if not arch_path.is_file() or not graphs_path.is_file():
        return

    arch_non_fenced, _ = iter_markdown_structure(arch_path.read_text(encoding="utf-8"))
    arch_table_pairs: dict[str, tuple[int, str]] = {}
    for line_no, line in arch_non_fenced:
        row_match = ARCH_CRATE_TABLE_ROW_RE.match(line.strip())
        if row_match:
            row_dir, row_pkg = row_match.group(1), row_match.group(2)
            arch_table_pairs[row_dir] = (line_no, row_pkg)
            expected_pkg = workspace.all_crate_pkgs.get(row_dir)
            if expected_pkg is not None and expected_pkg != row_pkg:
                problems.append(
                    f"docs/ARCHITECTURE.md:{line_no}: crate table maps `{row_dir}` to `{row_pkg}`, expected `{expected_pkg}`"
                )

    graphs_text = graphs_path.read_text(encoding="utf-8")
    _, graphs_fences = iter_markdown_structure(graphs_text)
    mermaid_blocks = [
        (open_ln, "\n".join(line for _, line in lines))
        for open_ln, info, lines in graphs_fences
        if info == "mermaid"
    ]
    if len(mermaid_blocks) < MIN_GRAPHS_MERMAID_BLOCKS:
        problems.append(
            f"docs/GRAPHS.md: expected at least {MIN_GRAPHS_MERMAID_BLOCKS} Mermaid blocks, found {len(mermaid_blocks)}"
        )
    if not mermaid_blocks:
        return

    graph1_line, graph1 = mermaid_blocks[0]
    active_dirs = {rel_dir for rel_dir, _ in workspace.active_members}

    for rel_dir, pkg_name in workspace.active_members:
        table_entry = arch_table_pairs.get(rel_dir)
        if table_entry is None or table_entry[1] != pkg_name:
            problems.append(
                f"docs/ARCHITECTURE.md: active workspace crate `{pkg_name}` (`{rel_dir}`) is missing from the crate map table"
            )

        node_pair_re = re.compile(
            rf"(?<![a-zA-Z0-9_-]){re.escape(pkg_name)}(?![a-zA-Z0-9_-])[^\"\n]*\(\s*{re.escape(rel_dir)}\s*\)"
        )
        if not node_pair_re.search(graph1):
            problems.append(
                f"docs/GRAPHS.md:{graph1_line}: active workspace crate `{pkg_name}` (`{rel_dir}`) is missing from Graph 1"
            )

    for mentioned_dir in sorted(set(re.findall(r"\bcrates/[a-zA-Z0-9_-]+\b", graph1))):
        if mentioned_dir not in active_dirs:
            problems.append(
                f"docs/GRAPHS.md:{graph1_line}: `{mentioned_dir}` in Graph 1 is not an active `[workspace]` crate in `Cargo.toml`"
            )


def check_repo(root: Path) -> list[str]:
    """Validate ``root`` and return every problem found (empty when clean)."""
    problems: list[str] = []
    slug_cache: dict[Path, set[str]] = {}

    for rel in REQUIRED_DOCS:
        if not (root / rel).is_file():
            problems.append(f"{rel}: required documentation file is missing")

    agents_path = root / "AGENTS.md"
    agents_links_resolved: set[str] = set()
    if agents_path.is_file():
        agents_size = agents_path.stat().st_size
        if agents_size > AGENTS_MAX_BYTES:
            problems.append(
                f"AGENTS.md: {agents_size} bytes exceeds {AGENTS_MAX_BYTES}-byte context budget"
            )

    copilot_path = root / ".github" / "copilot-instructions.md"
    if copilot_path.is_file():
        copilot_size = copilot_path.stat().st_size
        if copilot_size > COPILOT_MAX_BYTES:
            problems.append(
                f".github/copilot-instructions.md: {copilot_size} bytes exceeds {COPILOT_MAX_BYTES}-byte budget"
            )

    workspace = load_workspace_info(root, problems)
    check_workspace_crate_coverage(root, workspace, problems)

    # Collect AGENTS.md links first so skill routing coverage can verify them.
    if agents_path.is_file():
        agents_non_fenced, _ = iter_markdown_structure(
            agents_path.read_text(encoding="utf-8")
        )
        for _, line in agents_non_fenced:
            _, links = scan_line_spans_and_links(line)
            for link in links:
                if not link.startswith(("http://", "https://", "mailto:", "#")):
                    part = link.split("#", 1)[0].split("?", 1)[0]
                    if part:
                        resolved = (agents_path.parent / part).resolve()
                        try:
                            agents_links_resolved.add(
                                resolved.relative_to(root.resolve()).as_posix()
                            )
                        except ValueError:
                            pass

    skill_files = check_skills(root, agents_links_resolved, problems)

    # Gather all Markdown files for relative-link + Mermaid checks.
    docs_dir = root / "docs"
    doc_md_files = sorted(docs_dir.glob("*.md")) if docs_dir.is_dir() else []
    scanned_md_files: list[Path] = []
    for candidate in (
        agents_path,
        root / "SECURITY.md",
        copilot_path,
        *doc_md_files,
        *skill_files,
    ):
        if candidate.is_file() and candidate not in scanned_md_files:
            scanned_md_files.append(candidate)

    normative_set = {(root / rel).resolve() for rel in NORMATIVE_PATH_DOCS} | {
        sf.resolve() for sf in skill_files
    }

    for md_file in scanned_md_files:
        rel_doc = md_file.relative_to(root).as_posix()
        text = md_file.read_text(encoding="utf-8")
        non_fenced, fenced_blocks = iter_markdown_structure(text, rel_doc, problems)
        check_paths = md_file.resolve() in normative_set

        for line_no, line in non_fenced:
            code_spans, links = scan_line_spans_and_links(line)
            for link in links:
                check_relative_link(root, md_file, line_no, link, slug_cache, problems)
            if check_paths:
                for span in code_spans:
                    check_code_span_paths(
                        root, rel_doc, line_no, span, workspace, problems
                    )

        for open_line, info, block_lines in fenced_blocks:
            if info == "mermaid":
                check_mermaid_block(
                    root, rel_doc, open_line, block_lines, workspace, problems
                )
            if check_paths:
                check_fenced_block_paths(
                    root, rel_doc, block_lines, workspace, problems
                )

    return problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="Repository root directory (defaults to two levels above this script).",
    )
    args = parser.parse_args(argv)
    root = args.root.resolve()

    problems = check_repo(root)
    prefix = "::error::" if os.environ.get("GITHUB_ACTIONS") else "error: "
    for problem in problems:
        print(f"{prefix}{problem}")

    if not problems:
        print(
            f"OK: agent docs, Mermaid graphs, skills, and crate paths verified in {root}"
        )
        return 0
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
