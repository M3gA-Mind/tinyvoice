"""Compile the native listener boundary without a supported platform backend."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class NativePortabilityTests(unittest.TestCase):
    def test_unsupported_platform_compiles_without_dead_code(self):
        """Production builds need no listener constructor on unsupported platforms."""
        source = (ROOT / "crates/tinyvoice-module/src/service/hotkey/native/mod.rs").read_text()
        # Exercise the real unsupported-platform code on any host without requiring
        # a cross toolchain or platform-specific audio dependencies.
        source = re.sub(r'target_os = "[^"]+"', 'target_os = "fixture-unsupported"', source)
        target = ROOT / "target"
        target.mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(dir=target) as directory:
            root = Path(directory)
            (root / "native.rs").write_text(source)
            (root / "main.rs").write_text('''
extern crate self as tinyvoice_bus;
#[derive(Debug)]
pub enum HotkeyError { Unsupported }
pub struct HotkeyRequest;
mod native;
fn main() {
    match native::backend().start(&HotkeyRequest) {
        Err(HotkeyError::Unsupported) => (),
        Ok(mut listener) => {
            let _ = listener.events.try_recv();
            let _ = listener.overflow.load(std::sync::atomic::Ordering::SeqCst);
            let _ = listener.stop();
            panic!("unsupported platform started a listener");
        }
    }
}
''')
            result = subprocess.run(
                ["rustc", "--edition=2024", "-Dwarnings", str(root / "main.rs"),
                 "-o", str(root / "native-portability")],
                text=True, capture_output=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            subprocess.run([str(root / "native-portability")], check=True)


if __name__ == "__main__":
    unittest.main()
