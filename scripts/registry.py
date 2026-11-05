#!/usr/bin/env python3
"""Stage built plugins and generate the registry index.json (kind "plugin").

    registry.py stage [--out dist-new] [names...]   check and copy built modules
    registry.py generate --base-url URL --out index.json DIR...   build the index from staged dirs
    registry.py verify index.json                    re-check an index file

`stage` needs `cargo build --release --target wasm32-unknown-unknown --workspace` first.
A staged dir is `<name>/<name>.wasm` plus `<name>.wasm.minisig` when signed. `generate` also
reads the plugin's Cargo.toml and manifest.json from the repo, and refuses to change a
version that the existing --out file already lists (published versions are immutable).
Stdlib only, so the release job needs nothing built.
"""
import argparse
import hashlib
import json
import pathlib
import re
import sys
import tomllib

UNPUBLISHED = {"selftest"}  # test fixtures, never published
MAX_MODULE = 8 * 1024 * 1024
MAX_SCHEMA = 64 * 1024
NAME = re.compile(r"^[a-z0-9][a-z0-9_-]*$")


def leb(b, i):
    n = s = 0
    while True:
        c = b[i]
        i += 1
        n |= (c & 0x7F) << s
        s += 7
        if not c & 0x80:
            return n, i


def wasm_abi(b):
    """`major.minor` from the wayhouse.plugin-abi custom section, else raise ValueError."""
    if b[:8] != b"\0asm\x01\0\0\0":
        raise ValueError("not a core WebAssembly module")
    i, found = 8, []
    while i < len(b):
        sid = b[i]
        size, i = leb(b, i + 1)
        end = i + size
        if sid == 0:
            nlen, j = leb(b, i)
            if b[j : j + nlen] == b"wayhouse.plugin-abi":
                found.append(b[j + nlen : end])
        i = end
    if len(found) != 1 or len(found[0]) != 4:
        raise ValueError("need exactly one 4-byte wayhouse.plugin-abi section")
    d = found[0]
    return f"{int.from_bytes(d[:2], 'little')}.{int.from_bytes(d[2:], 'little')}"


def crate(root, name):
    c = tomllib.loads((root / "plugins" / name / "Cargo.toml").read_text())
    ws = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]
    p = c["package"]
    lic = ws["license"] if p.get("license", {}) == {"workspace": True} else p["license"]
    return p["name"], p["version"], p.get("description", ""), lic


def available(root):
    return sorted(
        p.name for p in (root / "plugins").iterdir() if p.is_dir() and p.name not in UNPUBLISHED
    )


def stage(args):
    root = args.root.resolve()
    names = args.names or available(root)
    errors = []
    for n in names:
        if n in UNPUBLISHED or not (root / "plugins" / n).is_dir():
            errors.append(f"unknown or unpublished plugin: {n}")
            continue
        try:
            cname, _, _, _ = crate(root, n)
            if cname != n:
                errors.append(f"{n}: crate name {cname!r} must equal the directory name")
            wasm = root / "target/wasm32-unknown-unknown/release" / (n.replace("-", "_") + ".wasm")
            data = wasm.read_bytes()
            if not 0 < len(data) <= MAX_MODULE:
                errors.append(f"{n}: module is {len(data)} bytes, limit {MAX_MODULE}")
            wasm_abi(data)
        except (OSError, ValueError, KeyError, tomllib.TOMLDecodeError) as e:
            errors.append(f"{n}: {e}")
            continue
        if not errors:
            d = args.out / n
            d.mkdir(parents=True, exist_ok=True)
            (d / f"{n}.wasm").write_bytes(data)
            print(f"staged {n} ({len(data)} bytes)")
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    return 0


def semver_key(v):
    core, _, pre = v.partition("-")
    nums = tuple(int(x) for x in core.split("."))
    # a release sorts above its prereleases
    return nums, (1, ()) if not pre else (0, tuple(pre.split(".")))


