"""Run brush's compat suite one test-case file at a time, with a tree-killing timeout.

On Windows the harness's per-case timeout kills only the direct child. A
grandchild that keeps the output pipe open (a background job, say) blocks
it forever, and run 4 lost its whole 2-hour budget to one such case. Here
each YAML file runs as its own invocation of the prebuilt test binary,
with output going to a file, not a pipe.

On Windows each invocation starts suspended inside a kill-on-close Job
Object, so every descendant is caught. `taskkill /T` was not enough: it
follows parent PIDs, an orphaned grandchild escaped it, and its open
handle made deleting the output file fail with WinError 32 (run 5). The
job also counts the processes each file spawned and how many were still
alive at the end (leaked background processes).

Each file is copied to tests/cases/set-NN/<same relative path>, the same
depth as tests/cases/compat, because cases reference ../../utils/.

Usage: python run_compat.py --brush-dir DIR --cases DIR [--target-dir DIR] --timeout SECS --summary MD --log LOG [--json JSON]

The shell under test is the harness's default (brush) unless BRUSH_PATH names another.
"""
import argparse
import glob
import os
import re
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

RAN = re.compile(
    r"(\d+) test case\(s\) ran: (\d+) succeeded, (\d+) failed, (\d+) known to fail, (\d+) skipped"
)
CASE = re.compile(r"^\* Test case: \[(.*?)\]")

