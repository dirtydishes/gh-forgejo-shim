from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.gui_path import (
    LABEL,
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

            removed = uninstall_gui_path(plist_path=target)

            self.assertEqual(removed, target)
            self.assertFalse(target.exists())


if __name__ == "__main__":
    unittest.main()