def generate(args):
    root = args.root.resolve()
    old = {}
    if args.out.is_file():
        for p in json.loads(args.out.read_text()).get("plugins", []):
            for v in p["versions"]:
                old[(p["name"], v["version"])] = v
    plugins = {}
    for d in args.dirs:
        d = pathlib.Path(d)
        name = d.name
        cname, version, desc, lic = crate(root, name)
        data = (d / f"{name}.wasm").read_bytes()
        sig = d / f"{name}.wasm.minisig"
        url = f"{args.base_url.rstrip('/')}/{name}-v{version}/{name}.wasm"
        v = {
            "version": version,
            "abi": wasm_abi(data),
            "url": url,
            "sha256": hashlib.sha256(data).hexdigest(),
            "size": len(data),
        }
        if sig.is_file():
            v["signature_url"] = url + ".minisig"
        mf = root / "plugins" / name / "manifest.json"
        if mf.is_file():
            schema = json.loads(mf.read_text()).get("options")
            if schema is not None:
                if len(json.dumps(schema)) > MAX_SCHEMA:
                    raise SystemExit(f"{name}: options schema over {MAX_SCHEMA} bytes")
                v["options_schema"] = schema
        prev = old.get((name, version))
        if prev and (prev["sha256"] != v["sha256"] or prev["size"] != v["size"]):
            raise SystemExit(f"{name} {version} is published with a different hash; bump the version")
        entry = plugins.setdefault(
            name,
            {
                "name": name,
                "description": desc,
                "license": lic,
                "homepage": f"https://github.com/wayhouse-proxy/plugins/tree/main/plugins/{name}",
                "versions": {},
            },
        )
        entry["versions"][version] = v
    # Keep versions of plugins/versions not rebuilt this run.
    for p in (json.loads(args.out.read_text()).get("plugins", []) if args.out.is_file() else []):
        e = plugins.setdefault(p["name"], {**p, "versions": {}})
        for v in p["versions"]:
            e["versions"].setdefault(v["version"], v)
    out = []
    for n in sorted(plugins):
        e = plugins[n]
        e["versions"] = sorted(e["versions"].values(), key=lambda v: semver_key(v["version"]), reverse=True)
        out.append(e)
    doc = {"schema": 1, "kind": "plugin", "name": args.name, "plugins": out}
    args.out.write_text(json.dumps(doc, indent=2) + "\n")
    return verify_doc(doc)


def verify_doc(doc):
    errs = []
    if doc.get("schema") != 1 or doc.get("kind") != "plugin":
        errs.append("schema must be 1 and kind plugin")
    seen = set()
    for p in doc.get("plugins", []):
        if not NAME.match(p["name"]) or p["name"] in seen:
            errs.append(f"bad or duplicate name {p['name']!r}")
        seen.add(p["name"])
        keys = [semver_key(v["version"]) for v in p["versions"]]
        if keys != sorted(keys, reverse=True) or len(set(keys)) != len(keys):
            errs.append(f"{p['name']}: versions must be unique and newest first")
        for v in p["versions"]:
            where = f"{p['name']} {v['version']}"
            if not v["url"].startswith("https://"):
                errs.append(f"{where}: url must be https")
            if not re.fullmatch(r"[0-9a-f]{64}", v["sha256"]):
                errs.append(f"{where}: bad sha256")
            if not 0 < v["size"] <= MAX_MODULE:
                errs.append(f"{where}: bad size")
            if not re.fullmatch(r"\d+\.\d+", v["abi"]):
                errs.append(f"{where}: bad abi")
    if errs:
        print("\n".join(errs), file=sys.stderr)
        return 1
    return 0


def verify(args):
    return verify_doc(json.loads(args.file.read_text()))


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--root", type=pathlib.Path, default=pathlib.Path("."))
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("stage")
    s.add_argument("--out", type=pathlib.Path, default=pathlib.Path("dist"))
    s.add_argument("names", nargs="*")
    s.set_defaults(fn=stage)
    g = sub.add_parser("generate")
    g.add_argument("--name", default="wayhouse official plugins")
    g.add_argument("--base-url", required=True)
    g.add_argument("--out", type=pathlib.Path, required=True)
    g.add_argument("dirs", nargs="+")
    g.set_defaults(fn=generate)
    v = sub.add_parser("verify")
    v.add_argument("file", type=pathlib.Path)
    v.set_defaults(fn=verify)
    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
