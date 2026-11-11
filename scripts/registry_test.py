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
            d = t / "dist/demo"
            d.mkdir(parents=True)
            (d / "demo.wasm").write_bytes(WASM)
            out = t / "index.json"
            cmd = [sys.executable, str(HERE / "registry.py"), "--root", str(root), "generate",
                   "--base-url", "https://x.test/dl", "--out", str(out), str(d)]
            self.assertEqual(subprocess.run(cmd).returncode, 0)
            first = out.read_text()
            doc = json.loads(first)
            v = doc["plugins"][0]["versions"][0]
            self.assertEqual(doc["kind"], "plugin")
            self.assertEqual(v["sha256"], hashlib.sha256(WASM).hexdigest())
            self.assertEqual(v["options_schema"], {"type": "object"})
            self.assertEqual(v["url"], "https://x.test/dl/demo-v0.1.0/demo.wasm")
            self.assertNotIn("signature_url", v)
            self.assertEqual(subprocess.run(cmd).returncode, 0)
            self.assertEqual(out.read_text(), first)
            (d / "demo.wasm").write_bytes(WASM[:-2] + b"\3\0")
            r = subprocess.run(cmd, capture_output=True, text=True)
            self.assertNotEqual(r.returncode, 0)
            self.assertIn("different hash", r.stderr)
            self.assertEqual(out.read_text(), first)


if __name__ == "__main__":
    unittest.main()
