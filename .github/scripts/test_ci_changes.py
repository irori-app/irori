"""Tests for ci_changes.py: python3 -m unittest discover -s .github/scripts"""

import unittest

from ci_changes import is_docs_only_path, needs_code_ci


class CiChangesTests(unittest.TestCase):
    def test_docs_skills_templates_and_pr_scripts_skip_rust_ci(self):
        self.assertFalse(
            needs_code_ci([
                "AGENTS.md",
                "README.md",
                "CONTRIBUTING.md",
                "docs/GRAPHS.md",
                "docs/PROMPTING_PLAYBOOK.md",
                ".agents/skills/irori-gpui-ui/SKILL.md",
                ".github/copilot-instructions.md",
                ".github/pull_request_template.md",
                ".github/ISSUE_TEMPLATE/bug_report.yml",
                ".github/scripts/pr_rules.py",
                "website/src/data/site.ts",
            ])
        )

    def test_code_manifests_packaging_and_workflows_require_rust_ci(self):
        code_paths = [
            "crates/core/src/lib.rs",
            "crates/feature-library/locales/en.json",
            "Cargo.toml",
            "Cargo.lock",
            "deny.toml",
            "clippy.toml",
            "rust-toolchain.toml",
            "packaging/package.py",
            "assets/engines/systems.json",
            ".github/workflows/ci.yml",
            ".github/actions/windows-arm64-tools/action.yml",
            "flake.nix",
        ]
        for path in code_paths:
            with self.subTest(path=path):
                self.assertTrue(needs_code_ci(["AGENTS.md", path]))

    def test_empty_or_traversal_paths_fail_safe_to_code_ci(self):
        self.assertTrue(needs_code_ci([]))
        self.assertFalse(is_docs_only_path("docs/../crates/core/src/lib.rs"))
        self.assertFalse(is_docs_only_path("/etc/passwd"))


if __name__ == "__main__":
    unittest.main()
