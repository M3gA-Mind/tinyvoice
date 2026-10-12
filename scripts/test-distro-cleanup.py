"""Verify container exit cleanup restores the actual module build directory."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class DistroCleanupTests(unittest.TestCase):
    def test_failed_container_restores_module_target_ownership(self):
        """Cargo metadata selects the target even when it is outside root target/."""
        source = (ROOT / "scripts/test-distro.sh").read_text()
        payload = source.split('"$image" bash -lc \'', 1)[1]
        cleanup = payload.split("    scripts/install-distro-packages.sh", 1)[0]
        target = ROOT / "target"
        target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory)
            binary = root / "bin"
            binary.mkdir()
            module_target = root / "crates/tinyvoice-module/custom-target"
            cargo = binary / "cargo"
            cargo.write_text("#!/usr/bin/env python3\nimport json\nprint(" +
                             repr(json.dumps({"target_directory": str(module_target)})) + ")\n")
            cargo.chmod(0o755)
            chown = binary / "chown"
            calls = root / "calls.jsonl"
            chown.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n" +
                             "with Path(" + repr(str(calls)) + ").open('a') as output:\n" +
                             " output.write(json.dumps(sys.argv[1:])+'\\n')\n")
            chown.chmod(0o755)
            env = dict(os.environ, PATH=str(binary) + os.pathsep + os.environ["PATH"],
                       HOST_UID="1001", HOST_GID="1002", OUTPUT_DIR="release-assets")
            result = subprocess.run(["bash", "-c", cleanup + "\nexit 17\n"],
                                    cwd=root, env=env, text=True, capture_output=True)
            self.assertEqual(result.returncode, 17, result.stderr)
            arguments = [argument for line in calls.read_text().splitlines()
                         for argument in json.loads(line)]
            self.assertIn(str(module_target), arguments)
            self.assertIn("1001:1002", arguments)


if __name__ == "__main__":
    unittest.main()
