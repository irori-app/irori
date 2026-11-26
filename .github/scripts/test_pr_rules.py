"""Tests for pr_rules.py: python3 -m unittest discover -s .github/scripts"""

from contextlib import redirect_stdout
import io
from pathlib import Path
import tempfile
import unittest

from pr_rules import (
    LARGE_PR_LABEL,
    LARGE_RS_WARN_LINES,
    RS_ADDED_MAX,
    check,
    check_body,
    check_large_rs_files,
    main,
    rs_added_from_numstat,
)

VALID_BODY = """## Plain-language summary

Fixes an issue where scrolling the library left the filter menu floating in place.

## Technical details

Updated `crates/feature-library/src/filters_panel.rs` to dismiss on virtual list scroll events.
"""


class PullRequestRules(unittest.TestCase):
    def ok(self, title, head, base="main", **kwargs):
        self.assertEqual(check(title, head, base, **kwargs), [], (title, head, base, kwargs))

    def broken(self, title, head, base="main", count=1, **kwargs):
        problems = check(title, head, base, **kwargs)
        self.assertEqual(len(problems), count, problems)

    def test_conventional_names_pass(self):
        self.ok("fix(sign-in): open the window without the helper", "fix/in-app-sign-in")
        self.ok("feat: add a login command", "feat/cli-login")
        self.ok("feat(ui)!: replace the sidebar", "feat/new-sidebar")
        self.ok("docs: add AGENTS.md", "docs/agents-md")
        self.ok("i18n(manage): translate the login dialog", "i18n/login-dialog")
        self.ok("build: update wry to 0.54", "build/wry-0.54")
        self.ok("fix(store-epic): keep the session", "fix/epic-session")
        self.ok("fix: drop GOG duplicates", "fix/gog-duplicates")

    def test_branch_needs_a_type_and_kebab_name(self):
        self.broken("fix: x", "claude/translucency")
        self.broken("fix: x", "fix-login")
        self.broken("fix: x", "fix/Login_Page")
        self.broken("fix: x", "feature/login", count=1)

    def test_title_needs_a_conventional_header(self):
        self.broken("Fix the login", "fix/login")
        self.broken("fix:no space", "fix/login")
        self.broken("fix(Epic): keep it", "fix/login")
        self.broken("fixed: keep it", "fix/login")

    def test_summary_style(self):
        self.broken("fix: keep the session.", "fix/login")
        self.broken("fix: Keep the session", "fix/login")
        self.broken("fix: " + "x" * 80, "fix/login")

    def test_types_must_match(self):
        self.broken("feat: add login", "fix/login")

    def test_base_is_main_or_a_stacked_branch(self):
        self.ok("fix: keep it", "fix/login", base="feat/cli-login")
        self.broken("fix: keep it", "fix/login", base="codex/macos-windows-support")

    def test_bots_are_exempt_from_branch_body_and_diff_size_rules(self):
        self.ok("build(deps): bump serde from 1.0.1 to 1.0.2", "dependabot/cargo/serde-1.0.2")
        self.ok(
            "build(deps): bump serde from 1.0.1 to 1.0.2",
            "dependabot/cargo/serde-1.0.2",
            rs_added=RS_ADDED_MAX + 1000,
            body="Bumps serde from 1.0.1 to 1.0.2.",
        )

    def test_rust_diff_size_limit(self):
        self.ok("feat: add feature", "feat/small-feature", rs_added=RS_ADDED_MAX)
        self.broken("feat: add feature", "feat/large-feature", rs_added=RS_ADDED_MAX + 1)
        self.ok(
            "feat: add feature",
            "feat/large-feature",
            rs_added=RS_ADDED_MAX + 1,
            labels=[LARGE_PR_LABEL],
        )

    def test_rs_added_from_numstat_ignores_binary_entries(self):
        sample = "120\t5\tcrates/core/src/lib.rs\n-\t-\tassets/icon.bin\n30\t10\tcrates/ui/src/lib.rs\n"
        self.assertEqual(rs_added_from_numstat(sample), 150)

    def test_body_valid_plain_language_then_technical(self):
        self.ok("fix(library): close filters on scroll", "fix/filters-scroll", body=VALID_BODY)

    def test_body_allows_optional_leading_draft_blockquote(self):
        draft_body = "> **Draft PR**: waiting on hardware verification.\n\n" + VALID_BODY
        self.assertEqual(check_body(draft_body), [])

    def test_body_allows_code_fences_in_technical_details_including_hashes(self):
        body = (
            "## Plain-language summary\n\n"
            "Makes store refresh faster and shows an explicit offline banner when disconnected.\n\n"
            "## Technical details\n\n"
            "```markdown\n"
            "## Fake heading inside code block\n"
            "```\n\n"
            "Updated `crates/services/src/games.rs`.\n"
        )
        self.assertEqual(check_body(body), [])

    def test_body_rejects_empty_or_template_comments_only(self):
        template_only = (
            "## Plain-language summary\n\n"
            "<!-- Start with 2 to 4 plain sentences. -->\n\n"
            "## Technical details\n\n"
            "<!-- Why this approach. -->\n"
        )
        problems = check_body(template_only)
        self.assertEqual(len(problems), 2, problems)
        self.assertTrue(any("Plain-language summary" in p for p in problems))
        self.assertTrue(any("Technical details" in p for p in problems))

    def test_body_rejects_technical_details_before_plain_language(self):
        swapped = (
            "## Technical details\n\n"
            "Refactored `crates/core/src/settings.rs` into submodules.\n\n"
            "## Plain-language summary\n\n"
            "Reorganizes internal settings code without changing how Irori looks or behaves.\n"
        )
        problems = check_body(swapped)
        self.assertTrue(any("must start with '## Plain-language summary'" in p for p in problems))
        self.assertTrue(any("must come after" in p for p in problems))

    def test_body_rejects_code_fences_inside_plain_language_summary(self):
        with_code = (
            "## Plain-language summary\n\n"
            "Updates the game launch pipeline for Linux users:\n\n"
            "```rust\n"
            "let x = 1;\n"
            "```\n\n"
            "## Technical details\n\n"
            "Updated `crates/runtime/src/lib.rs` to handle missing paths.\n"
        )
        problems = check_body(with_code)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("fenced code blocks", problems[0])

    def test_body_rejects_non_blockquote_text_before_plain_language_summary(self):
        body = "Random intro paragraph.\n\n" + VALID_BODY
        problems = check_body(body)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("before any other text", problems[0])

    def test_body_rejects_missing_technical_details(self):
        body = (
            "## Plain-language summary\n\n"
            "Improves controller focus wrapping on the Big Picture home shelves.\n"
        )
        problems = check_body(body)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("Technical details", problems[0])

    def test_cli_body_file_validates_pr_body(self):
        with tempfile.TemporaryDirectory() as tmp:
            body_path = Path(tmp) / "body.md"
            body_path.write_text(VALID_BODY, encoding="utf-8")
            buf = io.StringIO()
            with redirect_stdout(buf):
                rc = main([
                    "--title", "fix(library): close filters on scroll",
                    "--head", "fix/filters-scroll",
                    "--body-file", str(body_path),
                ])
            self.assertEqual(rc, 0)
            self.assertIn("OK:", buf.getvalue())

            body_path.write_text("## Technical details\n\nOnly technical text here.\n", encoding="utf-8")
            buf_bad = io.StringIO()
            with redirect_stdout(buf_bad):
                rc_bad = main([
                    "--title", "fix(library): close filters on scroll",
                    "--head", "fix/filters-scroll",
                    "--body-file", str(body_path),
                ])
            self.assertEqual(rc_bad, 1)
            self.assertIn("Plain-language summary", buf_bad.getvalue())

    def test_large_rs_file_advisory_warnings(self):
        warnings = check_large_rs_files({
            "crates/core/src/small.rs": LARGE_RS_WARN_LINES,
            "crates/core/src/big.rs": LARGE_RS_WARN_LINES + 1,
        })
        self.assertEqual(len(warnings), 1, warnings)
        self.assertIn("crates/core/src/big.rs", warnings[0])
        self.assertIn("irori-submodule-refactor", warnings[0])


if __name__ == "__main__":
    unittest.main()
