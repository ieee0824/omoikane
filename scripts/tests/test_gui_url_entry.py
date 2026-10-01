"""Outcome classification and pixel sampling of the GUI URL-entry E2E."""

import importlib.util
from pathlib import Path
import sys
import unittest

from PIL import Image

SCRIPTS = Path(__file__).parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location('url_entry', SCRIPTS / 'test-gui-url-entry.py')
url_entry = importlib.util.module_from_spec(spec)
spec.loader.exec_module(url_entry)


class ClassifyTest(unittest.TestCase):
    def test_declaration_separates_missing_feature_from_regression(self):
        failure = {'stage': 'destination paint', 'error': 'x'}
        self.assertEqual(url_entry.classify('implemented', None), 'PASS')
        self.assertEqual(url_entry.classify('implemented', failure), 'REGRESSION')
        self.assertEqual(url_entry.classify('unimplemented', failure), 'NOT_IMPLEMENTED')
        self.assertEqual(url_entry.classify('unimplemented', None), 'UNKNOWN')

    def test_setup_failures_are_environment_errors_for_any_declaration(self):
        failure = {'stage': 'environment', 'error': 'Xvfb startup timeout'}
        for declared in ('implemented', 'unimplemented'):
            self.assertEqual(url_entry.classify(declared, failure), 'ENV_ERROR')

    def test_exit_codes_are_distinct_and_only_pass_is_zero(self):
        self.assertEqual(len(set(url_entry.EXIT.values())), len(url_entry.EXIT))
        self.assertEqual([k for k, v in url_entry.EXIT.items() if v == 0], ['PASS'])


class RegionRatioTest(unittest.TestCase):
    def test_samples_only_the_client_region_fraction(self):
        im = Image.new('RGB', (300, 200), (0, 0, 0))
        im.paste((217, 118, 30), (100 + 25, 50 + 25, 100 + 75, 50 + 75))
        bounds = {'X': 100, 'Y': 50, 'WIDTH': 100, 'HEIGHT': 100}
        fraction = [0.25, 0.25, 0.75, 0.75]
        self.assertEqual(url_entry.region_ratio(im, bounds, fraction, [217, 118, 30]), 1.0)
        self.assertEqual(url_entry.region_ratio(im, bounds, fraction, [30, 111, 217]), 0.0)


if __name__ == '__main__':
    unittest.main()
