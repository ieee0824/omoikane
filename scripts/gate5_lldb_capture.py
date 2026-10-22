"""Launch the recorded WPT executable and save its native LLDB stop state."""

import hashlib
import json
import os
from pathlib import Path
import time


def save_native_state(process, path):
    """Read the stopped inferior directly, independent of command selection."""
    with path.open("w") as log:
        log.write(f"process {process.GetProcessID()} state={process.GetState()}\n")
        for thread in process:
            log.write(f"thread {thread.GetThreadID()} name={thread.GetName()!r} "
                      f"stop={thread.GetStopDescription(4096)!r}\n")
            for index in range(thread.GetNumFrames()):
                frame = thread.GetFrameAtIndex(index)
                log.write(f"  frame #{index}: {frame}\n")
                for registers in frame.GetRegisters():
                    log.write(f"    {registers.GetName()}:\n")
                    for register in registers:
                        log.write(f"      {register.GetName()} = {register.GetValue()}\n")


def capture(debugger):
    """Save all thread stacks before terminating a stopped diagnostic process."""
    import lldb

    output = Path(os.environ["OMOIKANE_LLDB_OUTPUT"]).resolve()
    output.mkdir(parents=True, exist_ok=False)
    target = debugger.GetSelectedTarget()
    executable = Path(target.GetExecutable().fullpath).resolve()
    digest = lambda: hashlib.sha256(executable.read_bytes()).hexdigest()
    expected = os.environ["OMOIKANE_LLDB_BINARY_SHA256"]
    assert digest() == expected
    argv = ["--include-ignored", "--nocapture"]
    report = {"binary": str(executable), "binary_sha256": expected,
              "argv": argv, "cwd": os.getcwd(), "completed": False,
              "scope": "separate debugger execution; original Gate5 failure stays failed"}
    proof = output / "process.json"
    proof.write_text(json.dumps(report, indent=2) + "\n")
    debugger.SetAsync(False)
    launch = lldb.SBLaunchInfo(argv)
    launch.SetWorkingDirectory(os.getcwd())
    launch.SetEnvironmentEntries([key + "=" + value for key, value in os.environ.items()], False)
    launch.SetLaunchFlags(launch.GetLaunchFlags() & ~lldb.eLaunchFlagDisableASLR)
    report["launch_flags"] = launch.GetLaunchFlags()
    report["target_triple"] = target.GetTriple()
    assert launch.AddOpenFileAction(1, str(output / "target.log"), False, True)
    assert launch.AddDuplicateFileAction(1, 2)
    error = lldb.SBError()
    started = time.monotonic()
    process = target.Launch(launch, error)
    report.update(launch_error=str(error), pid=process.GetProcessID(),
                  seconds=time.monotonic() - started,
                  state=lldb.SBDebugger.StateAsCString(process.GetState()))
    # Script commands may retain a pre-launch execution context even when an
    # SBExecutionContext(process) is supplied. Avoid the command interpreter for
    # frames/registers: the SBProcess returned by Launch owns the stopped threads.
    save_native_state(process, output / "native-stack.log")
    with (output / "native-stack.log").open("a") as log:
        log.write("modules:\n")
        for module in target.modules:
            log.write(f"  {module.GetFileSpec()} "
                      f"load_address={module.GetObjectFileHeaderAddress().GetLoadAddress(target):#x}\n")
    report["threads"] = [
        {"id": thread.GetThreadID(), "name": thread.GetName(),
         "stop_reason": thread.GetStopReason(),
         "stop_description": thread.GetStopDescription(4096)} for thread in process]
    if process.GetState() == lldb.eStateExited:
        report["inferior_exit"] = process.GetExitStatus()
    elif process.GetState() in (lldb.eStateStopped, lldb.eStateCrashed):
        # Preserve all frames before killing the diagnostic inferior. No retry/continue.
        report["kill_error"] = str(process.Kill())
    report["binary_unchanged"] = digest() == expected
    report["completed"] = True
    proof.write_text(json.dumps(report, indent=2) + "\n")
