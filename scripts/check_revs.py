#!/usr/bin/env python3
"""The wayhouse revision is pinned in three places; they must be the same commit."""
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parents[1]
found = {
    "Cargo.toml": re.findall(r'wayhouse-plugin-abi = \{[^}]*rev = "([0-9a-f]{40})"', (root / "Cargo.toml").read_text()),
    ".github/workflows/ci.yml": re.findall(r"^\s*WAYHOUSE_REV: ([0-9a-f]{40})\s*$", (root / ".github/workflows/ci.yml").read_text(), re.M),
    ".github/workflows/release.yml": re.findall(r"^\s*WAYHOUSE_REV: ([0-9a-f]{40})\s*$", (root / ".github/workflows/release.yml").read_text(), re.M),
}
bad = {k: v for k, v in found.items() if len(v) != 1}
if bad or len({v[0] for v in found.values()}) != 1:
    print(f"the wayhouse revisions disagree or are missing: {found}", file=sys.stderr)
    sys.exit(1)
print("wayhouse rev:", next(iter(found.values()))[0])
