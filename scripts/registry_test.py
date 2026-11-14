import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

import registry

HERE = pathlib.Path(__file__).parent
WASM = b"\0asm\x01\0\0\0" + bytes([0, 24, 19]) + b"wayhouse.plugin-abi" + b"\0\0\2\0"


class T(unittest.TestCase):
    def test_abi_is_read_from_the_section(self):
        self.assertEqual(registry.wasm_abi(WASM), "0.2")

    def test_missing_or_bad_abi_is_refused(self):
        with self.assertRaises(ValueError):
            registry.wasm_abi(b"\0asm\x01\0\0\0")
        with self.assertRaises(ValueError):
            registry.wasm_abi(b"nope")

    def test_claim_names_the_signed_artifact(self):
        with tempfile.TemporaryDirectory() as t:
            wasm = pathlib.Path(t) / "demo.wasm"
            wasm.write_bytes(WASM)
            r = subprocess.run(
                [sys.executable, str(HERE / "registry.py"), "claim", "demo", "0.1.0", str(wasm)],
                capture_output=True, text=True, check=True,
            )
        want = f"wayhouse-v1 plugin demo 0.1.0 0.2 {hashlib.sha256(WASM).hexdigest()}"
        self.assertEqual(r.stdout.strip(), want)

    def test_prereleases_sort_below_the_release(self):
        self.assertLess(registry.semver_key("0.3.0-beta.1"), registry.semver_key("0.3.0"))
        self.assertLess(registry.semver_key("0.2.9"), registry.semver_key("0.3.0-beta.1"))

    def test_generate_is_idempotent_and_refuses_a_changed_published_version(self):
        with tempfile.TemporaryDirectory() as t:
            t = pathlib.Path(t)
            root = t / "repo"
            (root / "plugins/demo").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace.package]\nlicense = "MIT"\n')
            (root / "plugins/demo/Cargo.toml").write_text(
                '[package]\nname = "demo"\nversion = "0.1.0"\ndescription = "d"\nlicense.workspace = true\n'
            )
            (root / "plugins/demo/manifest.json").write_text('{"options":{"type":"object"}}')
            (root / "plugins/demo/catalog.toml").write_text('categories = ["other"]\ntags = [" Demo ", "demo", "x-y"]\n')
            (root / "categories.toml").write_text('categories = ["other", "routing"]\n')
            d = t / "dist/demo"
            d.mkdir(parents=True)
            (d / "demo.wasm").write_bytes(WASM)
            out = t / "index.json"
            cmd = [sys.executable, str(HERE / "registry.py"), "--root", str(root), "generate",
                   "--base-url", "https://x.test/dl", "--date", "2026-11-12", "--out", str(out), str(d)]
            self.assertEqual(subprocess.run(cmd).returncode, 0)
            first = out.read_text()
            doc = json.loads(first)
            v = doc["plugins"][0]["versions"][0]
            self.assertEqual(doc["kind"], "plugin")
            self.assertEqual(v["sha256"], hashlib.sha256(WASM).hexdigest())
            self.assertEqual(v["options_schema"], {"type": "object"})
            self.assertEqual(v["url"], "https://x.test/dl/demo-v0.1.0/demo.wasm")
            self.assertNotIn("signature_url", v)
            e = doc["plugins"][0]
            self.assertEqual(doc["schema"], 2)
            self.assertEqual((e["categories"], e["tags"]), (["other"], ["demo", "x-y"]))
            self.assertEqual((e["latest"], e["abi"], e["signed"], e["updated"]), ("0.1.0", "0.2", False, "2026-11-12"))
            later = cmd[:cmd.index("--date") + 1] + ["2026-12-01"] + cmd[cmd.index("--date") + 2:]
            self.assertEqual(subprocess.run(later).returncode, 0)
            self.assertEqual(out.read_text(), first)
            (d / "demo.wasm").write_bytes(WASM[:-2] + b"\3\0")
            r = subprocess.run(cmd, capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("different hash", r.stderr)
            self.assertEqual(out.read_text(), first)

    def test_a_category_off_the_list_and_a_bad_tag_are_refused(self):
        with tempfile.TemporaryDirectory() as t:
            root = pathlib.Path(t)
            (root / "plugins/p").mkdir(parents=True)
            (root / "plugins/p/catalog.toml").write_text('categories = ["nope"]\n')
            with self.assertRaises(SystemExit) as cm:
                registry.labels(root, "p", ["other"])
            self.assertIn("unknown category 'nope'", str(cm.exception))
            with self.assertRaises(SystemExit):
                registry.allowed_categories(root)
        self.assertEqual(registry.label_errors("p", ["a1"], ["ok", "Bad Tag"]), ["p: 'Bad Tag' is not 2 to 24 of a-z, 0-9, '-'"])
        self.assertTrue(registry.label_errors("p", ["aa", "bb", "cc", "dd"], []))
        self.assertTrue(registry.label_errors("p", [], [f"t{i}" for i in range(9)]))

    def test_verify_rejects_schema_1_and_a_catalog_that_disagrees_with_its_versions(self):
        v = {"version": "0.1.0", "abi": "0.2", "url": "https://x.test/a", "sha256": "a" * 64, "size": 1}
        e = {"name": "d", "categories": ["other"], "tags": [], "latest": "0.1.0", "abi": "0.2", "signed": False,
             "updated": "2026-11-12", "versions": [v]}
        doc = {"schema": 2, "kind": "plugin", "name": "n", "plugins": [e]}
        self.assertEqual(registry.verify_doc(doc), 0)
        self.assertEqual(registry.verify_doc({**doc, "schema": 1}), 1)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "signed": True}]}), 1)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "updated": "soon"}]}), 1)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "homepage": "http://x.test"}]}), 1)
        for bad in ("https://@x.test/a", "https:///a", "https://u:p@x.test/a", "https://"):
            self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "versions": [{**v, "url": bad}]}]}), 1, bad)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "versions": [{**v, "signature_url": "http://x.test/s"}]}]}), 1)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "latest": "1.0", "versions": [{**v, "version": "1.0"}]}]}), 1)
        self.assertEqual(registry.verify_doc({**doc, "plugins": [{**e, "versions": [{**v, "size": 0}]}]}), 1)

    def test_a_removed_plugin_stays_in_the_index_and_generate_still_works(self):
        with tempfile.TemporaryDirectory() as t:
            t = pathlib.Path(t)
            root = t / "repo"
            (root / "plugins/demo").mkdir(parents=True)
            (root / "Cargo.toml").write_text('[workspace.package]\nlicense = "MIT"\n')
            (root / "plugins/demo/Cargo.toml").write_text(
                '[package]\nname = "demo"\nversion = "0.1.0"\ndescription = "d"\nlicense.workspace = true\n'
            )
            (root / "plugins/demo/catalog.toml").write_text('categories = ["other"]\ntags = ["demo"]\n')
            (root / "categories.toml").write_text('categories = ["other"]\n')
            d = t / "dist/demo"
            d.mkdir(parents=True)
            (d / "demo.wasm").write_bytes(WASM)
            out = t / "index.json"
            cmd = [sys.executable, str(HERE / "registry.py"), "--root", str(root), "generate",
                   "--base-url", "https://x.test/dl", "--date", "2026-11-12", "--out", str(out), str(d)]
            self.assertEqual(subprocess.run(cmd).returncode, 0)
            # The plugin directory is removed and nothing is rebuilt this run.
            import shutil
            shutil.rmtree(root / "plugins/demo")
            other = t / "dist/other"
            other.mkdir()
            (root / "plugins/other").mkdir(parents=True)
            (root / "plugins/other/Cargo.toml").write_text(
                '[package]\nname = "other"\nversion = "0.1.0"\ndescription = "o"\nlicense.workspace = true\n'
            )
            (root / "plugins/other/catalog.toml").write_text('categories = ["other"]\n')
            (other / "other.wasm").write_bytes(WASM)
            cmd2 = cmd[:-1] + [str(other)]
            r = subprocess.run(cmd2, capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            names = {p["name"]: p for p in json.loads(out.read_text())["plugins"]}
            self.assertEqual(set(names), {"demo", "other"})
            self.assertEqual((names["demo"]["categories"], names["demo"]["tags"]), (["other"], ["demo"]))


if __name__ == "__main__":
    unittest.main()
