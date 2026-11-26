"""Tests for check_agent_docs.py: python3 -m unittest discover -s .github/scripts -p "test_*.py"."""

from __future__ import annotations

import contextlib
import io
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

from check_agent_docs import (  # noqa: E402
    AGENTS_MAX_BYTES,
    COPILOT_MAX_BYTES,
    check_repo,
    main,
    scan_line_spans_and_links,
)

REPO_ROOT = SCRIPT_DIR.parents[1]


def _write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def _build_minimal_valid_repo(root: Path) -> None:
    """Create a minimal synthetic workspace passing every check in check_repo."""
    _write(
        root / "Cargo.toml",
        '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\nexclude = ["crates/vendored"]\n',
    )
    _write(
        root / "crates/alpha/Cargo.toml",
        '[package]\nname = "irori-alpha"\nversion = "0.1.0"\nedition = "2024"\n',
    )
    _write(root / "crates/alpha/src/lib.rs", "//! Alpha crate.\n")
    _write(root / "crates/alpha/src/settings.rs", "//! Settings facade.\n")
    _write(root / "crates/alpha/src/window.rs", "pub fn helper() {}\n")
    _write(root / "crates/alpha/src/view/editor/mod.rs", "//! Editor module.\n")
    _write(root / "crates/alpha/locales/en.json", '{"alpha.ok": "OK"}\n')

    _write(
        root / "crates/vendored/Cargo.toml",
        '[package]\nname = "vendored-patch"\nversion = "0.1.0"\nedition = "2024"\n',
    )
    _write(root / "crates/vendored/src/lib.rs", "//! Excluded patch crate.\n")

    _write(
        root / ".github/scripts/check_agent_docs.py",
        "# placeholder script for path check\n",
    )
    _write(
        root / ".github/copilot-instructions.md",
        "# Copilot\n\nRead [AGENTS.md](../AGENTS.md) and [docs/GRAPHS.md](../docs/GRAPHS.md).\n",
    )
    _write(
        root / "SECURITY.md",
        "# Security\n\nSee [`crates/alpha`](crates/alpha/src/lib.rs).\n",
    )
    _write(
        root / "AGENTS.md",
        "\n".join(
            [
                "# AGENTS.md",
                "",
                "- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)",
                "- [docs/GRAPHS.md](docs/GRAPHS.md)",
                "- [docs/UI_CONVENTIONS.md](docs/UI_CONVENTIONS.md)",
                "- [docs/PERFORMANCE.md](docs/PERFORMANCE.md)",
                "- [docs/TRANSLATING.md](docs/TRANSLATING.md)",
                "- [docs/PROMPTING_PLAYBOOK.md](docs/PROMPTING_PLAYBOOK.md)",
                "- [SECURITY.md](SECURITY.md)",
                "- [.agents/skills/sample-skill/SKILL.md](.agents/skills/sample-skill/SKILL.md)",
                "",
                "Run `python3 .github/scripts/check_agent_docs.py --head docs/sample-branch`.",
            ]
        )
        + "\n",
    )
    _write(
        root / "docs/ARCHITECTURE.md",
        "\n".join(
            [
                "# Architecture",
                "",
                "| Directory | Package |",
                "| --- | --- |",
                "| `crates/alpha` | `irori-alpha` |",
                "",
                "Core files: `crates/alpha/src/{lib,settings}.rs` and `crates/alpha/src/window.rs::helper`.",
                "Branch example: `docs/agents-md`. Feature example: `irori-alpha/test-server`.",
            ]
        )
        + "\n",
    )

    five_graphs = "\n\n".join(
        [
            '```mermaid\nflowchart TD\n    subgraph Layer["Core (workspace)"]\n        A["irori-alpha (crates/alpha)"]\n    end\n```',
            "```mermaid\nsequenceDiagram\n    participant UI as View\n    participant B as Backend\n    alt Ready\n        UI->>B: run\n    end\n```",
            '```mermaid\nflowchart LR\n    Start(["Launch (Wine)"]) --> Db[("SQLite (cache)")]\n```',
            '```mermaid\nflowchart LR\n    Q["Queue"] --> W[["Worker (io)"]]\n```',
            '```mermaid\nflowchart LR\n    F["crates/alpha/src/settings.rs"] --> S["crates/alpha/src/window.rs"]\n```',
        ]
    )
    _write(root / "docs/GRAPHS.md", f"# Graphs\n\n{five_graphs}\n")
    _write(
        root / "docs/UI_CONVENTIONS.md",
        "# UI\n\nUses `irori-alpha/src/lib.rs`, `view/editor/mod.rs`, and `[ignored](not-a-link.md)`.\n\n```rust\nlet x = arr[0](call);\n```\n",
    )
    _write(
        root / "docs/PERFORMANCE.md",
        "# Performance\n\nSee [Graphs](GRAPHS.md#graphs) and [Performance](#performance).\n",
    )
    _write(
        root / "docs/TRANSLATING.md",
        "# Translating\n\nCatalogs match `crates/alpha/locales/*.json`.\n",
    )
    _write(
        root / "docs/PROMPTING_PLAYBOOK.md",
        "\n".join(
            [
                "# Playbook",
                "",
                "Check `AGENTS.md`.",
                "",
                "```xml",
                "<context>",
                "Read crates/alpha/src/settings.rs.",
                "</context>",
                "```",
            ]
        )
        + "\n",
    )
    _write(
        root / ".agents/skills/sample-skill/SKILL.md",
        "\n".join(
            [
                "---",
                "name: sample-skill",
                "description: >",
                "  Split modules cleanly and run offline",
                "  verification checks.",
                "---",
                "",
                "# Sample Skill",
                "",
                "Include an `<example>` block as shown below.",
                "",
                "<example>",
                "<input>",
                "Check `crates/alpha/src/settings.rs`.",
                "</input>",
                "<output>",
                "```rust",
                "pub mod sub;",
                "```",
                "</output>",
                "</example>",
            ]
        )
        + "\n",
    )


