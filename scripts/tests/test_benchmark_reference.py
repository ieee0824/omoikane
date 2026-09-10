import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('benchmark_reference', Path(__file__).resolve().parents[1] / 'benchmark-reference.py')
REFERENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REFERENCE)


class ReferenceValidationTests(unittest.TestCase):
    def setUp(self):
        self.expected = {'array': [500000, 124999750000]}
        self.rows = ['interpreter|1|array|500000|5.0|10.0|124999750000',
                     'jit|1|array|500000|2.5|5.0|124999750000']

    def test_complete_results_are_retained(self):
        samples = REFERENCE.validate_samples('\n'.join(self.rows), 1, self.expected)
        self.assertEqual(len(samples), 2)
        self.assertTrue(all(s['result'] == 124999750000 for s in samples))

    def test_incorrect_finite_and_nonfinite_values_fail(self):
        for result in ('NaN', 'Infinity', '-Infinity', '0', '124999750001'):
            with self.subTest(result=result), self.assertRaises(ValueError):
                REFERENCE.validate_samples('\n'.join([self.rows[0], self.rows[1].rsplit('|', 1)[0] + '|' + result]), 1, self.expected)

    def test_missing_duplicate_and_legacy_rows_fail(self):
        for rows in (self.rows[:1], self.rows * 2, [r.rsplit('|', 1)[0] for r in self.rows]):
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                REFERENCE.validate_samples('\n'.join(rows), 1, self.expected)

    def test_invalid_identity_count_and_timing_fail(self):
        for row in ('other|1|array|500000|5|10|124999750000',
                    'jit|2|array|500000|5|10|124999750000',
                    'jit|1|unknown|500000|5|10|124999750000',
                    'jit|1|array|1024|5|10|124999750000',
                    'jit|1|array|500000|0|10|124999750000',
                    'jit|1|array|500000|5|NaN|124999750000'):
            with self.subTest(row=row), self.assertRaises(ValueError):
                REFERENCE.validate_samples('\n'.join([self.rows[0], row]), 1, self.expected)

    def test_contract_matches_versioned_fixture(self):
        source = (REFERENCE.ROOT / 'tests/js_benchmark/shapes.js').read_text()
        expected, passes, fixture = REFERENCE.fixture_contract(source)
        self.assertEqual(expected['array'], [500000, 124999750000])
        self.assertEqual((len(expected), passes, fixture['version']), (11, 4, 2))
        self.assertEqual(len(fixture['sha256']), 64)


if __name__ == '__main__':
    unittest.main()
