#!/usr/bin/env python3
"""Write one release's notes from docs/release-notes.md to OUT_FILE.

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
FENCE = re.compile(r"^\s*(`{3,}|~{3,})(.*)$")
TRAILER = re.compile(r"^The v\S+ notes follow\.$")
# An inline link or image's target, `<...>` or bare, up to an optional title; and a reference
# definition's target, `[a]: target`.
INLINE = re.compile(r'(!\[[^\]]*)?(\]\(\s*)(<[^>]+>|[^)\s]+)')
REFERENCE = re.compile(r"^(\s{0,3}\[[^\]]+\]:\s*)(<[^>]+>|\S+)")
CODE_SPAN = re.compile(r"(`+).+?\1")


class NotesError(Exception):
    pass


def _relative(target):
    return not (target.startswith(("#", "/")) or re.match(r"^[a-z][a-z0-9+.-]*:", target, re.I))


def _absolute(target, tag, repo, image=False):
    bracketed = target.startswith("<")
    bare = target[1:-1] if bracketed else target
    if not _relative(bare):
        return target
    path, sep, anchor = bare.partition("#")
    path = posixpath.normpath(posixpath.join("docs", path))
    if path.startswith(".."):
        raise NotesError(f"link {bare!r} points outside the repository")
    # /blob/ is GitHub's page for a file; an image needs the file itself.
    url = f"https://github.com/{repo}/{'raw' if image else 'blob'}/{tag}/{path}{sep}{anchor}"
    return f"<{url}>" if bracketed else url


def _absolute_links(line, tag, repo):
    def inline(match):
        prefix, opening, target = match.groups()
        return f"{prefix or ''}{opening}{_absolute(target, tag, repo, image=bool(prefix))}"

    line = REFERENCE.sub(lambda m: m.group(1) + _absolute(m.group(2), tag, repo), line)
    # Code spans show their text as written, so links inside them stay as they are.
    out, pos = [], 0
    for span in CODE_SPAN.finditer(line):
        out.append(INLINE.sub(inline, line[pos : span.start()]))
        out.append(span.group(0))
        pos = span.end()
    out.append(INLINE.sub(inline, line[pos:]))
    return "".join(out)


def section(text, tag, repo):
    lines = text.splitlines()
    starts = [i for i, line in enumerate(lines) if line == HEADING + tag]
    if not starts:
        raise NotesError(f"no '{HEADING}{tag}' heading")
    if len(starts) > 1:
        raise NotesError(f"'{HEADING}{tag}' appears {len(starts)} times")

    # A fence closes only on its own character, at least as long and with nothing after it.
    body, fence = [], None
    for line in lines[starts[0] + 1 :]:
        match = FENCE.match(line)
        if fence:
            marker = match.group(1) if match and not match.group(2).strip() else ""
            if marker[:1] == fence[0] and len(marker) >= len(fence):
                fence = None
            elif line.startswith(HEADING):
                raise NotesError(f"an unclosed code block runs into '{line}'")
            body.append(line)
            continue
        # A backtick fence's info string can't hold a backtick: ```foo``` is inline code.
        if match and not (match.group(1)[0] == "`" and "`" in match.group(2)):
            fence = match.group(1)
        elif line.startswith("# "):
            break
        body.append(line if fence else _absolute_links(line, tag, repo))
    if fence:
        raise NotesError(f"the {tag} section ends inside a code block")

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