class CheckAgentDocsTests(unittest.TestCase):
    def test_live_repository_passes(self) -> None:
        problems = check_repo(REPO_ROOT)
        self.assertEqual(problems, [], "\n".join(problems))

    def test_minimal_synthetic_repo_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)
            self.assertEqual(check_repo(root), [])

    def test_cli_main_exit_codes_and_github_annotations(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                rc = main(["--root", str(root)])
            self.assertEqual(rc, 0)
            self.assertIn("OK:", out.getvalue())

            (root / "docs/GRAPHS.md").unlink()
            err_out = io.StringIO()
            with mock.patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}):
                with contextlib.redirect_stdout(err_out):
                    rc_fail = main(["--root", str(root)])
            self.assertEqual(rc_fail, 1)
            self.assertIn("::error::docs/GRAPHS.md", err_out.getvalue())

    def test_enforces_agents_and_copilot_byte_budgets(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            agents = root / "AGENTS.md"
            base = agents.read_text(encoding="utf-8")
            _write(agents, base + ("a" * (AGENTS_MAX_BYTES - len(base.encode("utf-8")))))
            self.assertEqual(check_repo(root), [])

            _write(agents, agents.read_text(encoding="utf-8") + "x")
            problems = check_repo(root)
            self.assertTrue(any("AGENTS.md:" in p and "budget" in p for p in problems), problems)

            _write(agents, base)
            copilot = root / ".github/copilot-instructions.md"
            _write(copilot, copilot.read_text(encoding="utf-8") + ("c" * COPILOT_MAX_BYTES))
            problems = check_repo(root)
            self.assertTrue(
                any("copilot-instructions.md" in p and "budget" in p for p in problems),
                problems,
            )

    def test_detects_stale_path_in_inline_mermaid_fenced_and_relative_rs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            # Simulate moving `settings.rs` -> `settings/mod.rs` and `view/editor.rs` -> `view/editor/mod.rs`
            (root / "crates/alpha/src/settings.rs").unlink()
            _write(root / "crates/alpha/src/settings/mod.rs", "//! Moved to mod.rs.\n")
            _write(
                root / "docs/UI_CONVENTIONS.md",
                "# UI\n\nStale bare subpath `view/editor.rs`.\n",
            )

            problems = check_repo(root)
            joined = "\n".join(problems)
            self.assertIn("docs/ARCHITECTURE.md", joined)
            self.assertIn("docs/GRAPHS.md", joined)
            self.assertIn("docs/PROMPTING_PLAYBOOK.md", joined)
            self.assertIn("SKILL.md", joined)
            self.assertIn("relative Rust module path `view/editor.rs`", joined)

    def test_facade_submodule_splits_and_removed_crates(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            # Simulate PRs #258/#259/#260: `<name>.rs` stays as thin facade + `<name>/*.rs` added.
            _write(root / "crates/alpha/src/settings.rs", "mod app;\npub use app::*;\n")
            _write(root / "crates/alpha/src/settings/app.rs", "pub struct AppSettings;\n")
            self.assertEqual(check_repo(root), [])

            # Simulate PR #226: a removed crate directory still cited in docs fails immediately.
            _write(
                root / "docs/UI_CONVENTIONS.md",
                "# UI\n\nLegacy reference to `crates/webview`.\n",
            )
            problems = check_repo(root)
            self.assertEqual(len(problems), 1, problems)
            self.assertIn("crates/webview", problems[0])

    def test_bidirectional_workspace_crate_table_and_graph1_parity(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            # 1. Deleting the crate table row in ARCHITECTURE.md must fail even if
            # `crates/alpha` and `irori-alpha` still appear in prose.
            _write(
                root / "docs/ARCHITECTURE.md",
                "# Architecture\n\nProse mentions `crates/alpha` and `irori-alpha` without a table row.\n",
            )
            problems = check_repo(root)
            self.assertTrue(
                any("missing from the crate map table" in p for p in problems),
                problems,
            )

            # Restore ARCHITECTURE.md and verify Graph 1 specifically requires active crates
            # and rejects excluded crates (`crates/vendored`).
            _build_minimal_valid_repo(root)
            bad_graphs = (root / "docs/GRAPHS.md").read_text(encoding="utf-8").replace(
                'A["irori-alpha (crates/alpha)"]',
                'V["vendored-patch (crates/vendored)"]',
            )
            _write(root / "docs/GRAPHS.md", bad_graphs)
            problems = check_repo(root)
            joined = "\n".join(problems)
            self.assertIn("`irori-alpha` (`crates/alpha`) is missing from Graph 1", joined)
            self.assertIn("`crates/vendored` in Graph 1 is not an active `[workspace]` crate", joined)

    def test_detects_new_workspace_crate_missing_from_docs_and_graphs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            _write(
                root / "crates/beta/Cargo.toml",
                '[package]\nname = "irori-beta"\nversion = "0.1.0"\nedition = "2024"\n',
            )
            _write(root / "crates/beta/src/lib.rs", "//! Beta crate.\n")

            problems = check_repo(root)
            self.assertTrue(
                any("docs/ARCHITECTURE.md" in p and "irori-beta" in p for p in problems),
                problems,
            )
            self.assertTrue(
                any("docs/GRAPHS.md" in p and "irori-beta" in p for p in problems),
                problems,
            )

    def test_relative_links_anchors_package_aliases_and_commonmark_backticks(self) -> None:
        spans, links = scan_line_spans_and_links(
            "Check `` `crates/alpha/src/lib.rs` `` and `a `` b` plus [`irori-alpha/src/window.rs`](ARCHITECTURE.md#architecture)."
        )
        self.assertEqual(
            spans,
            ["`crates/alpha/src/lib.rs`", "a `` b", "irori-alpha/src/window.rs"],
        )
        self.assertEqual(links, ["ARCHITECTURE.md#architecture"])

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            _write(
                root / "docs/PERFORMANCE.md",
                "\n".join(
                    [
                        "# Performance",
                        "",
                        "- Broken doc: [Missing](DOES_NOT_EXIST.md)",
                        "- Broken local anchor: [BadSelf](#no-such-heading)",
                        "- Broken remote anchor: [BadCross](GRAPHS.md#no-such-section)",
                        "- Local URI: [Local](file:///tmp/notes.md)",
                        "- Escapes root: [Escape](../../outside.md)",
                        "- Broken alias: `irori-alpha/src/missing.rs`",
                        "- Broken template dir: `crates/ghost/locales/*.json`",
                        "- Broken docs file: `docs/missing-guide.md`",
                    ]
                )
                + "\n",
            )
            problems = check_repo(root)
            joined = "\n".join(problems)
            self.assertIn("broken relative link 'DOES_NOT_EXIST.md'", joined)
            self.assertIn("broken heading anchor '#no-such-heading'", joined)
            self.assertIn("broken heading anchor '#no-such-section'", joined)
            self.assertIn("file://", joined)
            self.assertIn("escapes repository root", joined)
            self.assertIn("crates/alpha/src/missing.rs", joined)
            self.assertIn("crates/ghost/locales", joined)
            self.assertIn("docs/missing-guide.md", joined)

    def test_mermaid_validation_catches_syntax_and_dompurify_errors(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            _write(
                root / "docs/PERFORMANCE.md",
                "\n".join(
                    [
                        "# Performance",
                        "",
                        "```mermaid",
                        "flowchart TD",
                        "    subgraph Unclosed",
                        "        Bad[Backend (Tokio)]",
                        "        Round(Worker (io))",
                        '        Html["crates/<crate>/src/<area>.rs"]',
                        '        Hash["tests.rs (#[cfg(test)] unit tests)"]',
                        "```",
                        "",
                        "```mermaid",
                        "pie title InvalidHeader",
                        '    "A" : 10',
                        "```",
                    ]
                )
                + "\n",
            )
            problems = check_repo(root)
            joined = "\n".join(problems)
            self.assertIn("unquoted parentheses inside Mermaid label `[Backend (Tokio)]`", joined)
            self.assertIn("unquoted parentheses inside Mermaid rounded node `(Worker (io))`", joined)
            self.assertIn("raw angle-bracket tag `<crate>`", joined)
            self.assertIn("unescaped `#` inside Mermaid label", joined)
            self.assertIn("unclosed subgraph", joined)
            self.assertIn("unsupported or invalid Mermaid diagram header", joined)

    def test_skill_frontmatter_multiline_and_nested_examples_validation(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _build_minimal_valid_repo(root)

            _write(
                root / ".agents/skills/broken-skill/SKILL.md",
                "\n".join(
                    [
                        "---",
                        "name: wrong-name",
                        "description: too short",
                        "---",
                        "",
                        "# Broken Skill",
                        "",
                        "Only inline `<example><input>x</input><output>y</output></example>` in backticks.",
                    ]
                )
                + "\n",
            )
            _write(
                root / ".agents/skills/nested-skill/SKILL.md",
                "\n".join(
                    [
                        "---",
                        "name: nested-skill",
                        "description: |",
                        "  Multiline literal block description",
                        "  spanning two lines cleanly.",
                        "---",
                        "",
                        "<example><input>a</input><example><output>b</output></example></example>",
                    ]
                )
                + "\n",
            )
            problems = check_repo(root)
            joined = "\n".join(problems)
            self.assertIn("expected 'broken-skill'", joined)
            self.assertIn("at least 20 characters", joined)
            self.assertIn("must include at least one `<example>", joined)
            self.assertIn("nested or misordered `<example>` tags", joined)
            self.assertIn("AGENTS.md: missing on-demand routing link to `.agents/skills/broken-skill/SKILL.md`", joined)


if __name__ == "__main__":
    unittest.main()
