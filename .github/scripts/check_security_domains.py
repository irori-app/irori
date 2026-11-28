#!/usr/bin/env python3
"""Verify that every external HTTP(S) domain referenced in first-party Rust
production code is documented in SECURITY.md.

Usage:
    python3 .github/scripts/check_security_domains.py
"""

from __future__ import annotations

import os
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent.parent
CRATES_DIR = ROOT / "crates"
SECURITY_MD = ROOT / "SECURITY.md"

# Vendored crates tracked with their own upstream notices.
VENDORED_PREFIXES = (
    "crates/gpui-pre-windows/",
    "crates/gilrs-core/",
    "crates/comet/",
)

# XML namespaces, plist DTDs, loopback pseudo-hosts, and UI placeholder URLs
# that appear in source code as literal identifiers rather than network targets.
NON_NETWORK_HOSTS = frozenset(
    {
        "docs.oasis-open.org",
        "example.com",
        "example.net",
        "example.org",
        "localhost",
        "schemas.microsoft.com",
        "schemas.xmlsoap.org",
        "specs.openid.net",
        "steamloopback.host",
        "www.apple.com",
        "www.w3.org",
    }
)

URL_HOST_RE = re.compile(r"https?://([a-zA-Z0-9.-]+\.[a-zA-Z]{2,})")
TEST_BLOCK_RE = re.compile(r"#\[cfg\((?:test|all\(test\b)")


def strip_test_tail(source: str) -> str:
    """Drop inline `#[cfg(test)]` / `#[cfg(all(test, ...))]` tails."""
    match = TEST_BLOCK_RE.search(source)
    return source[: match.start()] if match else source


def registrable_domain(host: str) -> str:
    """Reduce a hostname to its two-label domain (keeping common 2-label ccTLDs)."""
    host = host.lower().rstrip(".")
    parts = host.split(".")
    if len(parts) <= 2:
        return host
    return ".".join(parts[-2:])


def extract_prod_domains(root: Path = ROOT) -> dict[str, list[str]]:
    """Return a map of host -> sorted list of relative paths in prod Rust code."""
    found: dict[str, set[str]] = {}
    crates = root / "crates"
    for dirpath, _, filenames in os.walk(crates):
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            full = Path(dirpath) / fn
            rel = full.relative_to(root).as_posix()
            if any(rel.startswith(p) for p in VENDORED_PREFIXES):
                continue
            parts = rel.split("/")
            if (
                any(p in ("tests", "examples", "benches") for p in parts)
                or fn in ("tests.rs", "ui_tests.rs")
                or fn.endswith("_tests.rs")
                or fn.endswith("_test.rs")
            ):
                continue
            text = strip_test_tail(full.read_text(encoding="utf-8", errors="replace"))
            for line in text.splitlines():
                if line.lstrip().startswith("//"):
                    continue
                for host in URL_HOST_RE.findall(line):
                    host = host.lower()
                    if host in NON_NETWORK_HOSTS:
                        continue
                    found.setdefault(host, set()).add(rel)
    return {host: sorted(paths) for host, paths in sorted(found.items())}


def find_undocumented_domains(
    security_text: str, prod_domains: dict[str, list[str]]
) -> dict[str, list[str]]:
    """Return hosts whose full host and registrable domain are absent from SECURITY.md."""
    lower_doc = security_text.lower()
    missing: dict[str, list[str]] = {}
    for host, paths in prod_domains.items():
        base = registrable_domain(host)
        if host not in lower_doc and base not in lower_doc:
            missing[host] = paths
    return missing


def main() -> int:
    security_text = SECURITY_MD.read_text(encoding="utf-8")
    prod_domains = extract_prod_domains(ROOT)
    missing = find_undocumented_domains(security_text, prod_domains)
    if missing:
        print(
            "ERROR: external domains found in production Rust code are missing from SECURITY.md:",
            file=sys.stderr,
        )
        for host, paths in missing.items():
            sample = ", ".join(paths[:3])
            print(f"  - {host} ({sample})", file=sys.stderr)
        return 1
    print(
        f"OK: all {len(prod_domains)} production external hosts are covered by SECURITY.md"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
