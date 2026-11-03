"""Isolated Linux subreaper for a leader-exits/member-ignores-TERM regression.

Only this test process adopts grandchildren, so the test never relies on the
container's PID 1 reaping zombies. Production cleanup must prove group exit.
"""

import ctypes
import importlib.util
import inspect
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time


def load(path):
    spec = importlib.util.spec_from_file_location("group_fixture_subject", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def exists(pgid):
    try:
        os.killpg(pgid, 0)
        return True
    except ProcessLookupError:
        return False


def start_reaper(member):
    def reap():
        while True:
            try:
                os.waitpid(member, 0)
                return
            except ChildProcessError:
                time.sleep(0.005)
    thread = threading.Thread(target=reap, daemon=True)
    thread.start()
    return thread


def main(kind, subject, output):
    # Linux PR_SET_CHILD_SUBREAPER, declared in linux/prctl.h.
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    module = load(subject)
    output.mkdir(parents=True, exist_ok=True)
    marker = output / "member.json"
    member_code = (
        "import os,signal,time,json,sys\nfrom pathlib import Path\n"
        "signal.signal(signal.SIGTERM,signal.SIG_IGN)\n"
        "Path(sys.argv[1]).write_text(json.dumps([os.getppid(),os.getpid(),os.getpgrp()]))\n"
        "time.sleep(60)\n")
    leader_code = (
        "import subprocess,sys,time\n"
        f"subprocess.Popen([sys.executable,'-c',{member_code!r},sys.argv[1]])\n"
        "time.sleep(60)\n")
    command = [sys.executable, "-c", leader_code, str(marker)]
    leader, member, pgid, reaper = None, None, None, None

    def ready():
        nonlocal member, pgid, reaper
        if not marker.exists():
            return False
        _, member, pgid = json.loads(marker.read_text())
        if reaper is None:
            reaper = start_reaper(member)
        return True

    try:
        if kind == "guard":
            leader = subprocess.Popen(command, start_new_session=True)
            owned_pgid = leader.pid
            deadline = time.monotonic() + 5
            while not ready() and time.monotonic() < deadline:
                time.sleep(0.005)
            assert member is not None
            if "pgid" in inspect.signature(module.stop_process_group).parameters:
                module.stop_process_group(leader, timeout_seconds=0.05, pgid=owned_pgid)
            elif "timeout_seconds" in inspect.signature(module.stop_process_group).parameters:
                module.stop_process_group(leader, timeout_seconds=0.05)
            else:
                module.stop_process_group(leader)
            result = {"leader_exit": leader.poll(), "owned_pgid": owned_pgid}
        else:
            def usage(_):
                return (1000, 999, 1) if ready() else (1000, 0, 1000)
            code = module.run_monitored(command, output / "evidence", output,
                                        dict(os.environ), 100, 0.005, usage=usage)
            result = json.loads((output / "evidence/capacity-result.json").read_text())
            assert code == 75 and result["status"] == "failed" and "cleanup_error" not in result
        result["member"] = member
        if reaper is not None:
            reaper.join(timeout=1)
        result["group_exists_after_cleanup"] = exists(pgid)
        print(json.dumps(result), flush=True)
        assert not result["group_exists_after_cleanup"], "TERM-resistant descendant survived leader exit"
    finally:
        if pgid is None and marker.exists():
            _, member, pgid = json.loads(marker.read_text())
        if pgid is not None:
            try:
                os.killpg(pgid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        if leader is not None:
            leader.wait(timeout=2)
        if reaper is not None:
            reaper.join(timeout=2)


if __name__ == "__main__":
    main(sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]))
