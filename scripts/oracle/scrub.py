"""Remove email addresses before captured output reaches disk. Also scrubs old fixtures."""
from pathlib import Path
import re
import sys

EMAIL = re.compile(rb"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?")
PLACEHOLDER = b'email@example.invalid'


def emails(value):
    return EMAIL.sub(PLACEHOLDER, value)


if __name__ == '__main__':
    if sys.argv[1:]:
        for filename in sys.argv[1:]:
            path = Path(filename)
            path.write_bytes(emails(path.read_bytes()))
    else:
        sys.stdout.buffer.write(emails(sys.stdin.buffer.read()))
