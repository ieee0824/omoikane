"""Do not confuse stale or partial traces with a busy page-owner turn."""
import importlib.util
import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
spec = importlib.util.spec_from_file_location(
    'page_worker', Path(__file__).resolve().parents[1] / 'test-gui-page-worker.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def marker(**extra):
    return 'OMOIKANE_ATTRIBUTE_SET ' + json.dumps({
        'name': 'style', 'declarations': {'changed_properties': ['--worker-busy-2']},
        **extra}) + '\n'


class RuntimeErrorScrollTraceTests(unittest.TestCase):
    def test_owner_completion_ends_the_busy_period(self):
        active = marker() + 'OMOIKANE_NATIVE_SCROLL_PROBE {"event":"present"}\n'
        self.assertTrue(module.runtime_error_state(active)['busy_in_progress'])
        ended = active + 'OMOIKANE_FRAME_EVENT_LOOP {"success":false}\n'
        state = module.runtime_error_state(ended)
        self.assertTrue(state['second_busy'])
        self.assertFalse(state['busy_in_progress'])

    def test_partial_and_unrelated_attributes_do_not_start_a_busy_period(self):
        for text in [
            marker().rstrip('\n'),
            marker(name='class'),
            marker(declarations=None),
            marker(declarations={'changed_properties': []},
                   cached_style_state={'old_property': '--worker-busy-2'}),
            'OMOIKANE_ATTRIBUTE_SET {broken --worker-busy-2}\n',
            'OMOIKANE_ATTRIBUTE_SET []\n',
        ]:
            with self.subTest(text=text):
                self.assertFalse(module.runtime_error_state(text)['second_busy'])

    def test_runtime_errors_require_a_worker_failure_record(self):
        text = ('console: page worker operation failed: RuntimeLimit\n'
                'page worker operation failed: other error\n'
                'page worker operation failed: RuntimeLimit: timeout\n')
        self.assertEqual(module.runtime_error_state(text)['errors'], 1)


if __name__ == '__main__':
    unittest.main()
