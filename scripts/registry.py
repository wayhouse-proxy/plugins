#!/usr/bin/env python3
"""Stage built plugins and generate the registry index.json (kind "plugin").

    registry.py stage [--out dist-new] [names...]   check and copy built modules
    registry.py generate --base-url URL --out index.json [--date YYYY-MM-DD] DIR...   build the index from staged dirs
    registry.py verify index.json                    re-check an index file

`stage` needs `cargo build --release --target wasm32-unknown-unknown --workspace` first.
A staged dir is `<name>/<name>.wasm` plus `<name>.wasm.minisig` when signed. `generate` also
reads the plugin's Cargo.toml and manifest.json from the repo, and refuses to change a
version that the existing --out file already lists (published versions are immutable).
Stdlib only, so the release job needs nothing built.
"""
import argparse
import datetime
import hashlib
import json
import pathlib
import re
import sys
import tomllib
import urllib.parse

UNPUBLISHED = {"selftest"}  # test fixtures, never published
MAX_MODULE = 8 * 1024 * 1024
MAX_SCHEMA = 64 * 1024
NAME = re.compile(r"^[a-z0-9][a-z0-9_-]*$")
LABEL = re.compile(r"^[a-z0-9-]{2,24}$")
DATE = re.compile(r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$")
SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")
MAX_CATEGORIES, MAX_TAGS = 3, 8


def https_url_errors(where, what, url):
    """The client refuses an index whose URL is not plain https with a host and no userinfo."""
    try:
        u = urllib.parse.urlsplit(url)
        ok = u.scheme == "https" and bool(u.hostname) and u.username is None and u.password is None and "@" not in u.netloc
    except ValueError:
        ok = False
    return [] if ok else [f"{where}: {what} must be https with a host and no credentials"]


def allowed_categories(root):
    """The fixed list in categories.toml; a missing file is an error (it must not fail open)."""
    f = root / "categories.toml"
    if not f.is_file():
        raise SystemExit(f"{f} not found: the allowed category list is required")
    cats = tomllib.loads(f.read_text())["categories"]
    bad = [c for c in cats if not LABEL.match(c)]
    if bad or len(set(cats)) != len(cats):
        raise SystemExit(f"categories.toml: invalid or repeated {bad or cats}")
    return cats


def labels(root, name, allowed):
    """(categories, tags) from the plugin's catalog.toml (not manifest.json: that one is embedded in the
    module, so editing it would change published bytes). Tags are trimmed, lowercased and de-duplicated."""
    cf = root / "plugins" / name / "catalog.toml"
    m = tomllib.loads(cf.read_text()) if cf.is_file() else {}
    cats = m.get("categories", [])
    tags = list(dict.fromkeys(str(x).strip().lower() for x in m.get("tags", [])))
    errs = label_errors(name, cats, tags)
    if allowed is not None:
        errs += [f"{name}: unknown category {c!r} (allowed: {', '.join(allowed)})" for c in cats if c not in allowed]
    if not cats:
        errs.append(f"{name}: catalog.toml needs `categories` (1 to {MAX_CATEGORIES})")
    if errs:
        raise SystemExit("\n".join(errs))
    return cats, tags


def label_errors(name, cats, tags):
    errs = []
    if len(cats) > MAX_CATEGORIES:
        errs.append(f"{name}: more than {MAX_CATEGORIES} categories")
    if len(tags) > MAX_TAGS:
        errs.append(f"{name}: more than {MAX_TAGS} tags")
    for lst in (cats, tags):
        errs += [f"{name}: {x!r} is not 2 to 24 of a-z, 0-9, '-'" for x in lst if not LABEL.match(x)]
        if len(set(lst)) != len(lst):
            errs.append(f"{name}: repeated label in {lst}")
    return errs


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
    allowed = allowed_categories(root)
    old = {}
    old_updated = {}
    if args.out.is_file():
        for p in json.loads(args.out.read_text()).get("plugins", []):
            old_updated[p["name"]] = p.get("updated")
            for v in p["versions"]:
                old[(p["name"], v["version"])] = v
    plugins = {}
    added = set()
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
        if version not in entry["versions"] and (name, version) not in old:
            added.add(name)
        entry["versions"][version] = v
    # Keep versions of plugins/versions not rebuilt this run.
    # A plugin whose directory is gone keeps its categories and tags from the previous index.
    kept = {}
    for p in (json.loads(args.out.read_text()).get("plugins", []) if args.out.is_file() else []):
        if p["name"] not in plugins:
            kept[p["name"]] = (p.get("categories", []), p.get("tags", []))
        e = plugins.setdefault(p["name"], {**p, "versions": {}})
        for v in p["versions"]:
            e["versions"].setdefault(v["version"], v)
    out = []
    for n in sorted(plugins):
        e = plugins[n]
        e["versions"] = sorted(e["versions"].values(), key=lambda v: semver_key(v["version"]), reverse=True)
        top = e["versions"][0]
        e["categories"], e["tags"] = kept[n] if n in kept else labels(root, n, allowed)
        e["latest"], e["abi"], e["signed"] = top["version"], top["abi"], "signature_url" in top
        # The date moves only when a version is added (a backfill of an old version is still an add).
        e["updated"] = args.date if n in added or not old_updated.get(n) else old_updated[n]
        out.append({k: e[k] for k in ("name", "description", "license", "homepage", "categories", "tags", "latest", "abi", "signed", "updated", "versions") if k in e})
    doc = {"schema": 2, "kind": "plugin", "name": args.name, "plugins": out}
    args.out.write_text(json.dumps(doc, indent=2) + "\n")
    return verify_doc(doc)


def verify_doc(doc):
    errs = []
    if doc.get("schema") != 2 or doc.get("kind") != "plugin":
        errs.append("schema must be 2 and kind plugin")
    seen = set()
    for p in doc.get("plugins", []):
        if not NAME.match(p["name"]) or p["name"] in seen:
            errs.append(f"bad or duplicate name {p['name']!r}")
        seen.add(p["name"])
        errs += label_errors(p["name"], p.get("categories", []), p.get("tags", []))
        if not DATE.match(p.get("updated", "")):
            errs.append(f"{p['name']}: updated must be YYYY-MM-DD")
        if "homepage" in p:
            errs += https_url_errors(p["name"], "homepage", p["homepage"])
        if p["versions"]:
            top = p["versions"][0]
            if (p.get("latest"), p.get("abi"), p.get("signed")) != (top["version"], top["abi"], "signature_url" in top):
                errs.append(f"{p['name']}: latest, abi and signed must describe the newest version")
        keys = [semver_key(v["version"]) for v in p["versions"]]
        if keys != sorted(keys, reverse=True) or len(set(keys)) != len(keys):
            errs.append(f"{p['name']}: versions must be unique and newest first")
        for v in p["versions"]:
            where = f"{p['name']} {v['version']}"
            if not SEMVER.match(v["version"]):
                errs.append(f"{where}: bad version")
            errs += https_url_errors(where, "url", v["url"])
            if "signature_url" in v:
                errs += https_url_errors(where, "signature_url", v["signature_url"])
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


def claim(args):
    """Print the minisign trusted comment wayhouse requires: it names the signed artifact."""
    data = args.wasm.read_bytes()
    print(f"wayhouse-v1 plugin {args.name} {args.version} {wasm_abi(data)} {hashlib.sha256(data).hexdigest()}")
    return 0


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
    g.add_argument("--date", default=datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d"),
                   help="updated date (YYYY-MM-DD) for plugins that gain a version; default today, UTC")
    g.add_argument("dirs", nargs="+")
    g.set_defaults(fn=generate)
    c = sub.add_parser("claim")
    c.add_argument("name")
    c.add_argument("version")
    c.add_argument("wasm", type=pathlib.Path)
    c.set_defaults(fn=claim)
    v = sub.add_parser("verify")
    v.add_argument("file", type=pathlib.Path)
    v.set_defaults(fn=verify)
    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
