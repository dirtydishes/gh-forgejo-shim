from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.gui_path import (
    LABEL,
    default_real_gh_gui_path,
    default_gui_path,
    install_gui_path,
    launch_agent_plist,
    uninstall_gui_path,
)


class GuiPathTests(unittest.TestCase):
    def test_default_gui_path_prioritizes_user_local_and_homebrew(self) -> None:
        value = default_gui_path(home=Path("/Users/alice"), env={"HOME": "/Users/alice"})

        parts = value.split(":")
        self.assertEqual(parts[0], "/Users/alice/.local/bin")
        self.assertIn("/opt/homebrew/bin", parts)
        self.assertIn("/usr/bin", parts)

    def test_launch_agent_plist_sets_path_with_launchctl(self) -> None:
        plist = launch_agent_plist("/tmp/a&b:/usr/bin")

        self.assertIn(LABEL, plist)
        self.assertIn("<string>/bin/launchctl</string>", plist)
        self.assertIn("<string>setenv</string>", plist)
        self.assertIn("<string>PATH</string>", plist)
        self.assertIn("/tmp/a&amp;b:/usr/bin", plist)

    def test_install_and_uninstall_gui_path_plist(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "LaunchAgents" / "test.plist"
            result = install_gui_path(
                path_value="/tmp/bin:/usr/bin",
                plist_path=target,
                apply_now=False,
            )

            self.assertEqual(result.plist_path, target)
            self.assertFalse(result.applied)
            self.assertTrue(target.exists())
            self.assertIn("/tmp/bin:/usr/bin", target.read_text(encoding="utf-8"))

            removed = uninstall_gui_path(plist_path=target, apply_now=False)

            self.assertEqual(removed.plist_path, target)
            self.assertEqual(removed.path_value, default_real_gh_gui_path())
            self.assertFalse(target.exists())

    def test_uninstall_gui_path_applies_real_gh_fallback_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / "LaunchAgents" / "test.plist"
            target.parent.mkdir()
            target.write_text(launch_agent_plist("/tmp/shim-only"), encoding="utf-8")
            calls: list[str] = []

            def fake_apply(path_value: str) -> tuple[bool, str | None]:
                calls.append(path_value)
                return True, None

            import gh_forgejo_shim.gui_path as gui_path

            original = gui_path.apply_gui_path
            try:
                gui_path.apply_gui_path = fake_apply
                result = uninstall_gui_path(plist_path=target)
            finally:
                gui_path.apply_gui_path = original

        self.assertFalse(target.exists())
        self.assertTrue(result.applied)
        self.assertEqual(calls, [default_real_gh_gui_path()])
        self.assertNotIn(".local/bin", result.path_value)


if __name__ == "__main__":
    unittest.main()
