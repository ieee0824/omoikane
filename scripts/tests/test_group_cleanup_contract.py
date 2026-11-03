"""Monitor cleanup failure and repeated-signal contracts."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

spec = importlib.util.spec_from_file_location('cleanup_space', Path(__file__).parents[1] / 'gate5-space.py')
space = importlib.util.module_from_spec(spec)
spec.loader.exec_module(space)

class CleanupContractTests(unittest.TestCase):
    def run_cleanup(self, cleanup, cancelled=False, exitcode=75, late_signal=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            child = Mock()
            child.poll.return_value = 0
            def command(*args):
                state, result = args[-2:]
                state['process'] = child
                if cancelled:
                    state['signal'] = signal.SIGTERM
                return exitcode
            replace = Path.replace
            signalled = False
            def replace_then_signal(path, target):
                nonlocal signalled
                value = replace(path, target)
                if late_signal and not signalled:
                    signalled = True
                    os.kill(os.getpid(), signal.SIGTERM)
                return value
            with patch.object(space, 'poll_command', side_effect=command), patch.object(space, 'stop_process_group', side_effect=cleanup), patch.object(Path, 'replace', replace_then_signal):
                try:
                    code = space.run_monitored(['fixture'], root, root, {}, 100, .01)
                    error = None
                except BaseException as caught:
                    code, error = None, caught
            return code, error, json.loads((root / 'capacity-result.json').read_text())

    def assert_cleanup_failure(self, error, cancelled=False):
        def fail(_):
            raise error
        code, caught, result = self.run_cleanup(fail, cancelled)
        self.assertIs(caught, error)
        self.assertIsNone(code)
        self.assertNotEqual(result['status'], 'passed')
        self.assertEqual(result['exit'], 143 if cancelled else 1)
        self.assertEqual(result['cleanup_error']['type'], type(error).__name__)
        self.assertNotIn('cleanup', result)

    def test_ps_failure_is_terminal_cleanup_error(self):
        self.assert_cleanup_failure(subprocess.CalledProcessError(1, ['ps']))

    def test_malformed_ps_is_terminal_cleanup_error(self):
        self.assert_cleanup_failure(RuntimeError('cannot parse process group observation'))

    def test_post_kill_survivor_is_terminal_cleanup_error(self):
        self.assert_cleanup_failure(RuntimeError('live processes remain after SIGKILL'))

    def test_cancel_exit_retains_cleanup_failure_and_raises(self):
        self.assert_cleanup_failure(RuntimeError('observation failed'), cancelled=True)

    def test_repeated_signals_during_cleanup_preserve_first_and_restore_handlers(self):
        before = {number: signal.getsignal(number) for number in (signal.SIGTERM, signal.SIGINT)}
        def clean(_):
            os.kill(os.getpid(), signal.SIGTERM)
            os.kill(os.getpid(), signal.SIGINT)
            os.kill(os.getpid(), signal.SIGTERM)
        code, caught, result = self.run_cleanup(clean)
        self.assertIsNone(caught)
        self.assertEqual(code, 143)
        self.assertEqual(result['signal'], signal.SIGTERM)
        self.assertEqual(result['status'], 'cancelled')
        self.assertEqual(result['cleanup'], 'verified_no_live_members')
        for number, handler in before.items():
            self.assertEqual(signal.getsignal(number), handler)

    def test_cancel_during_terminal_write_still_cleans_exited_group(self):
        cleaned = Mock()
        code, caught, result = self.run_cleanup(cleaned, exitcode=0, late_signal=True)
        self.assertIsNone(caught)
        self.assertEqual(code, 143)
        cleaned.assert_called_once()
        self.assertEqual(result['cleanup'], 'verified_no_live_members')

    def test_late_cancel_cleanup_failure_is_saved_and_raised(self):
        failure = RuntimeError('late cleanup observation failed')
        def fail(_):
            raise failure
        code, caught, result = self.run_cleanup(fail, exitcode=0, late_signal=True)
        self.assertIs(caught, failure)
        self.assertIsNone(code)
        self.assertEqual(result['exit'], 143)
        self.assertEqual(result['cleanup_error']['message'], str(failure))
        self.assertNotIn('cleanup', result)

    def test_normal_success_keeps_existing_descendant_policy(self):
        cleaned = Mock()
        code, caught, result = self.run_cleanup(cleaned, exitcode=0)
        self.assertEqual(code, 0)
        self.assertIsNone(caught)
        cleaned.assert_not_called()
        self.assertEqual(result['status'], 'passed')

    def test_pending_cancel_before_terminal_commit_is_cleaned(self):
        cleaned = Mock()
        original_pending = signal.sigpending
        injected = False
        before_mask = signal.pthread_sigmask(signal.SIG_BLOCK, set())
        def pending_then_check():
            nonlocal injected
            if not injected:
                injected = True
                os.kill(os.getpid(), signal.SIGTERM)
            return original_pending()
        with patch.object(space.signal, 'sigpending', side_effect=pending_then_check):
            code, caught, result = self.run_cleanup(cleaned, exitcode=0)
        self.assertIsNone(caught)
        self.assertEqual(code, 143)
        cleaned.assert_called_once()
        self.assertEqual(result['cleanup'], 'verified_no_live_members')
        self.assertEqual(signal.pthread_sigmask(signal.SIG_BLOCK, set()), before_mask)

    def test_signal_after_terminal_commit_uses_restored_caller_handler(self):
        cleaned = Mock()
        delivered = []
        original_handler = signal.signal(signal.SIGTERM, lambda number, _: delivered.append(number))
        before_mask = signal.pthread_sigmask(signal.SIG_BLOCK, set())
        commit = space.commit_terminal_signals
        def commit_then_signal(state, number):
            committed = commit(state, number)
            if committed:
                os.kill(os.getpid(), signal.SIGTERM)
                self.assertIn(signal.SIGTERM, signal.sigpending())
                self.assertEqual(delivered, [])
            return committed
        try:
            with patch.object(space, 'commit_terminal_signals', side_effect=commit_then_signal):
                code, caught, result = self.run_cleanup(cleaned, exitcode=0)
            self.assertIsNone(caught)
            self.assertEqual(code, 0)
            self.assertEqual(result['status'], 'passed')
            cleaned.assert_not_called()
            self.assertEqual(delivered, [signal.SIGTERM])
            self.assertEqual(signal.pthread_sigmask(signal.SIG_BLOCK, set()), before_mask)
        finally:
            signal.signal(signal.SIGTERM, original_handler)
