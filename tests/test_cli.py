from __future__ import annotations

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from gh_forgejo_shim.cli import main
from gh_forgejo_shim.gui_path import default_real_gh_gui_path
from gh_forgejo_shim.shim import install_shim


class CliTests(unittest.TestCase):
    def test_uninstall_shim_rewrites_managed_gui_path_to_real_gh_path_on_macos(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bin_dir = root / "bin"
            plist = root / "LaunchAgents" / "com.gh-forgejo-shim.user-gui-path.plist"
            plist.parent.mkdir()
            plist.write_text("managed gui path\n", encoding="utf-8")
            install_shim(bin_dir=bin_dir)
            applied: list[str] = []

            def fake_apply(path_value: str) -> tuple[bool, str | None]:
                applied.append(path_value)
                return True, None

            out = io.StringIO()
            with (
                mock.patch("sys.platform", "darwin"),
                mock.patch("gh_forgejo_shim.cli.default_plist_path", return_value=plist),
                mock.patch("gh_forgejo_shim.gui_path.apply_gui_path", side_effect=fake_apply),
                contextlib.redirect_stdout(out),
            ):
                code = main(["uninstall-shim", "--bin-dir", str(bin_dir)])

        self.assertEqual(code, 0)
        self.assertFalse((bin_dir / "gh").exists())
        self.assertEqual(applied, [default_real_gh_gui_path()])
        self.assertIn("updated macOS GUI PATH LaunchAgent", out.getvalue())


if __name__ == "__main__":
    unittest.main()
