"""Stdlib verification script for the coding benchmark fixture."""

import unittest

from buggy_data import moving_average, normalize_email, unique_by_id


class BenchmarkVerificationTests(unittest.TestCase):
    def test_moving_average_includes_last_window(self):
        values = [2, 4, 6, 8]
        self.assertEqual(moving_average(values, 2), [3.0, 5.0, 7.0])

    def test_normalize_email_trims_and_lowercases(self):
        self.assertEqual(
            normalize_email("  Alice.Example@Example.COM  "),
            "alice.example@example.com",
        )

    def test_unique_by_id_keeps_first_occurrence(self):
        records = [
            {"id": "a", "value": 10},
            {"id": "b", "value": 20},
            {"id": "a", "value": 99},
            {"id": "c", "value": 30},
        ]
        self.assertEqual(
            unique_by_id(records),
            [
                {"id": "a", "value": 10},
                {"id": "b", "value": 20},
                {"id": "c", "value": 30},
            ],
        )


if __name__ == "__main__":
    unittest.main()
