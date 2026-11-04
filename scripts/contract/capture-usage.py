#!/usr/bin/env python3
"""Capture gh's local help for exact argument-error usage, without reading a login."""

import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

from capture import official_gh
from common import ROOT


def section(help_text, heading):
    match = re.search(r"^" + re.escape(heading) + r"\n((?:[ \t].*\n|\n)*)", help_text, re.M)
    return match[1].rstrip("\n") if match else ""


def main():
    names = re.findall(r'^\[command\."([^"]+)"\]', (ROOT / "contract/tiers.toml").read_text(), re.M)
    groups = sorted({name.split()[0] for name in names if " " in name})
    cache = ROOT / "target/contract"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="usage-", dir=cache) as temporary:
        binary = official_gh(Path(temporary))
        env = {"PATH": os.environ["PATH"], "HOME": temporary, "GH_CONFIG_DIR": temporary,
               "GH_NO_UPDATE_NOTIFIER": "1", "NO_COLOR": "1", "TERM": "dumb", "LC_ALL": "C"}
        usage = {}
        for name in names + groups:
            # `help` never executes the named command, including merge and edit.
            result = subprocess.run([str(binary), "help", *name.split()], env=env, cwd=temporary,
                                    capture_output=True, text=True, check=True, timeout=20)
            text = result.stdout
            synopsis = section(text, "USAGE").strip()
            flags = section(text, "FLAGS")
            commands = sorted(re.findall(r"^  ([a-z][a-z-]+):", text, re.M))
            suffix = f"\nUsage:  {synopsis}\n"
            if name in groups:
                suffix += "\nAvailable commands:\n" + "".join(f"  {cmd}\n" for cmd in commands)
            elif flags:
                suffix += "\nFlags:\n" + flags + "\n  \n"
            usage[name] = {"suffix": suffix, "commands": commands, "help": text}
        (ROOT / "contract/usage-errors.json").write_text(json.dumps(usage, indent=2) + "\n")
        print(f"captured help for {len(usage)} commands/groups with official gh 2.103.3")


if __name__ == "__main__":
    main()
