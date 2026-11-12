#!/usr/bin/env python3
"""Print one release's notes from docs/release-notes.md.

usage: notes.py NOTES_FILE TAG OWNER/REPO OUT_FILE

docs/release-notes.md keeps every release's notes, newest first, each under a
`# gh-forgejo-shim vX.Y.Z` heading. A GitHub release gets only its own section:
without the heading, which the release title repeats, and without the closing
"The vX.Y.Z notes follow." line. Relative links point at the docs as they were
at TAG, since GitHub would resolve them against the release page.
"""

import posixpath
import re
import sys

HEADING = "# gh-forgejo-shim "
FENCE = re.compile(r"^\s*(```|~~~)")
TRAILER = re.compile(r"^The v\S+ notes follow\.$")
LINK = re.compile(r"\]\(([^)\s]+)\)")


class NotesError(Exception):
    pass


def _relative(target):
    return not (target.startswith(("#", "/")) or re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I))


def _absolute_links(line, tag, repo):
    def fix(match):
        target = match.group(1)
        if not _relative(target):
            return match.group(0)
        path, sep, anchor = target.partition("#")
        path = posixpath.normpath(posixpath.join("docs", path))
        return f"](https://github.com/{repo}/blob/{tag}/{path}{sep}{anchor})"

    return LINK.sub(fix, line)


def section(text, tag, repo):
    lines = text.splitlines()
    starts = [i for i, line in enumerate(lines) if line == HEADING + tag]
    if not starts:
        raise NotesError(f"no '{HEADING}{tag}' heading")
    if len(starts) > 1:
        raise NotesError(f"'{HEADING}{tag}' appears {len(starts)} times")

    body, fenced = [], False
    for line in lines[starts[0] + 1 :]:
        if FENCE.match(line):
            fenced = not fenced
        elif not fenced and line.startswith("# "):
            break
        body.append(line if fenced else _absolute_links(line, tag, repo))

    while body and not body[-1].strip():
        body.pop()
    if body and TRAILER.match(body[-1]):
        body.pop()
    while body and not body[-1].strip():
        body.pop()
    while body and not body[0].strip():
        body.pop(0)
    if not body:
        raise NotesError(f"the {tag} section is empty")
    return "\n".join(body) + "\n"


def main(argv):
    if len(argv) != 5:
        sys.exit(__doc__.strip().splitlines()[2])
    _, notes_file, tag, repo, out_file = argv
    with open(notes_file, encoding="utf-8") as f:
        text = f.read()
    try:
        body = section(text, tag, repo)
    except NotesError as err:
        sys.exit(f"{notes_file}: {err}")
    with open(out_file, "w", encoding="utf-8") as f:
        f.write(body)


if __name__ == "__main__":
    main(sys.argv)
