# Phase 08 Manual Validation Notes

Phase 08 ports setup and diagnostic commands to Rust. Automated tests use temp
homes, temp bins, fake `gh`/`git`, and injectable launchctl seams; they do not
mutate the real user environment.

## macOS GUI PATH

Safe plist-only validation:

```bash
tmp_home="$(mktemp -d)"
HOME="$tmp_home" gfj install-gui-path --no-apply
plutil -lint "$tmp_home/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist"
grep -F "$tmp_home/.local/bin" "$tmp_home/Library/LaunchAgents/com.gh-forgejo-shim.user-gui-path.plist"
HOME="$tmp_home" gfj uninstall-gui-path
rm -rf "$tmp_home"
```

Real launchd validation is explicit opt-in because it changes the current user
launchd environment:

```bash
gfj install-gui-path
/bin/launchctl getenv PATH
gfj uninstall-gui-path
```

After `install-gui-path`, restart a GUI-launched app and verify that `gh` resolves
through the intended user-local shim path. After `uninstall-gui-path`, verify the
LaunchAgent plist is removed. A login-session restart may be required for GUI
apps to return to the default launchd PATH.

## Keychain-Adjacent Auth Checks

`doctor` and `bootstrap` only check whether a token is discoverable; they do not
write Keychain entries. On macOS, validate without storing secrets by using a
temp home and env token:

```bash
tmp_home="$(mktemp -d)"
HOME="$tmp_home" FJ_SHIM_HOSTS=git.example.com FJ_SHIM_TOKEN=redacted-token gfj doctor
rm -rf "$tmp_home"
```

For real Keychain storage, use the existing `gfj auth login HOST` flow only as an
explicit opt-in manual test, then remove it with `gfj auth logout HOST`.

## Git Recorder

Safe recorder validation with a fake real git:

```bash
tmp_root="$(mktemp -d)"
printf '#!/bin/sh\necho fake-git "$@"\n' > "$tmp_root/real-git"
chmod +x "$tmp_root/real-git"
gfj trace git-recorder create "$tmp_root/git-trace.jsonl" --real-git "$tmp_root/real-git"
# Export the printed PATH and save the printed recorder directory, then run:
git status --token should-be-redacted
gfj trace summarize "$tmp_root/git-trace.jsonl"
gfj trace git-recorder remove "$printed_recorder_directory"
rm -rf "$tmp_root"
```

The recorder trace should contain `kind=git`, raw probe argv with sensitive
values redacted, byte counts, and no unredacted token values.
