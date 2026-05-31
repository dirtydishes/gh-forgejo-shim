from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.shim import install_shim, is_managed_shim, uninstall_shim


class ShimLifecycleTests(unittest.TestCase):
    def test_install_refuses_unrelated_file_without_force(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            target = bin_dir / "gh"
            target.write_text("#!/bin/sh\n", encoding="utf-8")
            with self.assertRaises(FileExistsError):
                install_shim(bin_dir=bin_dir)

    def test_install_force_overwrites_and_uninstall_removes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            target = bin_dir / "gh"
            target.write_text("unrelated", encoding="utf-8")
            install_shim(bin_dir=bin_dir, force=True)
            self.assertTrue(is_managed_shim(target))
            self.assertTrue(os.access(target, os.X_OK))

            uninstall_shim(bin_dir=bin_dir)
            self.assertFalse(target.exists())

    def test_uninstall_refuses_unrelated_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp)
            target = bin_dir / "gh"
            target.write_text("unrelated", encoding="utf-8")
            with self.assertRaises(FileExistsError):
                uninstall_shim(bin_dir=bin_dir)


if __name__ == "__main__":
    unittest.main()
