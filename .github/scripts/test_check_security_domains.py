"""Unit tests for .github/scripts/check_security_domains.py."""

from __future__ import annotations

from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_security_domains as csd


class CheckSecurityDomainsTests(unittest.TestCase):
    def test_repo_security_md_covers_all_prod_domains(self) -> None:
        security_text = csd.SECURITY_MD.read_text(encoding="utf-8")
        prod = csd.extract_prod_domains(csd.ROOT)
        missing = csd.find_undocumented_domains(security_text, prod)
        self.assertEqual(missing, {}, f"Undocumented production hosts: {missing}")

    def test_detects_undocumented_domain_and_ignores_tests_and_comments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            src = root / "crates" / "demo" / "src"
            src.mkdir(parents=True)
            (src / "lib.rs").write_text(
                "\n".join(
                    [
                        '// doc link https://ignored-comment.example.invalid/x',
                        'pub const API: &str = "https://api.documented.dev/v1";',
                        'pub const SECRET: &str = "https://stealth.tracker.invalid/ping";',
                        'pub const NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";',
                        "#[cfg(test)]",
                        "mod tests {",
                        '    const TEST_URL: &str = "https://only-in-tests.invalid/";',
                        "}",
                    ]
                ),
                encoding="utf-8",
            )
            prod = csd.extract_prod_domains(root)
            self.assertIn("api.documented.dev", prod)
            self.assertIn("stealth.tracker.invalid", prod)
            self.assertNotIn("ignored-comment.example.invalid", prod)
            self.assertNotIn("schemas.xmlsoap.org", prod)
            self.assertNotIn("only-in-tests.invalid", prod)

            missing = csd.find_undocumented_domains(
                "Allowed services: documented.dev", prod
            )
            self.assertEqual(list(missing.keys()), ["stealth.tracker.invalid"])


if __name__ == "__main__":
    unittest.main()
