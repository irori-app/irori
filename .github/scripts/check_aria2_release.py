#!/usr/bin/env python3
"""Check whether upstream aria2/aria2 has a newer release than Irori's pinned Windows binary.

Reads ``VERSION`` from ``crates/services/src/web_downloads/aria2_download.rs``,
queries the GitHub Releases API for ``aria2/aria2``, and when a newer stable
version exists:

- locates the official Windows 64-bit ``.zip`` release asset under
  ``https://github.com/aria2/aria2/releases/download/``;
- computes its SHA-256 digest in streaming chunks; and
- optionally opens one deduplicated tracking issue in the current repository
  when invoked with ``--open-issue`` (used by ``.github/workflows/check-aria2-release.yml``).
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Mapping
import hashlib
import json
import os
from pathlib import Path
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

ARIA2_DOWNLOAD_RS = Path("crates/services/src/web_downloads/aria2_download.rs")
UPSTREAM_LATEST_API = "https://api.github.com/repos/aria2/aria2/releases/latest"
ALLOWED_ASSET_PREFIX = "https://github.com/aria2/aria2/releases/download/"
USER_AGENT = "irori-aria2-release-check"
MAX_ARCHIVE_BYTES = 50 * 1024 * 1024

VERSION_CONST_RE = re.compile(r'^pub const VERSION:\s*&str\s*=\s*"([^"]+)";\s*$', re.MULTILINE)
VERSION_NUM_RE = re.compile(r"^(?:release-|v)?([0-9]+(?:\.[0-9]+)*)$", re.IGNORECASE)


def parse_pinned_version(source: str) -> str:
    """Extract the pinned ``VERSION`` constant from ``aria2_download.rs``."""
    match = VERSION_CONST_RE.search(source)
    if not match:
        raise ValueError("Could not locate `pub const VERSION: &str = \"...\";` in aria2_download.rs")
    return match.group(1).strip()


def normalize_version(raw: str) -> str:
    """Strip ``release-`` or ``v`` prefixes and validate a numeric dotted version."""
    match = VERSION_NUM_RE.match(raw.strip())
    if not match:
        raise ValueError(f"Unsupported aria2 version/tag format: {raw!r}")
    return match.group(1)


def parse_version_tuple(raw: str) -> tuple[int, ...]:
    """Parse a release tag or version string into comparable integer components."""
    normalized = normalize_version(raw)
    return tuple(int(part) for part in normalized.split("."))


def is_newer_version(current: str, candidate: str) -> bool:
    """Return True iff ``candidate`` is strictly newer than ``current``."""
    cur = parse_version_tuple(current)
    cand = parse_version_tuple(candidate)
    width = max(len(cur), len(cand))
    cur_padded = cur + (0,) * (width - len(cur))
    cand_padded = cand + (0,) * (width - len(cand))
    return cand_padded > cur_padded


def pick_windows_asset(
    assets: Iterable[Mapping[str, object]],
    version: str,
) -> tuple[str, str] | None:
    """Pick the official Windows 64-bit zip asset from GitHub release metadata.

    Only accepts download URLs strictly prefixed by ``ALLOWED_ASSET_PREFIX``.
    """
    preferred = f"aria2-{version}-win-64bit-build1.zip".lower()
    fallback: tuple[str, str] | None = None
    for asset in assets:
        name = str(asset.get("name") or "").strip()
        url = str(asset.get("browser_download_url") or "").strip()
        if not name or not url.startswith(ALLOWED_ASSET_PREFIX):
            continue
        lower = name.lower()
        if lower == preferred:
            return (name, url)
        if lower.endswith(".zip") and "win-64bit" in lower and fallback is None:
            fallback = (name, url)
    return fallback


def issue_title_for(version: str) -> str:
    return f"chore(deps): update bundled Windows aria2c to {version}"


def format_issue_body(
    *,
    current_version: str,
    latest_version: str,
    tag_name: str,
    release_url: str,
    asset_name: str | None,
    asset_sha256: str | None,
) -> str:
    lines = [
        f"A new upstream **aria2** release (`{latest_version}`, tag `{tag_name}`) is available.",
        "",
        f"- **Current pinned version:** `{current_version}`",
        f"- **Latest upstream release:** [{tag_name}]({release_url})",
        f"- **File to update:** `{ARIA2_DOWNLOAD_RS.as_posix()}`",
    ]
    if asset_name and asset_sha256:
        lines.extend(
            [
                "",
                "### Verified Windows 64-bit asset",
                f"- **Archive:** `{asset_name}`",
                f"- **SHA-256:** `{asset_sha256}`",
            ]
        )
    else:
        lines.extend(
            [
                "",
                "No official `win-64bit` `.zip` asset was found on the release yet; verify upstream binaries manually before bumping.",
            ]
        )
    lines.extend(
        [
            "",
            "Once updated and released, existing Windows users whose managed `aria2c.exe` version marker (`version.txt`) is older will be offered a one-click update inside `+ Add download`.",
        ]
    )
    return "\n".join(lines) + "\n"


def has_open_issue(existing_issues: Iterable[Mapping[str, object]], title: str) -> bool:
    return any(str(issue.get("title") or "").strip() == title for issue in existing_issues)


def github_headers(token: str | None) -> dict[str, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": USER_AGENT,
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    return headers


def fetch_json(url: str, token: str | None) -> object:
    req = urllib.request.Request(url, headers=github_headers(token))
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode("utf-8"))


def sha256_url(url: str) -> str:
    if not url.startswith(ALLOWED_ASSET_PREFIX):
        raise ValueError(f"Refusing to download asset outside {ALLOWED_ASSET_PREFIX}: {url}")
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    hasher = hashlib.sha256()
    total = 0
    with urllib.request.urlopen(req, timeout=120) as resp:
        while True:
            chunk = resp.read(65536)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_ARCHIVE_BYTES:
                raise ValueError(f"Upstream aria2 archive exceeded {MAX_ARCHIVE_BYTES} bytes")
            hasher.update(chunk)
    return hasher.hexdigest()


def ensure_tracking_issue(repo: str, token: str, title: str, body: str) -> str:
    encoded_repo = "/".join(urllib.parse.quote(part, safe="") for part in repo.split("/", 1))
    list_url = f"https://api.github.com/repos/{encoded_repo}/issues?state=open&per_page=100"
    issues = fetch_json(list_url, token)
    if isinstance(issues, list) and has_open_issue(issues, title):
        return f"Issue already open: {title}"

    create_url = f"https://api.github.com/repos/{encoded_repo}/issues"
    payload = json.dumps({"title": title, "body": body}).encode("utf-8")
    headers = github_headers(token)
    headers["Content-Type"] = "application/json"
    req = urllib.request.Request(create_url, data=payload, headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=30) as resp:
        created = json.loads(resp.read().decode("utf-8"))
    return f"Opened issue: {created.get('html_url', title)}"


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--source",
        type=Path,
        default=ARIA2_DOWNLOAD_RS,
        help="Path to aria2_download.rs containing `pub const VERSION`.",
    )
    parser.add_argument(
        "--open-issue",
        action="store_true",
        help="Open a deduplicated GitHub issue when a newer upstream release exists.",
    )
    args = parser.parse_args(argv)

    token = os.environ.get("GITHUB_TOKEN") or None
    current = parse_pinned_version(args.source.read_text(encoding="utf-8"))
    release = fetch_json(UPSTREAM_LATEST_API, token)
    if not isinstance(release, dict):
        raise RuntimeError("Unexpected GitHub API response for aria2/aria2 latest release")

    tag_name = str(release.get("tag_name") or "").strip()
    release_url = str(
        release.get("html_url")
        or f"https://github.com/aria2/aria2/releases/tag/{tag_name}"
    ).strip()
    latest = normalize_version(tag_name)

    if not is_newer_version(current, latest):
        print(f"Up to date: aria2 {current} (latest upstream: {latest})")
        return 0

    assets = release.get("assets") if isinstance(release.get("assets"), list) else []
    picked = pick_windows_asset(assets, latest)
    asset_name: str | None = None
    asset_sha256: str | None = None
    if picked is not None:
        asset_name, asset_url = picked
        asset_sha256 = sha256_url(asset_url)

    title = issue_title_for(latest)
    body = format_issue_body(
        current_version=current,
        latest_version=latest,
        tag_name=tag_name,
        release_url=release_url,
        asset_name=asset_name,
        asset_sha256=asset_sha256,
    )
    print(f"Update available: {current} -> {latest}")
    print(body)

    if args.open_issue:
        repo = os.environ.get("GITHUB_REPOSITORY", "").strip()
        if not repo or not token:
            raise RuntimeError("GITHUB_REPOSITORY and GITHUB_TOKEN are required with --open-issue")
        print(ensure_tracking_issue(repo, token, title, body))

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
