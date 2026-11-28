"""Unit tests for .github/scripts/check_aria2_release.py."""

from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_aria2_release as mod


class CheckAria2ReleaseTests(unittest.TestCase):
    def test_parses_real_pinned_version(self) -> None:
        repo_root = Path(__file__).resolve().parents[2]
        source = (repo_root / mod.ARIA2_DOWNLOAD_RS).read_text(encoding="utf-8")
        version = mod.parse_pinned_version(source)
        self.assertGreaterEqual(mod.parse_version_tuple(version), (1, 37, 0))

    def test_version_comparison_handles_tags_and_lengths(self) -> None:
        self.assertFalse(mod.is_newer_version("1.37.0", "release-1.37.0"))
        self.assertFalse(mod.is_newer_version("1.37.0", "release-1.36.0"))
        self.assertTrue(mod.is_newer_version("1.37.0", "release-1.37.1"))
        self.assertTrue(mod.is_newer_version("1.37.0", "release-1.38.0"))
        self.assertTrue(mod.is_newer_version("1.37.0", "v2.0"))
        with self.assertRaises(ValueError):
            mod.normalize_version("nightly-2026")

    def test_pick_windows_asset_prefers_build1_and_blocks_foreign_hosts(self) -> None:
        assets = [
            {
                "name": "aria2-1.38.0-win-64bit-build1.zip",
                "browser_download_url": "https://evil.example/aria2-1.38.0-win-64bit-build1.zip",
            },
            {
                "name": "aria2-1.38.0-win-64bit-build2.zip",
                "browser_download_url": "https://github.com/aria2/aria2/releases/download/release-1.38.0/aria2-1.38.0-win-64bit-build2.zip",
            },
            {
                "name": "aria2-1.38.0-win-64bit-build1.zip",
                "browser_download_url": "https://github.com/aria2/aria2/releases/download/release-1.38.0/aria2-1.38.0-win-64bit-build1.zip",
            },
        ]
        picked = mod.pick_windows_asset(assets, "1.38.0")
        self.assertEqual(
            picked,
            (
                "aria2-1.38.0-win-64bit-build1.zip",
                "https://github.com/aria2/aria2/releases/download/release-1.38.0/aria2-1.38.0-win-64bit-build1.zip",
            ),
        )

        # Foreign-host asset alone must be rejected.
        self.assertIsNone(mod.pick_windows_asset([assets[0]], "1.38.0"))

    def test_issue_formatting_and_deduplication(self) -> None:
        title = mod.issue_title_for("1.38.0")
        self.assertEqual(title, "chore(deps): update bundled Windows aria2c to 1.38.0")
        self.assertTrue(mod.has_open_issue([{"title": title}], title))
        self.assertFalse(mod.has_open_issue([{"title": "other issue"}], title))

        body = mod.format_issue_body(
            current_version="1.37.0",
            latest_version="1.38.0",
            tag_name="release-1.38.0",
            release_url="https://github.com/aria2/aria2/releases/tag/release-1.38.0",
            asset_name="aria2-1.38.0-win-64bit-build1.zip",
            asset_sha256="ab" * 32,
        )
        self.assertIn("1.37.0", body)
        self.assertIn("1.38.0", body)
        self.assertIn("aria2-1.38.0-win-64bit-build1.zip", body)
        self.assertIn("ab" * 32, body)
        self.assertIn("crates/services/src/web_downloads/aria2_download.rs", body)


if __name__ == "__main__":
    unittest.main()
