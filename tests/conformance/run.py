"""Runs the conformance cases (cases.py) against Bash shells on Windows.

Each shell runs each case as an agent harness runs commands: `<shell> -c <script>`,
started detached (no console, as Claude Code starts it), stdin from NUL, stdout and
stderr piped, in a fresh directory holding the case's files. The environment is the
runner's own, minus Python's UTF-8 switches (they would hide what the shell itself does).

Usage (on Windows):
  python run.py --shell sill=C:\\...\\bash.exe --shell gitbash=... --echoargs echoargs.exe
                --out results.json --report report.md [--only ID ...]
                [--mcp sill-mcp=C:\\...\\bash.exe]

--mcp runs each case through that shell's MCP server (`bash.exe --mcp`, started detached
in the case's directory, as an MCP client starts it): the script goes to its `bash` tool,
and the result's text is split back into stdout, stderr and the exit code. Its output is
text for a model, so CRLF arrives as LF there.
"""
import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from cases import CASES, text  # noqa: E402

DETACHED_PROCESS = 0x00000008
CATEGORY_TITLES = {
    "argv": "Arguments to native programs", "paths": "Paths", "encoding": "Encoding",
    "crlf": "Line endings and bytes", "process": "Processes", "limits": "Size limits",
    "scripts": "Running scripts", "tools": "Tools agents use", "files": "Files",
    "interactive": "Programs that prompt", "alike": "Look-alikes (sill's rules don't fire)",
    "errors": "Errors and identity",
}


class _Done(Exception):
    """Ends the launch step of `run_case` early (the MCP path has its results)."""


def shell_env():
    env = dict(os.environ)
    for k in ("PYTHONUTF8", "PYTHONIOENCODING"):
        env.pop(k, None)
    return env


def run_in_job(argv, timeout, **kw):
    """Runs `argv` (no console, like a harness's shell) and returns (stdout, stderr, code).
    On Windows inside a Job Object, so a timeout ends everything it started: killing only the
    shell would leave a grandchild (an editor git started) holding the output pipes, and
    reading them would wait forever. A run that ends in time leaves what it started running
    (the cases that check for leftovers look for them). On a timeout, raises
    subprocess.TimeoutExpired with what was read."""
    if os.name != "nt":
        p = subprocess.run(argv, capture_output=True, timeout=timeout, **kw)
        return p.stdout, p.stderr, p.returncode
    import ctypes
    from ctypes import wintypes
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    job = wintypes.HANDLE(k32.CreateJobObjectW(None, None))

    def kill_on_close(on):
        # JOBOBJECT_EXTENDED_LIMIT_INFORMATION (144 bytes on x64); LimitFlags at offset 16.
        info = (ctypes.c_byte * 144)()
        ctypes.c_uint32.from_buffer(info, 16).value = 0x2000 if on else 0
        k32.SetInformationJobObject(job, 9, info, 144)

    kill_on_close(True)
    p = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         creationflags=DETACHED_PROCESS | 0x4, **kw)  # 0x4: CREATE_SUSPENDED
    k32.AssignProcessToJobObject(job, wintypes.HANDLE(int(p._handle)))
    ctypes.WinDLL("ntdll").NtResumeProcess(wintypes.HANDLE(int(p._handle)))
    try:
        out, err = p.communicate(timeout=timeout)
        kill_on_close(False)
        return out, err, p.returncode
    except subprocess.TimeoutExpired:
        k32.TerminateJobObject(job, 1)
        out, err = p.communicate()
        raise subprocess.TimeoutExpired(argv, timeout, output=out, stderr=err) from None
    finally:
        k32.CloseHandle(job)


def processes_with(fragment):
    """Command lines of running processes that contain `fragment`."""
    ps = ("Get-CimInstance Win32_Process | Where-Object { $_.CommandLine } | "
          "ForEach-Object { $_.ProcessId.ToString() + ' ' + $_.CommandLine }")
    out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True).stdout
    lines = out.decode("utf-8", "replace").splitlines()
    return [l for l in lines if fragment in l and "Get-CimInstance" not in l]


def end(pids):
    for pid in pids:
        subprocess.run(["taskkill", "/T", "/F", "/PID", pid], capture_output=True)


