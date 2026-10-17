"""LLDB API regression tests; no debugger or native executable required."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    "gate5_lldb_capture", Path(__file__).resolve().parents[1] / "gate5_lldb_capture.py")
capture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(capture)


class Value(list):
    def __init__(self, name, value=None, children=()):
        super().__init__(children)
        self.name, self.value = name, value

    def GetName(self):
        return self.name

    def GetValue(self):
        return self.value


class Frame:
    def __init__(self, name):
        self.name = name

    def __str__(self):
        return self.name

    def GetRegisters(self):
        return [Value("general", children=[Value("pc", "0x1234"), Value("sp", "0x5678")])]


class Thread:
    def __init__(self, identifier):
        self.identifier = identifier

    def GetThreadID(self):
        return self.identifier

    def GetName(self):
        return f"worker-{self.identifier}"

    def GetStopDescription(self, limit):
        return "signal SIGABRT"

    def GetNumFrames(self):
        return 2

    def GetFrameAtIndex(self, index):
        return Frame(f"function-{self.identifier}-{index}")


class Process(list):
    def GetProcessID(self):
        return 42

    def GetState(self):
        return 5  # eStateStopped


class Result:
    error = ""

    def GetError(self):
        return self.error


class StaleCommandInterpreter:
    """Model script-command context missing a selected stopped thread/frame."""
    def HandleCommand(self, command, execution, result):
        result.error = "error: Command requires a process which is currently stopped."


class CaptureTests(unittest.TestCase):
    def test_stopped_process_is_saved_despite_stale_command_context(self):
        process = Process([Thread(7), Thread(8)])
        # The previous implementation's command route fails even with a valid
        # stopped process, as observed in the Darwin artifact.
        result = Result()
        StaleCommandInterpreter().HandleCommand("thread backtrace all", process, result)
        self.assertIn("currently stopped", result.GetError())
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native-stack.log"
            capture.save_native_state(process, path)
            output = path.read_text()
        self.assertIn("process 42 state=5", output)
        for identifier in (7, 8):
            self.assertIn(f"thread {identifier}", output)
            for index in (0, 1):
                self.assertIn(f"function-{identifier}-{index}", output)
        self.assertEqual(output.count("pc = 0x1234"), 4)
        self.assertEqual(output.count("sp = 0x5678"), 4)


if __name__ == "__main__":
    unittest.main()