if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    _k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _ntdll = ctypes.WinDLL("ntdll")

    class _IoCounters(ctypes.Structure):
        _fields_ = [(n, ctypes.c_ulonglong) for n in (
            "ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
            "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]

    class _BasicLimit(ctypes.Structure):
        _fields_ = [
            ("PerProcessUserTimeLimit", ctypes.c_int64),
            ("PerJobUserTimeLimit", ctypes.c_int64),
            ("LimitFlags", wintypes.DWORD),
            ("MinimumWorkingSetSize", ctypes.c_size_t),
            ("MaximumWorkingSetSize", ctypes.c_size_t),
            ("ActiveProcessLimit", wintypes.DWORD),
            ("Affinity", ctypes.c_size_t),
            ("PriorityClass", wintypes.DWORD),
            ("SchedulingClass", wintypes.DWORD),
        ]

    class _ExtendedLimit(ctypes.Structure):
        _fields_ = [
            ("BasicLimitInformation", _BasicLimit),
            ("IoInfo", _IoCounters),
            ("ProcessMemoryLimit", ctypes.c_size_t),
            ("JobMemoryLimit", ctypes.c_size_t),
            ("PeakProcessMemoryUsed", ctypes.c_size_t),
            ("PeakJobMemoryUsed", ctypes.c_size_t),
        ]

    class _BasicAccounting(ctypes.Structure):
        _fields_ = [
            ("TotalUserTime", ctypes.c_int64),
            ("TotalKernelTime", ctypes.c_int64),
            ("ThisPeriodTotalUserTime", ctypes.c_int64),
            ("ThisPeriodTotalKernelTime", ctypes.c_int64),
            ("TotalPageFaultCount", wintypes.DWORD),
            ("TotalProcesses", wintypes.DWORD),
            ("ActiveProcesses", wintypes.DWORD),
            ("TotalTerminatedProcesses", wintypes.DWORD),
        ]

    _k32.CreateJobObjectW.restype = wintypes.HANDLE
    _k32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
    _k32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    _k32.QueryInformationJobObject.argtypes = [
        wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD, wintypes.LPVOID]
    _k32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    _k32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
    _k32.CloseHandle.argtypes = [wintypes.HANDLE]
    _ntdll.NtResumeProcess.argtypes = [wintypes.HANDLE]

    _JOB_BASIC_ACCOUNTING = 1
    _JOB_EXTENDED_LIMIT = 9
    _KILL_ON_JOB_CLOSE = 0x2000
    _CREATE_SUSPENDED = 0x4

    def _check(ok, what):
        if not ok:
            raise OSError(ctypes.get_last_error(), what)

    class ProcessTree:
        """A process started suspended inside a kill-on-close Job Object."""

        def __init__(self, argv, **popen_kwargs):
            self.job = _k32.CreateJobObjectW(None, None)
            _check(self.job, "CreateJobObjectW")
            info = _ExtendedLimit()
            info.BasicLimitInformation.LimitFlags = _KILL_ON_JOB_CLOSE
            _check(_k32.SetInformationJobObject(
                self.job, _JOB_EXTENDED_LIMIT, ctypes.byref(info), ctypes.sizeof(info)),
                "SetInformationJobObject")
            self.proc = subprocess.Popen(argv, creationflags=_CREATE_SUSPENDED, **popen_kwargs)
            handle = wintypes.HANDLE(int(self.proc._handle))
            _check(_k32.AssignProcessToJobObject(self.job, handle), "AssignProcessToJobObject")
            _ntdll.NtResumeProcess(handle)

        def accounting(self):
            acct = _BasicAccounting()
            _k32.QueryInformationJobObject(
                self.job, _JOB_BASIC_ACCOUNTING, ctypes.byref(acct), ctypes.sizeof(acct), None)
            return acct.TotalProcesses, acct.ActiveProcesses

        def processes(self):
            """The command lines of the processes still in the job."""
            # JOBOBJECT_BASIC_PROCESS_ID_LIST (class 3): two DWORD counts (the second is how
            # many ids follow), then the ids.
            buf = (ctypes.c_size_t * 1025)()
            if not _k32.QueryInformationJobObject(self.job, 3, buf, ctypes.sizeof(buf), None):
                return []
            pids = [str(buf[1 + i]) for i in range(min(buf[0] >> 32, 1024))]
            if not pids:
                return []
            ps = ("Get-CimInstance Win32_Process | Where-Object { @(" + ",".join(pids) + ") -contains $_.ProcessId } | "
                  "ForEach-Object { [string]$_.ProcessId + ' (parent ' + $_.ParentProcessId + ') ' + $_.CommandLine }")
            out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True)
            return out.stdout.decode("utf-8", "replace").splitlines()

        def kill_all(self):
            _k32.TerminateJobObject(self.job, 1)
            self.proc.wait()
            _k32.CloseHandle(self.job)

else:

    class ProcessTree:
        """POSIX stand-in for local runs: a new session, killed as a group."""

        def __init__(self, argv, **popen_kwargs):
            self.proc = subprocess.Popen(argv, start_new_session=True, **popen_kwargs)

        def accounting(self):
            return None, None

        def processes(self):
            return []

        def kill_all(self):
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            self.proc.wait()


def test_binary(target_dir):
    pattern = "brush_compat_tests-*.exe" if os.name == "nt" else "brush_compat_tests-*"
    found = [
        p
        for p in glob.glob(str(target_dir / "release" / "deps" / pattern))
        if os.access(p, os.X_OK) and not p.endswith((".d", ".pdb"))
    ]
    if not found:
        sys.exit("run_compat: build the test binary first (cargo test --release --test brush-compat-tests --no-run)")
    return max(found, key=os.path.getmtime)


def run_set(exe, set_dir, cwd, timeout, out_path):
    """Returns (status, total_processes, processes_still_alive_at_end, what was running if it
    hung)."""
    with open(out_path, "wb") as out:
        tree = ProcessTree(
            [exe, "--test-cases-path", str(set_dir)],
            cwd=cwd,
            stdout=out,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )
        try:
            tree.proc.wait(timeout=timeout)
            status = "done"
        except subprocess.TimeoutExpired:
            status = "HUNG"
        total, alive = tree.accounting()
        running = tree.processes() if status == "HUNG" else []
        # Kill stragglers either way: leaked background processes would hold files open.
        tree.kill_all()
        return status, total, alive, running


def remove_with_retry(path, attempts=10):
    """Bounded retry: on Windows a just-killed process can hold a handle briefly."""
    for i in range(attempts):
        try:
            if path.is_dir():
                shutil.rmtree(path)
            elif path.exists():
                path.unlink()
            return True
        except OSError:
            time.sleep(0.5 * (i + 1))
    return False


def parse(text):
    m = None
    for m in RAN.finditer(text):
        pass
    counts = tuple(int(x) for x in m.groups()) if m else None
    unexpected, current = [], None
    for line in text.splitlines():
        c = CASE.match(line)
        if c:
            current = c.group(1)
            if "unexpected success" in line:
                unexpected.append(f"{current} (unexpected success)")
        elif line.strip() == "FAILED." and current:
            unexpected.append(current)
    errors = sum(1 for line in text.splitlines() if line.startswith("HARNESS-ERROR"))
    return counts, unexpected, errors


def write_summary(path, rows, totals, unexpected, timeout):
    hung = [r[0] for r in rows if r[1] == "HUNG"]
    leaky = [r for r in rows if r[6]]
    md = [
        "## brush compat suite on Windows, per file (oracle: Git Bash)",
        "",
        f"**Total: {totals[0]} ran: {totals[1]} succeeded, {totals[2]} failed, "
        f"{totals[3]} known to fail, {totals[4]} skipped.** "
        f"Files: {len(rows)}; hung (killed after {timeout}s): {len(hung)}"
        + (f" ({', '.join(hung)})" if hung else "")
        + f". Harness errors counted as failures: {sum(r[4] for r in rows)}. "
        f"Files that left processes running: {len(leaky)}.",
        "",
        "| file | status | s | ran | ok | fail | known | skip | harness errors | procs | left running |",
        "|---|---|---|---|---|---|---|---|---|---|---|",
    ]
    for rel, status, secs, counts, errors, total, alive in rows:
        c = counts or ("-",) * 5
        md.append(
            f"| {rel} | {status} | {secs} | " + " | ".join(map(str, c))
            + f" | {errors} | {'' if total is None else total} | {alive or ''} |"
        )
    if unexpected:
        md += ["", f"### Unexpected results ({len(unexpected)})", ""]
        md += [f"- {u}" for u in unexpected[:300]]
    with open(path, "w", encoding="utf-8") as fh:
        fh.write("\n".join(md) + "\n")
    return md


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--brush-dir", required=True, type=Path)
    ap.add_argument("--cases", required=True, type=Path)
    ap.add_argument("--target-dir", type=Path, help="cargo target dir (default: BRUSH_DIR/target)")
    ap.add_argument("--timeout", type=int, default=120)
    ap.add_argument("--summary", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--json", help="also write the totals and unexpected results as JSON")
    a = ap.parse_args()

    brush_dir = a.brush_dir.resolve()
    exe = test_binary((a.target_dir or brush_dir / "target").resolve())
    shell_dir = brush_dir / "brush-shell"
    cases_root = shell_dir / "tests" / "cases"
    cases = a.cases.resolve()
    files = sorted(cases.rglob("*.yaml"), key=lambda p: p.relative_to(cases).as_posix())

    rows, totals, all_unexpected = [], [0] * 5, []
    try:
        with open(a.log, "w", encoding="utf-8") as log:
            for i, f in enumerate(files):
                rel = f.relative_to(cases).as_posix()
                set_dir = cases_root / f"set-{i:03d}"
                remove_with_retry(set_dir)
                (set_dir / rel).parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(f, set_dir / rel)
                out_path = cases_root / f"set-{i:03d}.out"
                t0 = time.monotonic()
                status, total, alive, running = run_set(exe, set_dir, shell_dir, a.timeout, out_path)
                secs = round(time.monotonic() - t0)
                text = out_path.read_text(encoding="utf-8", errors="replace")
                remove_with_retry(out_path)
                remove_with_retry(set_dir)
                log.write(f"===== {rel} [{status}, {secs}s, procs={total}, left={alive}]\n{text}\n")
                if running:
                    log.write("Still running when it was stopped:\n" + "".join(f"  {r}\n" for r in running))
                log.flush()
                counts, unexpected, errors = parse(text)
                if counts:
                    totals = [t + c for t, c in zip(totals, counts)]
                all_unexpected += [f"{rel}: {u}" for u in unexpected]
                rows.append((rel, status, secs, counts, errors, total, alive))
                shown = "/".join(map(str, counts)) if counts else "no result"
                print(f"{rel}: {status} {secs}s {shown} harness-errors={errors} procs={total} left={alive}", flush=True)
    finally:
        # Always leave a summary, even if a later file blows up the runner.
        md = write_summary(a.summary, rows, totals, all_unexpected, a.timeout)
        print("\n".join(md[:4]))
        if a.json:
            import json
            with open(a.json, "w", encoding="utf-8") as fh:
                json.dump({
                    "ran": totals[0], "succeeded": totals[1], "failed": totals[2],
                    "known_to_fail": totals[3], "skipped": totals[4],
                    "hung": [r[0] for r in rows if r[1] == "HUNG"],
                    "unexpected": all_unexpected,
                }, fh, ensure_ascii=False, indent=1)


if __name__ == "__main__":
    main()