def run_case(case, shell, echoargs, timeout, mcp=False):
    work = Path(tempfile.mkdtemp(prefix="conf-"))
    try:
        for rel, data in (case.get("files") or {}).items():
            p = work / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data if isinstance(data, bytes) else data.encode("utf-8"))
        script = case["script"].replace("{echoargs}", echoargs.replace("\\", "/"))
        t0 = time.monotonic()
        try:
            if mcp:
                out, err, code = run_via_mcp(shell, script, work, timeout)
                raise _Done
            # Past Windows' 32,767-character command line, a harness passes a file.
            if len(script) > 30000:
                (work / "_case.sh").write_bytes(script.encode("utf-8"))
                argv = [shell, "_case.sh"]
            else:
                argv = [shell, "-c", script]
            if case.get("after_running"):
                # A program `start` leaves running inherits every inheritable handle cmd.exe
                # has, a pipe's included (as under Git Bash), and a pipe would wait for it.
                with tempfile.TemporaryFile() as fo, tempfile.TemporaryFile() as fe:
                    p = subprocess.run(argv, cwd=work, env=shell_env(), stdin=subprocess.DEVNULL,
                                       stdout=fo, stderr=fe, creationflags=DETACHED_PROCESS,
                                       timeout=timeout)
                    fo.seek(0)
                    fe.seek(0)
                    out, err, code = fo.read(), fe.read(), p.returncode
            else:
                out, err, code = run_in_job(argv, timeout, cwd=work, env=shell_env(), stdin=subprocess.DEVNULL)
        except _Done:
            pass
        except subprocess.TimeoutExpired as e:
            out, err, code = e.stdout or b"", (e.stderr or b"") + b"\n[timed out]", -1
        secs = time.monotonic() - t0
        if "expect" in case:
            # Through MCP the tool ends output with a newline either way.
            ok = (text(out).rstrip("\n") == case["expect"].rstrip("\n")) if mcp else text(out) == case["expect"]
        else:
            try:
                ok = bool(case["check"](out, err, code, secs))
            except Exception:  # noqa: BLE001
                ok = False
        if case.get("after_running"):
            if secs > 10:
                ok = False
                err += b"\n[the shell waited for what the command left running]"
            time.sleep(1)
            alive = processes_with(case["after_running"])
            if not alive:
                ok = False
                err += f"\n[not running any more: {case['after_running']}]".encode()
            end([l.split()[0] for l in alive])
        if case.get("after_no_process"):
            left = processes_with(case["after_no_process"])
            if left:
                ok = False
                err += f"\n[left running: {left[0][:120]}]".encode()
                end([l.split()[0] for l in left])
        return {"pass": ok, "code": code, "secs": round(secs, 2),
                "stdout": text(out)[:600], "stderr": text(err)[:600]}
    finally:
        shutil.rmtree(work, ignore_errors=True)


class McpClient:
    """Just enough of an MCP client (stdio, JSON-RPC lines) to call one server's tools."""

    def __init__(self, argv, cwd, env):
        self.p = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, creationflags=DETACHED_PROCESS)
        self.n = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "conformance", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    def send(self, msg):
        self.p.stdin.write((json.dumps(msg) + "\n").encode())
        self.p.stdin.flush()

    def request(self, method, params):
        self.n += 1
        self.send({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params})
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("MCP server closed")
            msg = json.loads(line)
            if msg.get("id") == self.n:
                if "error" in msg:
                    raise RuntimeError(msg["error"])
                return msg["result"]

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:  # noqa: BLE001
            self.p.kill()


def run_via_mcp(shell, script, work, timeout):
    """Runs `script` with the shell's MCP `bash` tool; returns (stdout, stderr, code)."""
    client = McpClient([shell, "--mcp"], work, shell_env())
    try:
        result = client.request("tools/call", {"name": "bash", "arguments": {
            "command": script, "timeout_ms": min(timeout * 1000, 600000)}})
    finally:
        client.close()
    text = "".join(c.get("text", "") for c in result.get("content", []))
    # The output ends with a footer line, `--- exit N · cwd ... ---` (or `--- stopped after
    # ...`), and maybe a note after it (programs the command left running).
    footers = list(re.finditer(r"^--- (?:exit (-?\d+)\b|stopped )[^\n]*---$", text, re.M))
    m = footers[-1] if footers else None
    body = text[:m.start()].removesuffix("\n") if m else text
    code = int(m.group(1)) if m and m.group(1) else -1
    if body.startswith("--- stderr ---\n"):
        out_part, err_part = "", body[len("--- stderr ---\n"):]
    elif "\n--- stderr ---\n" in body:
        out_part, err_part = body.split("\n--- stderr ---\n", 1)
    else:
        out_part, err_part = body, ""
    # The tool ends each stream with a newline; output without one can't be told apart.
    out = out_part + "\n" if out_part else ""
    err = err_part + "\n" if err_part else ""
    return out.encode("utf-8"), err.encode("utf-8"), code


