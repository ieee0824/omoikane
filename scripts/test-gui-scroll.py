#!/usr/bin/env python3
"""Measure real native wheel/render/presentation stages on an owned X11 desktop."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import statistics
import time

from PIL import ImageGrab
from gui_navigation_fixture import NavigationFixture
from gui_x11 import GuiSession, geometry, trace_records

ROOT = Path(__file__).resolve().parents[1]


def records(gui, kind):
    return trace_records(gui.out / 'browser.log', kind)


def status(gui, window):
    title = gui.command('xdotool', 'getwindowname', window)
    match = re.search(r'scroll:(\d+) wheel:(\d+) click:([^ ]+) events:(\d+)', title)
    return (int(match[1]), int(match[2]), match[3], int(match[4])) if match else None


def last_frame(gui):
    frames = records(gui, 'PAINT')
    return frames[-1] if frames else None


WHEEL_INPUT_PATHS = {
    'wheel': 'canonical',
    'native_scroll_accepted': 'accepted',
    'native_scroll_fallback': 'fallback',
}


def wheel_accounting(gui):
    """Join every nonzero GUI wheel receipt to its one JS delivery path."""
    source_rows = [row for row in records(gui, 'PAGE_INPUT')
                   if row.get('kind') in WHEEL_INPUT_PATHS]
    probes = records(gui, 'NATIVE_SCROLL_PROBE')
    receipts = [row for row in probes
                if row.get('event') == 'receipt' and row.get('nonzero')]
    decisions = [row for row in probes
                 if row.get('event') == 'decision' and row.get('nonzero')]
    native_deliveries = [row for row in probes
                         if row.get('event') == 'delivery'
                         and row.get('decision') in ('accepted', 'fallback')]
    presents = [row for row in probes
                if row.get('event') == 'present' and row.get('success')]

    source_pairs = []
    metadata_valid = True
    for row in source_rows:
        sequence = row.get('gui_sequences') or {}
        ids = sequence.get('ids', [])
        metadata_valid &= (not sequence.get('truncated', True)
                           and sequence.get('total_count') == row.get('source_count') == len(ids)
                           and bool(ids)
                           and sequence.get('first') == ids[0]
                           and sequence.get('last') == ids[-1])
        path = WHEEL_INPUT_PATHS[row['kind']]
        expected_outcome = (None if path == 'canonical' else path)
        outcome = row.get('native_scroll_outcome')
        metadata_valid &= (outcome == expected_outcome if path != 'fallback'
                           else isinstance(outcome, str) and outcome.startswith('fallback:'))
        if path != 'canonical':
            metadata_valid &= len(ids) == 1 and row.get('gui_sequence') == ids[0]
        source_pairs.extend((sequence_id, path) for sequence_id in ids)

    first_presents = {}
    for row in presents:
        first_presents.setdefault(row['gui_sequence'], row)
    accepted = {row['gui_sequence']: row for row in decisions
                if row.get('decision') == 'accepted'}
    presentation_valid = all(
        row['gui_sequence'] in accepted
        and row.get('input_sequence') == accepted[row['gui_sequence']].get('input_sequence')
        for row in presents)
    return {
        'source_rows': source_rows,
        'source_pairs': source_pairs,
        'receipt_ids': [row['gui_sequence'] for row in receipts],
        'receipt_deltas': [row.get('delta') for row in receipts],
        'decision_pairs': [(row['gui_sequence'], row['decision']) for row in decisions],
        'native_delivery_pairs': [(row['gui_sequence'], row['decision'])
                                  for row in native_deliveries],
        'first_presents': first_presents,
        'sources': len(source_pairs),
        'delivered': len(source_rows),
        'metadata_valid': metadata_valid,
        'presentation_valid': presentation_valid,
        'successful': (all(row.get('success') for row in source_rows)
                       and all(row.get('success') for row in native_deliveries)),
    }


def wheel_accounting_errors(accounting, expected_sources):
    receipt_ids = accounting['receipt_ids']
    source_pairs = accounting['source_pairs']
    decision_pairs = accounting['decision_pairs']
    native_decisions = [pair for pair in decision_pairs if pair[1] != 'canonical']
    accepted_ids = {sequence_id for sequence_id, path in decision_pairs
                    if path == 'accepted'}
    presented_ids = set(accounting['first_presents'])
    checks = {
        'source count': accounting['sources'] == expected_sources,
        'receipt count': len(receipt_ids) == expected_sources,
        'unique receipts': len(set(receipt_ids)) == expected_sources,
        'source metadata': accounting['metadata_valid'],
        'source deltas': all(delta == [0.0, 40.0]
                             for delta in accounting['receipt_deltas']),
        'successful deliveries': accounting['successful'],
        'source IDs': Counter(sequence_id for sequence_id, _ in source_pairs)
                      == Counter(receipt_ids),
        'source paths': Counter(source_pairs) == Counter(decision_pairs),
        'native deliveries': Counter(accounting['native_delivery_pairs'])
                             == Counter(native_decisions),
        'native presentations': (presented_ids <= accepted_ids
                                 and accounting['presentation_valid']),
    }
    return [label for label, passed in checks.items() if not passed]


def assert_complete_wheel_accounting(accounting, expected_sources):
    errors = wheel_accounting_errors(accounting, expected_sources)
    assert not errors, (errors, accounting)
    return accounting


def wheel_delivery_lengths(gui):
    return len(records(gui, 'WHEEL')), len(records(gui, 'NATIVE_SCROLL_DELIVERY'))


def wheel_deliveries_after(gui, before):
    canonical = [{'path': 'canonical', **row}
                 for row in records(gui, 'WHEEL')[before[0]:]]
    native = [{'path': row['kind'], **row}
              for row in records(gui, 'NATIVE_SCROLL_DELIVERY')[before[1]:]]
    return canonical + native


def medians(rows, keys):
    return {key: statistics.median(row[key] for row in rows) for key in keys} if rows else {}


def accounting_status(gui, expected_sources):
    accounting = wheel_accounting(gui)
    return {
        'sources': accounting['sources'],
        'delivered': accounting['delivered'],
        'receipts': len(accounting['receipt_ids']),
        'accepted': sum(path == 'accepted' for _, path in accounting['decision_pairs']),
        'presented': len(accounting['first_presents']),
        'errors': wheel_accounting_errors(accounting, expected_sources),
    }


def wait_for_accounting(gui, expected_sources):
    gui.wait_for('wheel trace accounted',
                 lambda: accounting_status(gui, expected_sources),
                 lambda value: not value['errors'])
    return assert_complete_wheel_accounting(wheel_accounting(gui), expected_sources)


def recorded_wheel_sample(gui, expected_sources, before_receipts, before_frame,
                          before_delivery, started):
    accounting = wait_for_accounting(gui, expected_sources)
    new_receipts = set(accounting['receipt_ids']) - before_receipts
    assert len(new_receipts) == 1, (new_receipts, accounting)
    gui_sequence = new_receipts.pop()
    path = dict(accounting['decision_pairs'])[gui_sequence]
    delivery = gui.wait_for(
        'wheel delivery traced',
        lambda: wheel_deliveries_after(gui, before_delivery),
        lambda rows: len(rows) == 1)[0]
    assert delivery['success'] and delivery['path'] == path, (delivery, path)

    if path != 'accepted':
        frame = gui.wait_for('canonical frame presented', lambda: last_frame(gui),
                             lambda row: row and row['sequence'] > before_frame)
        return {**frame, **delivery, 'gui_sequence': gui_sequence,
                'observed_ms': (time.monotonic() - started) * 1000}

    presentation = gui.wait_for(
        'native frame presented',
        lambda: wheel_accounting(gui)['first_presents'].get(gui_sequence),
        bool)
    observed_ms = (time.monotonic() - started) * 1000
    # Drain the owner's canonical frame without using its later timing as the
    # latency of the already-presented native frame.
    gui.wait_for('owner frame caught up', lambda: last_frame(gui),
                 lambda row: row and row['sequence'] > before_frame)
    receipt = next(row for row in records(gui, 'NATIVE_SCROLL_PROBE')
                   if row.get('event') == 'receipt'
                   and row.get('gui_sequence') == gui_sequence)
    return {**delivery, 'gui_sequence': gui_sequence,
            'visual_offset': presentation['visual_offset'],
            'receipt_to_present_ms': (presentation['mono_ns'] - receipt['mono_ns']) / 1e6,
            'observed_ms': observed_ms}


def burst_presentation_status(gui, before_frame, burst_ids):
    accounting = wheel_accounting(gui)
    accepted = {sequence_id for sequence_id, path in accounting['decision_pairs']
                if path == 'accepted' and sequence_id in burst_ids}
    first_presents = {sequence_id: row for sequence_id, row
                      in accounting['first_presents'].items()
                      if sequence_id in accepted}
    offsets = {tuple(row['visual_offset']) for row in first_presents.values()}
    paint_frames = len(records(gui, 'PAINT')) - before_frame
    return {
        'paint_frames': paint_frames,
        'native_presented_inputs': len(first_presents),
        'native_presented_offsets': len(offsets),
        'progress_frames': max(paint_frames, min(len(first_presents), len(offsets))),
    }


def verify(binary, artifacts, scale, iterations, baseline):
    environment = {'WINIT_X11_SCALE_FACTOR': str(scale), 'OMOIKANE_TRACE_PAINT': '1',
                   'OMOIKANE_TRACE_INPUT': '1'}
    with NavigationFixture(ROOT / 'tests/fixtures/gui-scroll') as fixture:
        with GuiSession(artifacts, environment) as gui:
            app = gui.launch(binary, [fixture.url('/long.html')])
            window = gui.window(app)
            gui.command('xdotool', 'windowactivate', '--sync', window)
            fixture.wait_for_request('/long.html')
            gui.command('xdotool', 'windowsize', window, str(640 * scale), str(440 * scale))
            gui.wait_for('initial page', lambda: status(gui, window), lambda s: s == (0, 0, 'none', 0))
            gui.wait_for('initial frame', lambda: last_frame(gui),
                         lambda frame: frame and frame['viewport'] == [640, 400])
            bounds = geometry(gui, window)
            gui.command('xdotool', 'mousemove', '--window', window, str(500 * scale), str(240 * scale))
            time.sleep(.3)
            samples = []
            for index in range(iterations + 2):
                before_frame = len(records(gui, 'PAINT'))
                before_delivery = wheel_delivery_lengths(gui)
                before_receipts = set(wheel_accounting(gui)['receipt_ids'])
                started = time.monotonic()
                # This Winit X11 backend translates both XTest wheel-button
                # edges. Alternate edges to measure one native wheel event.
                edge = 'mousedown' if index % 2 == 0 else 'mouseup'
                gui.command('xdotool', edge, '5')
                expected = (40 * (index + 1), index + 1)
                gui.wait_for('wheel and scroll delivered', lambda: status(gui, window),
                             lambda s: s and s[:2] == expected and s[3] == index + 1)
                if baseline:
                    frame = gui.wait_for('scrolled frame presented', lambda: last_frame(gui),
                                         lambda row: row and row['sequence'] > before_frame)
                    wheel = records(gui, 'WHEEL')[-1]
                    assert wheel['success']
                    sample = {**frame, **wheel, 'path': 'canonical',
                              'observed_ms': (time.monotonic() - started) * 1000}
                else:
                    sample = recorded_wheel_sample(
                        gui, index + 1, before_receipts, before_frame,
                        before_delivery, started)
                if index >= 2:
                    samples.append(sample)
            # Native events arrive faster than frames. Adjacent compatible
            # inputs may coalesce; retain every source delta and draw progress.
            singles = iterations + 2
            if singles % 2:
                before_balance_frame = len(records(gui, 'PAINT'))
                gui.command('xdotool', 'mouseup', '5')
                singles += 1
                gui.wait_for('balanced wheel and scroll delivered',
                             lambda: status(gui, window),
                             lambda s: s and s[:2] == (40 * singles, singles))
                if not baseline:
                    wait_for_accounting(gui, singles)
                gui.wait_for('balanced owner frame caught up', lambda: last_frame(gui),
                             lambda row: row and row['sequence'] > before_balance_frame)
            before_burst = len(records(gui, 'PAINT'))
            before_burst_receipts = set(wheel_accounting(gui)['receipt_ids'])
            # Keep input active across several frames even on CI's unoptimized
            # build, where a single software-painted frame can exceed 200ms.
            burst_clicks = 40
            gui.command('xdotool', 'click', '--repeat', str(burst_clicks), '--delay', '30', '5')
            expected_sources = singles + 2 * burst_clicks
            expected = (40 * expected_sources, expected_sources)
            def burst_status():
                page = status(gui, window)
                if baseline:
                    return {'page': page, 'sources': page[1] if page else 0,
                            'delivered': page[1] if page else 0}
                return {'page': page, **accounting_status(gui, expected_sources)}

            burst = gui.wait_for('continuous wheels consumed', burst_status,
                                 lambda value: value['page']
                                 and value['page'][0] == expected[0]
                                 and value['sources'] == expected[1]
                                 and value['page'][1] == value['delivered']
                                 and not value.get('errors'), seconds=60)
            if baseline:
                burst_ids = set()
            else:
                accounting = assert_complete_wheel_accounting(
                    wheel_accounting(gui), expected_sources)
                burst_ids = set(accounting['receipt_ids']) - before_burst_receipts
                assert len(burst_ids) == 2 * burst_clicks, burst_ids
            progress = gui.wait_for(
                'burst surfaces presented',
                lambda: burst_presentation_status(gui, before_burst, burst_ids),
                lambda value: (value['progress_frames'] > 0 if baseline
                               else value['progress_frames'] > 1))
            gui.command('xdotool', 'click', '1')
            expected_row = (expected[0] + 200) // 80
            gui.wait_for('post-scroll hit test', lambda: status(gui, window),
                         lambda s: s and s[2] == f'row-{expected_row}')
            image = ImageGrab.grab(xdisplay=gui.env['DISPLAY']).convert('RGB')
            color = image.getpixel((bounds['X'] + 500 * scale, bounds['Y'] + 240 * scale))
            expected_color = (192, 128, 64) if expected_row % 2 else (64, 192, 128)
            assert color == expected_color, (color, expected_color)
            gui.screenshot('scrolled')
            canonical_samples = [sample for sample in samples if sample['path'] != 'accepted']
            native_samples = [sample for sample in samples if sample['path'] == 'accepted']
            report = {
                'scale': scale, 'iterations': iterations, 'samples': samples,
                'page_bounds': [bounds['X'], bounds['Y'] + 40 * scale, 640 * scale, 400 * scale],
                'median_ms': medians(canonical_samples, (
                    'dispatch_ms', 'adjusted_layout_ms', 'paint_ms', 'frame_ms', 'present_ms')),
                'native_median_ms': medians(native_samples, (
                    'dispatch_ms', 'receipt_to_present_ms', 'observed_ms')),
                'continuous_status': burst, 'hit_row': expected_row,
                'burst_frames': progress['paint_frames'],
                'burst_native_presented_inputs': progress['native_presented_inputs'],
                'burst_native_presented_offsets': progress['native_presented_offsets'],
            }
            (gui.out / 'benchmark.json').write_text(json.dumps(report, indent=2) + '\n')
            print(json.dumps({'canonical': report['median_ms'],
                              'native': report['native_median_ms']}), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--artifacts', required=True)
    parser.add_argument('--scale', type=int, choices=(1, 2), default=1)
    parser.add_argument('--iterations', type=int, default=10)
    parser.add_argument('--baseline', action='store_true',
                        help='Record the old stalled burst without claiming the progress check passes')
    options = parser.parse_args()
    assert options.iterations > 0
    verify(options.binary, options.artifacts, options.scale, options.iterations, options.baseline)
