"""Filesystem regression tests; never write to the host's /etc.

Run with: python3 -m unittest discover -s scripts/tests -v
"""
import pathlib
import subprocess
import tempfile
import unittest


PROJECT = pathlib.Path(__file__).resolve().parents[2]
HELPER = PROJECT / "scripts/install-pen-quirk.sh"
QUIRK = (PROJECT / "packaging/libinput/99-uscreen-pen.quirks").read_bytes()
BEGIN = b"# BEGIN UScreen managed pen contact quirk\n"
END = b"# END UScreen managed pen contact quirk\n"


class PenQuirkInstallTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="uscreen-quirk-")
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.override = self.root / "etc/libinput/local-overrides.quirks"

    def run_helper(self, action="install", success=True):
        result = subprocess.run(
            ["bash", str(HELPER), action, "--root", str(self.root)],
            capture_output=True, text=True,
        )
        self.assertEqual(result.returncode == 0, success, result.stderr)
        return result

    def existing(self, content):
        self.override.parent.mkdir(parents=True)
        self.override.write_bytes(content)

    def test_fresh_install_and_removal(self):
        self.run_helper()
        self.assertEqual(self.override.read_bytes(), BEGIN + QUIRK + END)
        self.assertEqual(self.override.stat().st_mode & 0o777, 0o644)
        self.run_helper("remove")
        self.assertFalse(self.override.exists())

    def test_reinstall_is_idempotent(self):
        self.run_helper()
        original = self.override.read_bytes()
        self.run_helper()
        self.assertEqual(self.override.read_bytes(), original)

    def test_preserves_user_overrides_and_permissions(self):
        user = b"# My settings\n[Custom UScreen pressure]\nMatchName=UScreen Pen*\nAttrPressureRange=2:1\n"
        self.existing(user)
        self.override.chmod(0o640)
        self.run_helper()
        self.assertEqual(self.override.read_bytes(), BEGIN + QUIRK + END + user)
        self.assertEqual(self.override.stat().st_mode & 0o777, 0o640)
        self.run_helper()
        self.run_helper("remove")
        self.assertEqual(self.override.read_bytes(), user)

    def test_upgrade_replaces_managed_block_only(self):
        user = b"# Keep this\n"
        self.existing(user + BEGIN + b"# Old quirk\n" + END)
        self.run_helper()
        self.assertEqual(self.override.read_bytes(), BEGIN + QUIRK + END + user)

    def test_malformed_markers_leave_file_untouched(self):
        for content in (BEGIN + b"# unfinished\n", END, BEGIN + BEGIN + END):
            with self.subTest(content=content):
                self.override.parent.mkdir(parents=True, exist_ok=True)
                self.override.write_bytes(content)
                self.run_helper(success=False)
                self.assertEqual(self.override.read_bytes(), content)
                self.assertEqual(list(self.override.parent.glob(".uscreen-*")), [])

    def test_symlink_is_not_replaced(self):
        target = self.root / "custom.quirks"
        target.write_bytes(b"# Keep this\n")
        self.override.parent.mkdir(parents=True)
        self.override.symlink_to(target)
        self.run_helper(success=False)
        self.assertTrue(self.override.is_symlink())
        self.assertEqual(target.read_bytes(), b"# Keep this\n")

    def test_remove_absent_is_noop(self):
        self.run_helper("remove")
        self.assertFalse(self.override.parent.exists())


if __name__ == "__main__":
    unittest.main()
