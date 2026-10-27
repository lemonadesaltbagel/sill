"""Fails when brush's compat suite on Windows got worse than the committed baseline.

The runner (run_compat.py --json) lists the cases with an unexpected result: failures
(beyond the cases marked `known_failure`, which fail everywhere) and unexpected
successes. The baseline (baseline-windows.txt) holds the failures accepted today, one
`file.yaml: case name` per line, a `HUNG file.yaml` line for a file allowed to hang, and
`succeeded >= N`. A failure or hang not in it, or fewer successes, is a regression. A
baseline failure that now passes is reported, so the baseline can shrink.

Usage: python check_baseline.py RESULTS.json BASELINE.txt
"""
import json
import sys


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    results = json.load(open(sys.argv[1], encoding="utf-8"))
    allowed, hung_ok, min_ok = set(), set(), 0
    for line in open(sys.argv[2], encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("succeeded >= "):
            min_ok = int(line.removeprefix("succeeded >= "))
        elif line.startswith("HUNG "):
            hung_ok.add(line.removeprefix("HUNG "))
        else:
            allowed.add(line)

    failures = [u for u in results["unexpected"] if not u.endswith("(unexpected success)")]
    new = [f for f in failures if f not in allowed]
    new_hung = [h for h in results["hung"] if h not in hung_ok]
    fixed = sorted(allowed - set(failures))

    print(f"compat: {results['succeeded']} of {results['ran']} passed (baseline: at least {min_ok}); "
          f"{len(failures)} failures, {len(allowed)} accepted")
    if fixed:
        print(f"\nNow passing ({len(fixed)}); take them out of the baseline:")
        print("\n".join(f"  {f}" for f in fixed))
    problems = []
    if new:
        problems.append(f"New failures ({len(new)}):\n" + "\n".join(f"  {f}" for f in new))
    if new_hung:
        problems.append("Files that hung: " + ", ".join(new_hung))
    if results["succeeded"] < min_ok:
        problems.append(f"Fewer cases passed than the baseline: {results['succeeded']} < {min_ok}")
    if problems:
        print("\nREGRESSION\n" + "\n\n".join(problems))
        sys.exit(1)


if __name__ == "__main__":
    main()
