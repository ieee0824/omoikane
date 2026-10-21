"""Trace-accounting contracts for the real GUI scroll harness."""

import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location('test_gui_scroll', SCRIPTS / 'test-gui-scroll.py')
SCROLL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCROLL)


class FakeGui:
    def __init__(self, out):
        self.out = Path(out)


def page_input(kind, ids, outcome=None):
    return ('PAGE_INPUT', {
        'gui_sequence': ids[-1],
        'gui_sequences': {
            'first': ids[0], 'ids': ids, 'last': ids[-1],
            'total_count': len(ids), 'truncated': False,
        },
        'kind': kind,
        'native_scroll_outcome': outcome,
        'source_count': len(ids),
        'success': True,
    })


def probe(event, sequence, **fields):
    return ('NATIVE_SCROLL_PROBE', {
        'event': event, 'gui_sequence': sequence, **fields,
    })


class ScrollAccountingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.gui = FakeGui(self.temp.name)
        self.log = self.gui.out / 'browser.log'

    def write(self, rows):
        self.log.write_text(''.join(
            f'OMOIKANE_{kind} {json.dumps(row, separators=(",", ":"))}\n'
            for kind, row in rows))

    @staticmethod
    def mixed_rows():
        rows = [
            page_input('wheel', [10, 11]),
            page_input('native_scroll_accepted', [12], 'accepted'),
            page_input('native_scroll_fallback', [13], 'fallback:FallbackInFlight'),
        ]
        rows += [probe('receipt', sequence, nonzero=True, delta=[0.0, 40.0])
                 for sequence in range(10, 14)]
        rows += [probe('decision', 10, nonzero=True, decision='canonical'),
                 probe('decision', 11, nonzero=True, decision='canonical'),
                 probe('decision', 12, nonzero=True, decision='accepted',
                       input_sequence=1, visual_offset=[0.0, 80.0]),
                 probe('decision', 13, nonzero=True, decision='fallback')]
        rows += [probe('delivery', 12, decision='accepted', success=True),
                 probe('delivery', 13, decision='fallback', success=True)]
        rows += [probe('present', 12, input_sequence=1, success=True,
                       mono_ns=120, visual_offset=[0.0, 80.0]),
                 probe('present', 12, input_sequence=1, success=True,
                       mono_ns=130, visual_offset=[0.0, 80.0])]
        return rows

    def test_all_three_page_input_paths_account_for_each_source_once(self):
        self.write(self.mixed_rows())
        accounting = SCROLL.wheel_accounting(self.gui)
        SCROLL.assert_complete_wheel_accounting(accounting, 4)
        self.assertEqual(accounting['sources'], 4)
        self.assertEqual(accounting['delivered'], 3)
        self.assertEqual(accounting['source_pairs'], [
            (10, 'canonical'), (11, 'canonical'),
            (12, 'accepted'), (13, 'fallback'),
        ])
        self.assertEqual(set(accounting['first_presents']), {12})

    def test_missing_duplicate_and_truncated_sources_are_rejected(self):
        cases = {}
        cases['missing delivery'] = [row for row in self.mixed_rows()
                                     if not (row[0] == 'NATIVE_SCROLL_PROBE'
                                             and row[1].get('event') == 'delivery'
                                             and row[1]['gui_sequence'] == 13)]
        duplicate = self.mixed_rows()
        duplicate.insert(1, page_input('wheel', [10]))
        cases['duplicate source'] = duplicate
        truncated = self.mixed_rows()
        truncated[0][1]['gui_sequences']['truncated'] = True
        cases['truncated source list'] = truncated
        for label, rows in cases.items():
            with self.subTest(label=label):
                self.write(rows)
                self.assertTrue(SCROLL.wheel_accounting_errors(
                    SCROLL.wheel_accounting(self.gui), 4))

    def test_successful_present_must_belong_to_an_accepted_source(self):
        cases = {}
        wrong_source = self.mixed_rows()
        wrong_source.append(probe('present', 13, input_sequence=2, success=True,
                                  mono_ns=140, visual_offset=[0.0, 120.0]))
        cases['fallback source'] = wrong_source
        wrong_input = self.mixed_rows()
        wrong_input[-1][1]['input_sequence'] = 9
        cases['wrong input sequence'] = wrong_input
        for label, rows in cases.items():
            with self.subTest(label=label):
                self.write(rows)
                self.assertIn('native presentations', SCROLL.wheel_accounting_errors(
                    SCROLL.wheel_accounting(self.gui), 4))

    def test_rebased_present_may_use_a_new_visual_offset(self):
        rows = self.mixed_rows()
        for kind, row in rows:
            if kind == 'NATIVE_SCROLL_PROBE' and row.get('event') == 'present':
                row['visual_offset'] = [0.0, 96.0]
        self.write(rows)
        SCROLL.assert_complete_wheel_accounting(SCROLL.wheel_accounting(self.gui), 4)

    def test_repeated_present_does_not_inflate_continuous_progress(self):
        self.write(self.mixed_rows() + [('PAINT', {'sequence': 1})])
        progress = SCROLL.burst_presentation_status(self.gui, 0, {12})
        self.assertEqual(progress, {
            'paint_frames': 1,
            'native_presented_inputs': 1,
            'native_presented_offsets': 1,
            'progress_frames': 1,
        })

    def test_delivery_timing_uses_only_records_after_snapshot(self):
        rows = [('WHEEL', {'dispatch_ms': 4.0, 'success': True})]
        self.write(rows)
        before = SCROLL.wheel_delivery_lengths(self.gui)
        rows += [('NATIVE_SCROLL_DELIVERY', {
            'dispatch_ms': 2.0, 'kind': 'accepted', 'success': True,
        })]
        self.write(rows)
        self.assertEqual(SCROLL.wheel_deliveries_after(self.gui, before), [{
            'path': 'accepted', 'dispatch_ms': 2.0,
            'kind': 'accepted', 'success': True,
        }])


if __name__ == '__main__':
    unittest.main()
