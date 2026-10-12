"""Verify the shipped module archive layout without compiling native code."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent

class PackageTests(unittest.TestCase):
    def test_verifier_extracts_module_into_private_home(self):
        """Nested Docker verification must own the extracted module directory."""
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory); home = root / "home"; home.mkdir()
            archive = root / "module.tar.gz"
            with tarfile.open(archive, "w:gz") as package:
                for name, data in [("libtinyvoice_module.so", b"fixture"), ("modules.toml", b"fixture = true\n")]:
                    info = tarfile.TarInfo(name); info.size = len(data)
                    import io
                    package.addfile(info, io.BytesIO(data))
            bin_dir = root / "bin"; bin_dir.mkdir()
            cargo = bin_dir / "cargo"
            cargo.write_text(
                "#!/usr/bin/env python3\nimport os,sys\nfrom pathlib import Path\n"
                "library=Path(os.environ.get('TINYDOCS_TEST_MODULE') or sys.argv[sys.argv.index('--')+1]).resolve()\n"
                "assert library.is_file(), library\n"
                "assert library.is_relative_to(Path.home().resolve()), library\n"
            )
            cargo.chmod(0o755)
            env = dict(os.environ, HOME=str(home), PATH=str(bin_dir) + os.pathsep + os.environ["PATH"])
            env.pop("CARGO_TARGET_DIR", None)
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/verify-module.sh"), "--archive", str(archive)],
                cwd=ROOT, env=env, text=True, capture_output=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_arch_package_install_disables_only_landlock_filesystem_sandbox(self):
        """Pacman gets the filesystem-only sandbox exception in nested Arch builds."""
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            bash = shutil.which("bash")
            self.assertIsNotNone(bash)
            (bin_dir / "bash").symlink_to(bash)
            capture = root / "pacman-args"
            pacman = bin_dir / "pacman"
            pacman.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$PACMAN_ARGS\"\n")
            pacman.chmod(0o755)
            env = {"PATH": str(bin_dir), "PACMAN_ARGS": str(capture)}
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/install-distro-packages.sh")],
                cwd=ROOT,
                env=env,
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            args = capture.read_text().splitlines()
            self.assertIn("--disable-sandbox-filesystem", args)
            self.assertNotIn("--disable-sandbox", args)
            self.assertEqual(
                args,
                ["-Syu", "--disable-sandbox-filesystem", "--noconfirm", "base-devel",
                 "cmake", "curl", "git", "gzip", "jq", "openssl", "perl", "alsa-lib",
                 "libx11", "libxtst", "libxi"],
            )

    def test_archive_attests_host_library_bytes_from_a_symlinked_checkout(self):
        """Package the actual build script and verify the shipped bytes and attestation."""
        target = ROOT / "target"; target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory); (root/"scripts").mkdir(); (root/"bin").mkdir()
            shutil.copy2(ROOT/"scripts/build-module.sh", root/"scripts/build-module.sh")
            for file in ["LICENSE","README.md","MODULE.md"]: (root/file).write_text("fixture")
            cargo = root/"bin/cargo"
            cargo.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\n"
                "root=Path.cwd()\n"
                "if 'metadata' in sys.argv: print(json.dumps({'target_directory':str(root/'target')}))\n"
                "else:\n"
                " target=sys.argv[sys.argv.index('--target')+1]\n"
                " output=root/'target'/target/'release'; output.mkdir(parents=True,exist_ok=True)\n"
                " (output/'libtinyvoice_module.so').write_bytes(b'compiled-fixture')\n")
            cargo.chmod(0o755)
            env = dict(os.environ, PATH=str(root/"bin")+os.pathsep+os.environ["PATH"])
            env.pop("CARGO_TARGET_DIR", None)
            alias = root/"checkout-link"; alias.symlink_to(root, target_is_directory=True)
            result = subprocess.run(["bash",str(alias/"scripts/build-module.sh"),
                "--crate","tinyvoice-module","--version","1.2.3-beta.1",
                "--target","x86_64-unknown-linux-gnu","--host","ubuntu-24.04-x86_64"],
                cwd=root,env=env,text=True,capture_output=True)
            self.assertEqual(result.returncode,0,result.stderr)
            archive=root/"release-assets/tinyvoice-module-1.2.3-beta.1-ubuntu-24.04-x86_64.tar.gz"
            with tarfile.open(archive) as package:
                data=package.extractfile("./libtinyvoice_module.so").read()
                manifest=tomllib.loads(package.extractfile("./modules.toml").read().decode())
                self.assertEqual(data,b"compiled-fixture")
                self.assertEqual(manifest,{"libtinyvoice_module.so":hashlib.sha256(data).hexdigest()})
                for name in ["./LICENSE","./README.md","./MODULE.md"]:
                    self.assertIsNotNone(package.extractfile(name))

if __name__ == "__main__": unittest.main()
