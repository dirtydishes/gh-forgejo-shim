from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from gh_forgejo_shim.external import find_program


class ProgramDiscoveryTests(unittest.TestCase):
    def test_finds_real_gh_in_fallback_dirs_when_path_is_minimal(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            minimal_path = Path(tmp) / "minimal" / "bin"
            minimal_path.mkdir(parents=True)
            fallback = Path(tmp) / "homebrew" / "bin"
            real_gh = self._fake_executable(fallback, "gh")

            found = find_program(
                "gh",
                env={"PATH": str(minimal_path), "HOME": tmp},
                fallback_dirs=(str(fallback),),
            )

        self.assertEqual(found, str(real_gh))

    def test_skips_managed_gh_shim_in_fallback_dirs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            shim_dir = root / "local" / "bin"
            real_dir = root / "homebrew" / "bin"
            shim = shim_dir / "gh"
            shim_dir.mkdir(parents=True)
            shim.write_text(
                "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
                encoding="utf-8",
            )
            shim.chmod(0o755)
            real_gh = self._fake_executable(real_dir, "gh")

            found = find_program(
                "gh",
                env={"PATH": "", "HOME": tmp},
                fallback_dirs=(str(shim_dir), str(real_dir)),
            )

        self.assertEqual(found, str(real_gh))

    def test_configured_path_wins_over_fallback_dirs(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            configured = self._fake_executable(Path(tmp) / "configured", "gh")
            fallback = self._fake_executable(Path(tmp) / "fallback", "gh")

            found = find_program(
                "gh",
                configured=str(configured),
                env={"PATH": ""},
                fallback_dirs=(str(fallback.parent),),
            )

        self.assertEqual(found, str(configured))

    def test_configured_managed_gh_shim_does_not_win(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            configured_dir = root / "configured"
            configured = configured_dir / "gh"
            configured_dir.mkdir(parents=True)
            configured.write_text(
                "#!/bin/sh\n# managed by gh-forgejo-shim\nexec python -m gh_forgejo_shim gh \"$@\"\n",
                encoding="utf-8",
            )
            configured.chmod(0o755)
            fallback = self._fake_executable(root / "fallback", "gh")

            found = find_program(
                "gh",
                configured=str(configured),
                env={"PATH": ""},
                fallback_dirs=(str(fallback.parent),),
            )

        self.assertEqual(found, str(fallback))

    def _fake_executable(self, root: Path, name: str) -> Path:
        root.mkdir(parents=True, exist_ok=True)
        path = root / name
        path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        path.chmod(0o755)
        return path


if __name__ == "__main__":
    unittest.main()