def version(shell):
    try:
        p = subprocess.run([shell, "--version"], capture_output=True, timeout=30, stdin=subprocess.DEVNULL)
        return text(p.stdout).splitlines()[0][:80]
    except Exception:  # noqa: BLE001
        return "?"


def report(results, shells, meta):
    names = [n for n, _ in shells]
    cats = list(dict.fromkeys(c["cat"] for c in CASES))
    md = ["# Bash-on-Windows conformance for coding agents", "",
          f"Machine: {meta['os']}, locale code page {meta['acp']}. Run: {meta['date']}.", "",
          "| shell | version |", "|---|---|"]
    md += [f"| {n} | {meta['versions'][n]} |" for n in names]
    md += ["", "## Score by category", "", "| category | cases | " + " | ".join(names) + " |",
           "|---|---|" + "---|" * len(names)]
    totals = defaultdict(int)
    applicable = 0
    for cat in cats:
        ids = [c["id"] for c in CASES if c["cat"] == cat and c["id"] in results]
        applicable += len(ids)
        row = []
        for n in names:
            k = sum(results[i][n]["pass"] for i in ids)
            totals[n] += k
            row.append(f"{k}/{len(ids)}")
        md.append(f"| {CATEGORY_TITLES.get(cat, cat)} | {len(ids)} | " + " | ".join(row) + " |")
    md.append(f"| **total** | {applicable} | " + " | ".join(f"**{totals[n]}/{applicable}**" for n in names) + " |")
    md += ["", "## Cases", "", "| case | why | " + " | ".join(names) + " |", "|---|---|" + "---|" * len(names)]
    for c in CASES:
        if c["id"] not in results:
            continue
        cells = ["✅" if results[c["id"]][n]["pass"] else "❌" for n in names]
        md.append(f"| `{c['id']}` | {c['why']} | " + " | ".join(cells) + " |")
    md += ["", "## Failures", ""]
    for c in CASES:
        for n in names:
            r = results.get(c["id"], {}).get(n)
            if r and not r["pass"]:
                got = (r["stdout"].strip() or r["stderr"].strip()).replace("\n", " ⏎ ")[:160]
                want = c.get("expect", "(check)").replace("\n", " ⏎ ")[:60]
                md.append(f"- **{n}** `{c['id']}`: expected `{want}`, got `{got}` (exit {r['code']})")
    return "\n".join(md) + "\n"


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    ap = argparse.ArgumentParser()
    ap.add_argument("--shell", action="append", default=[], help="NAME=PATH")
    ap.add_argument("--mcp", action="append", default=[], help="NAME=PATH of a bash.exe run as its MCP server")
    ap.add_argument("--echoargs", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--report", required=True)
    ap.add_argument("--only", action="append")
    ap.add_argument("--timeout", type=int, default=60)
    a = ap.parse_args()
    shells = [s.split("=", 1) for s in a.shell] + [s.split("=", 1) for s in a.mcp]
    mcp_names = {s.split("=", 1)[0] for s in a.mcp}
    import ctypes
    meta = {"os": f"Windows {platform.version()}", "acp": ctypes.windll.kernel32.GetACP(),
            "date": time.strftime("%Y-%m-%d %H:%M"), "versions": {n: version(p) for n, p in shells}}
    results = {}
    for case in CASES:
        if a.only and case["id"] not in a.only:
            continue
        if any(shutil.which(t) is None for t in case.get("needs", [])):
            print(f"{case['id']:28} skipped (needs {case['needs']})", flush=True)
            continue
        results[case["id"]] = {}
        for name, path in shells:
            if name in mcp_names and case.get("direct_only"):
                results[case["id"]][name] = {"pass": True, "code": 0, "secs": 0, "stdout": "(direct only)", "stderr": ""}
                continue
            r = run_case(case, path, os.path.abspath(a.echoargs), case.get("timeout", a.timeout),
                         mcp=name in mcp_names)
            results[case["id"]][name] = r
        print(f"{case['id']:28} " + " ".join(f"{n}={'pass' if results[case['id']][n]['pass'] else 'FAIL'}"
                                              for n, _ in shells), flush=True)
    Path(a.out).write_text(json.dumps({"meta": meta, "results": results}, ensure_ascii=False, indent=1),
                           encoding="utf-8")
    Path(a.report).write_text(report(results, shells, meta), encoding="utf-8")


if __name__ == "__main__":
    main()
