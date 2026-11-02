"""Retry Forgejo's temporary mergeability rejection, within one bounded window."""
import time
import urllib.error


def merge_retry(operation, number, *, timeout=30):
    deadline = time.monotonic() + timeout
    attempt = 1
    while True:
        try:
            result = operation()
            print(f'Merge PR {number}: attempt {attempt} succeeded', flush=True)
            return result
        except urllib.error.HTTPError as error:
            remaining = deadline - time.monotonic()
            if error.code != 405 or remaining <= 0:
                raise
            delay = min(0.25 * 2 ** min(attempt - 1, 4), 2, remaining)
            print(f'Merge PR {number}: attempt {attempt} HTTP 405; retry in {delay:.2f}s', flush=True)
            error.close()
            time.sleep(delay)
            attempt += 1
