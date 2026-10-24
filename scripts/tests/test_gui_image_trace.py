"""Reject stale presentation, duplicate delivery, and partial image-test traces."""
import importlib.util
import json
from pathlib import Path
import unittest
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


spec = importlib.util.spec_from_file_location(
    'image_loading', Path(__file__).resolve().parents[1] / 'test-gui-image-loading.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def record(trace_kind, **row):
    return 'OMOIKANE_' + trace_kind + ' ' + json.dumps(row) + '\n'


def delivery(kind='native_scroll_fallback', ids=(7, 8)):
    return record('PAGE_INPUT', kind=kind, success=True, source_count=len(ids),
                  gui_sequences={'ids': list(ids), 'total_count': len(ids), 'truncated': False})


class ImageWheelTraceTests(unittest.TestCase):
    def test_fallback_requires_paint_after_delivery(self):
        paint = record('PAINT', sequence=4)
        self.assertEqual(module.wheel_progress(paint + delivery()),
                         {'sources': 2, 'presented': False})
        self.assertEqual(module.wheel_progress(delivery() + paint),
                         {'sources': 2, 'presented': True})

    def test_native_present_must_match_delivered_input(self):
        for identifier, expected in ((6, False), (7, True)):
            present = record('NATIVE_SCROLL_PROBE', event='present', success=True,
                             gui_sequence=identifier)
            result = module.wheel_progress(present + delivery('native_scroll_accepted'))
            self.assertEqual(result['presented'], expected)

    def test_duplicate_sources_are_rejected(self):
        with self.assertRaises(AssertionError):
            module.wheel_progress(delivery() + delivery())

    def test_truncated_trace_does_not_create_delivery(self):
        self.assertEqual(module.wheel_progress(delivery()[:-1])['sources'], 0)

    def test_failed_presentation_does_not_count(self):
        present = record('NATIVE_SCROLL_PROBE', event='present', success=False, gui_sequence=7)
        self.assertFalse(module.wheel_progress(delivery() + present)['presented'])


if __name__ == '__main__':
    unittest.main()
