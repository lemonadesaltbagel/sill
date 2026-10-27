"""Copy brush's compat test-case tree, dropping cases Windows can't host:
`pty: true` cases, and cases whose fixture files have names that are illegal
on Windows (`a:1`, `file*.txt`, ...). Neither kind tests brush itself.

brush's harness aborts the whole run at the first pty case on platforms
without pty support (brush-test-harness/src/execution.rs, the
`cfg(not(pty))` arm), so on Windows no case runs at all. Files with nothing
dropped are copied byte-for-byte; only files that lose cases are re-serialized.

Usage: python filter_cases.py SRC_DIR DST_DIR
"""
import re
import shutil
import sys
from pathlib import Path

import yaml

WINDOWS_ILLEGAL = re.compile(r'[<>:"|?*\x00-\x1f]|^(con|prn|aux|nul|com\d|lpt\d)(\.|$)|[ .]$', re.I)


def windows_illegal(case):
    paths = [f.get("path", "") for f in case.get("test_files") or []]
    return any(WINDOWS_ILLEGAL.search(seg) for p in paths for seg in p.split("/"))


def drop(case):
    if case.get("pty"):
        return "pty"
    if windows_illegal(case):
        return "windows-illegal fixture name"
    return None


def main():
    src, dst = Path(sys.argv[1]), Path(sys.argv[2])
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)
    dropped = 0
    for path in sorted(dst.rglob("*.yaml")):
        text = path.read_text(encoding="utf-8")
        doc = yaml.safe_load(text)
        cases = doc.get("cases") or []
        kept = []
        for c in cases:
            reason = drop(c)
            if reason:
                print(f"{path.relative_to(dst)}: drop [{reason}] {c.get('name')}")
            else:
                kept.append(c)
        if len(kept) == len(cases):
            continue
        dropped += len(cases) - len(kept)
        doc["cases"] = kept
        path.write_text(
            yaml.safe_dump(doc, sort_keys=False, allow_unicode=True, width=1 << 16),
            encoding="utf-8",
            newline="\n",
        )
    print(f"total cases dropped: {dropped}")


if __name__ == "__main__":
    main()
