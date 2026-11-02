"""Prove retry and deadline behavior without waiting for a real mergeability race."""
import contextlib
import io
import unittest
from unittest.mock import Mock, patch
import urllib.error

import merge_retry


class RetryTests(unittest.TestCase):
    def run_retry(self, operation, timeout=30):
        elapsed = 0
        waits = []
        def sleep(delay):
            nonlocal elapsed
            elapsed += delay
            waits.append(delay)
        with patch.object(merge_retry.time, 'monotonic', side_effect=lambda: elapsed), \
                patch.object(merge_retry.time, 'sleep', side_effect=sleep), \
                contextlib.redirect_stdout(io.StringIO()) as log:
            result = merge_retry.merge_retry(operation, 6, timeout=timeout)
        return result, waits, log.getvalue()

    def error(self, code):
        error = urllib.error.HTTPError('http://disposable.invalid/merge', code, 'test', {}, None)
        self.addCleanup(error.close)
        return error

    def test_retries_405_with_backoff_and_logs_every_attempt(self):
        operation = Mock(side_effect=[self.error(405), self.error(405), 'merged'])
        result, waits, log = self.run_retry(operation)
        self.assertEqual(result, 'merged')
        self.assertEqual(waits, [0.25, 0.5])
        self.assertEqual(operation.call_count, 3)
        self.assertIn('attempt 1 HTTP 405', log)
        self.assertIn('attempt 2 HTTP 405', log)
        self.assertIn('attempt 3 succeeded', log)

    def test_persistent_405_stops_at_deadline(self):
        operation = Mock(side_effect=self.error(405))
        elapsed = 0
        def sleep(delay):
            nonlocal elapsed
            elapsed += delay
        with patch.object(merge_retry.time, 'monotonic', side_effect=lambda: elapsed), \
                patch.object(merge_retry.time, 'sleep', side_effect=sleep), \
                contextlib.redirect_stdout(io.StringIO()), \
                self.assertRaises(urllib.error.HTTPError):
            merge_retry.merge_retry(operation, 6, timeout=30)
        self.assertEqual(elapsed, 30)
        self.assertLess(operation.call_count, 25)

    def test_other_http_errors_fail_without_retry(self):
        operation = Mock(side_effect=self.error(503))
        with self.assertRaises(urllib.error.HTTPError):
            self.run_retry(operation)
        self.assertEqual(operation.call_count, 1)


if __name__ == '__main__':
    unittest.main()
