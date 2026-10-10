"""Keep crash diagnosis separate from the authoritative failed Gate 5 suite."""
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import MagicMock, patch


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).parents[1] / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


diagnose = module("diagnose_gate5_wpt_tests", "diagnose-gate5-wpt.py")
capture = module("gate5_lldb_capture_tests", "gate5_lldb_capture.py")


class DiagnosticTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.suite, self.target, self.wpt = (self.root / name for name in ("suite", "target", "wpt"))
        for directory in (self.suite, self.target, self.wpt):
            directory.mkdir()
        self.binary = self.target / "wpt_smoke-0498357e008af293"
        self.binary.write_bytes(b"mock ELF bytes; this file is never executed")
        self.normal_environment = {
            "WPT_ROOT": str(self.wpt), "WPT_REQUIRED": "1",
            "WPT_MANIFEST": "tests/wpt/manifest.txt", "RUST_TEST_THREADS": "7", "CI": "1",
            "WPT_REPORT": str(self.suite / "wpt.json"), "WPT_JUNIT": str(self.suite / "wpt.xml"),
            "OMOIKANE_JIT_GATE_REPORT_DIR": str(self.suite),
            "OMOIKANE_WEB_API_REPORT": str(self.suite / "web-api.json")}
        self.save("result.json", {"status": "failed", "steps": [{"name": "full-suite", "exit": 101}]})
        self.save("suite-environment.json", self.normal_environment)
        self.save("wpt.json", {"summary": {"total": 279, "regression": 0}})
        self.save("acid3.json", {"direct": {"score": 100}})
        self.write_crash()

    def save(self, name, value):
        (self.suite / name).write_text(json.dumps(value))

    def write_crash(self, binary=None, argv="--include-ignored --nocapture"):
        (self.suite / "full-suite.log").write_text(
            "error: test failed, to rerun pass `--test wpt_smoke`\n"
            "Caused by:\n  process didn't exit successfully: `" + str(binary or self.binary)
            + " " + argv + "` (signal: 11, SIGSEGV: invalid memory reference)\n")

    def run_diagnostic(self, platform="darwin", replay=None):
        with patch.object(sys, "argv", ["diagnose", str(self.suite)]), \
             patch.object(sys, "platform", platform), \
             patch.dict(os.environ, {"CARGO_TARGET_DIR": str(self.target), "RUST_TEST_THREADS": "1"}), \
             patch.object(diagnose.subprocess, "check_output", return_value="mock identity\n"), \
             patch.object(diagnose.subprocess, "run", side_effect=replay) as launch:
            result = diagnose.main()
            return result, launch

    def test_replay_preserves_all_normal_bytes_and_uses_exact_binary_and_environment(self):
        before = {path.name: path.read_bytes() for path in self.suite.iterdir()}
        def replay(command, **kwargs):
            self.assertEqual(command[command.index("--file") + 1], str(self.binary))
            self.assertIn("script gate5_lldb_capture.capture(lldb.debugger)", command)
            environment = kwargs["env"]
            for name in ("WPT_ROOT", "WPT_REQUIRED", "WPT_MANIFEST", "RUST_TEST_THREADS", "CI"):
                self.assertEqual(environment[name], self.normal_environment[name])
            for name in ("WPT_REPORT", "WPT_JUNIT", "OMOIKANE_JIT_GATE_REPORT_DIR", "OMOIKANE_WEB_API_REPORT"):
                self.assertNotEqual(environment[name], self.normal_environment[name])
                self.assertTrue(Path(environment[name]).is_relative_to(self.suite / "crash-diagnostic"))
            return types.SimpleNamespace(returncode=0)
        result, launch = self.run_diagnostic(replay=replay)
        self.assertEqual(result, 0)
        self.assertEqual(launch.call_count, 1)
        self.assertEqual(before, {name: (self.suite / name).read_bytes() for name in before})
        proof = json.loads((self.suite / "crash-diagnostic/inputs.json").read_text())
        self.assertTrue(proof["normal_files_unchanged"] and proof["binary_unchanged"])
        self.assertEqual(proof["original_result"]["status"], "failed")
        self.assertTrue(proof["debugger_result_is_not_gate_result"])

    def test_unset_test_threads_stays_unset(self):
        self.normal_environment["RUST_TEST_THREADS"] = None
        self.save("suite-environment.json", self.normal_environment)
        def replay(command, **kwargs):
            self.assertNotIn("RUST_TEST_THREADS", kwargs["env"])
            return types.SimpleNamespace(returncode=0)
        self.assertEqual(self.run_diagnostic(replay=replay)[0], 0)

    def test_non_mac_and_non_crash_do_not_launch_lldb(self):
        result, launch = self.run_diagnostic(platform="linux")
        self.assertEqual(result, 0)
        launch.assert_not_called()
        proof = json.loads((self.suite / "crash-diagnostic/inputs.json").read_text())
        self.assertEqual(proof["not_run_reason"], "not a recorded Mac SIGSEGV")

    def test_non_signal_failure_does_not_launch_lldb(self):
        (self.suite / "full-suite.log").write_text("RuntimeLimit: exceeded maximum runtime limit\n")
        result, launch = self.run_diagnostic()
        self.assertEqual(result, 0)
        launch.assert_not_called()

    def reject_before_launch(self):
        launch = MagicMock()
        with self.assertRaises(AssertionError):
            self.run_diagnostic(replay=launch)
        launch.assert_not_called()
        proof = json.loads((self.suite / "crash-diagnostic/inputs.json").read_text())
        self.assertFalse(proof["completed"])
        self.assertIsNone(proof["debugger_execution"])
        self.assertEqual(proof["original_result"]["status"], "failed")

    def test_illegal_argv_is_rejected_before_lldb(self):
        self.write_crash(argv="--include-ignored --nocapture --test-threads=1")
        self.reject_before_launch()

    def test_binary_outside_target_is_rejected_before_lldb(self):
        self.write_crash(binary=self.root / "wpt_smoke-outside")
        self.reject_before_launch()

    def test_non_wpt_binary_is_rejected_before_lldb(self):
        self.write_crash(binary=self.target / "other_integration-test")
        self.reject_before_launch()

    def test_debugger_nonzero_exit_keeps_original_failure(self):
        result, launch = self.run_diagnostic(replay=lambda *args, **kwargs: types.SimpleNamespace(returncode=19))
        self.assertEqual(result, 19)
        self.assertEqual(launch.call_count, 1)
        self.assertEqual(json.loads((self.suite / "result.json").read_text())["status"], "failed")

    def test_lldb_launch_has_original_args_environment_and_captures_before_kill(self):
        import hashlib
        lldb = types.SimpleNamespace(eLaunchFlagDisableASLR=2, eStateExited=10,
                                    eStateStopped=5, eStateCrashed=8)
        launch, process, target, debugger = (MagicMock() for _ in range(4))
        lldb.SBLaunchInfo = MagicMock(return_value=launch)
        lldb.SBError = MagicMock(return_value="mock launch status")
        lldb.SBDebugger = types.SimpleNamespace(StateAsCString=lambda state: "stopped")
        command_result = MagicMock()
        command_result.GetOutput.return_value, command_result.GetError.return_value = "mock stack\n", ""
        lldb.SBCommandReturnObject = MagicMock(return_value=command_result)
        execution = object()
        lldb.SBExecutionContext = MagicMock(return_value=execution)
        launch.GetLaunchFlags.side_effect = [3, 1]
        debugger.GetSelectedTarget.return_value = target
        target.GetExecutable.return_value = types.SimpleNamespace(fullpath=str(self.binary))
        target.GetTriple.return_value = "arm64-apple-macosx"
        target.Launch.return_value = process
        process.GetState.return_value, process.GetProcessID.return_value = 5, 123
        process.__iter__.return_value = iter([])
        output = self.root / "lldb"
        environment = dict(self.normal_environment, OMOIKANE_LLDB_OUTPUT=str(output),
                           OMOIKANE_LLDB_BINARY_SHA256=hashlib.sha256(self.binary.read_bytes()).hexdigest())
        def kill():
            self.assertIn("mock stack", (output / "native-stack.log").read_text())
            return "mock kill status"
        process.Kill.side_effect = kill
        with patch.dict(sys.modules, {"lldb": lldb}), patch.dict(os.environ, environment):
            capture.capture(debugger)
        lldb.SBLaunchInfo.assert_called_once_with(["--include-ignored", "--nocapture"])
        launch.SetWorkingDirectory.assert_called_once_with(os.getcwd())
        entries, append = launch.SetEnvironmentEntries.call_args.args
        self.assertFalse(append)
        self.assertIn("WPT_REQUIRED=1", entries)
        self.assertIn("WPT_MANIFEST=" + environment["WPT_MANIFEST"], entries)
        self.assertIn("RUST_TEST_THREADS=7", entries)
        launch.SetLaunchFlags.assert_called_once_with(1)
        launch.AddDuplicateFileAction.assert_called_once_with(1, 2)
        commands = [call.args[0] for call in debugger.GetCommandInterpreter.return_value.HandleCommand.call_args_list]
        self.assertEqual(commands, ["process status", "thread list", "thread backtrace all", "register read", "image list -o -f"])
        lldb.SBExecutionContext.assert_called_once_with(process)
        for call in debugger.GetCommandInterpreter.return_value.HandleCommand.call_args_list:
            self.assertIs(call.args[1], execution)
            self.assertIs(call.args[2], command_result)
        process.Kill.assert_called_once()
        process.Continue.assert_not_called()
        self.assertTrue(json.loads((output / "process.json").read_text())["binary_unchanged"])


    def test_lldb_commands_use_launched_process_inside_prelaunch_script_context(self):
        import hashlib
        lldb = types.SimpleNamespace(eLaunchFlagDisableASLR=2, eStateExited=10,
                                    eStateStopped=5, eStateCrashed=8)
        launch, process, target, debugger = (MagicMock() for _ in range(4))
        lldb.SBLaunchInfo = MagicMock(return_value=launch)
        lldb.SBError = MagicMock(return_value="success")
        lldb.SBDebugger = types.SimpleNamespace(StateAsCString=lambda state: "stopped")
        lldb.SBExecutionContext = lambda active: types.SimpleNamespace(process=active)

        class CommandResult:
            def __init__(self):
                self.output, self.error = "", ""

            def GetOutput(self):
                return self.output

            def GetError(self):
                return self.error

        lldb.SBCommandReturnObject = CommandResult
        launch.GetLaunchFlags.return_value = 1
        # LLDB --file already selected the right target. The outer script
        # command still has its pre-launch override context with no process.
        debugger.GetSelectedTarget.return_value = target
        target.GetExecutable.return_value = types.SimpleNamespace(fullpath=str(self.binary))
        target.GetTriple.return_value = "arm64-apple-macosx"
        target.Launch.return_value = process
        process.GetState.return_value, process.GetProcessID.return_value = 5, 64633
        process.__iter__.return_value = iter([])
        expected = {"process status": "Process 64633 stopped",
                    "thread list": "* thread #2: EXC_BAD_ACCESS",
                    "thread backtrace all": "thread #2: frame #0 JsObject::invoke",
                    "register read": "pc = 0x103315918",
                    "image list -o -f": "wpt_smoke image"}

        def interpret(command, *arguments):
            if len(arguments) == 1:
                result, active = arguments[0], None
            else:
                context, result = arguments
                active = context.process
            if active is process or command == "image list -o -f":
                result.output = expected[command] + "\n"
            else:
                result.error = "error: Command requires a current process.\n"

        debugger.GetCommandInterpreter.return_value.HandleCommand.side_effect = interpret
        output = self.root / "lldb-prelaunch-context"
        environment = dict(self.normal_environment, OMOIKANE_LLDB_OUTPUT=str(output),
                           OMOIKANE_LLDB_BINARY_SHA256=hashlib.sha256(self.binary.read_bytes()).hexdigest())

        def kill():
            saved = (output / "native-stack.log").read_text()
            self.assertNotIn("Command requires a current process", saved)
            for text in expected.values():
                self.assertIn(text, saved)
            return "success"

        process.Kill.side_effect = kill
        with patch.dict(sys.modules, {"lldb": lldb}), patch.dict(os.environ, environment):
            capture.capture(debugger)
        process.Continue.assert_not_called()
        process.Kill.assert_called_once()
        self.assertTrue(json.loads((output / "process.json").read_text())["completed"])


if __name__ == "__main__":
    unittest.main()
