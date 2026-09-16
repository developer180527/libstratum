#!/usr/bin/env python3
"""Opens every binary the host ships and checks that libstratum never crashes or hangs.

The fixture corpus is small, clean and built by us; a host's own system binaries are none of
those things. They carry the shapes no fixture has: resource-only and managed PEs, ancient
32-bit images, stripped and prelinked ELF, universal Mach-O, and plenty of files that only
look like binaries. Every one of them must produce an answer or an honest diagnostic --
never a panic, an abort, or a wedged process.

    python3 scripts/sweep.py
    python3 scripts/sweep.py --info target/debug/examples/info --max-files 500
    python3 scripts/sweep.py --root /opt/homebrew/bin

Exit status: 0 when nothing crashed, 1 when something did (or the corpus came up empty,
which means the roots are wrong and the sweep proved nothing).
"""
from __future__ import annotations

import argparse
import os
import platform
import re
import subprocess
import sys
from collections import Counter
from itertools import zip_longest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Where each host keeps the binaries it actually runs, as (directory, glob, recurse). The point is
# breadth of shape, not exhaustiveness, so these are globs rather than walks of the whole filesystem.
ROOTS: dict[str, list[tuple[str, str, bool]]] = {
    "Windows": [
        (r"C:\Windows\System32", "*.dll", False),
        (r"C:\Windows\System32", "*.exe", False),
        (r"C:\Windows\System32", "*.sys", False),       # drivers: PE with no entry point
        (r"C:\Windows\SysWOW64", "*.dll", False),       # 32-bit PE32, not PE32+
        (r"C:\Windows\Microsoft.NET", "*.dll", True),   # managed: CLR header, no native code
    ],
    "Linux": [
        ("/usr/bin", "*", False),
        ("/usr/sbin", "*", False),
        ("/usr/lib", "*.so*", True),
        ("/usr/libexec", "*", False),
    ],
    "Darwin": [
        ("/usr/bin", "*", False),
        ("/bin", "*", False),
        ("/usr/lib", "*.dylib", False),
        ("/usr/libexec", "*", False),
    ],
}

# A tool that is missing or refuses to run would otherwise look like a clean sweep.
TIMEOUT_SECONDS = 30


def default_info() -> Path:
    exe = "info.exe" if platform.system() == "Windows" else "info"
    return ROOT / "target" / "debug" / "examples" / exe


def collect(roots: list[tuple[str, str, bool]], max_files: int) -> list[Path]:
    per_root: list[list[Path]] = []
    for directory, pattern, recurse in roots:
        base = Path(directory)
        if not base.is_dir():
            continue
        try:
            found = sorted(p for p in (base.rglob(pattern) if recurse else base.glob(pattern)) if p.is_file())
        except OSError:
            continue  # unreadable directory: not this script's problem
        if found:
            per_root.append(found)

    # Round-robin, not concatenate-then-truncate: a cap applied to one sorted list would be spent
    # entirely on the first directory, and the roots that carry the unusual shapes (SysWOW64's
    # 32-bit images, the managed assemblies) would never be reached.
    files: list[Path] = []
    seen: set[Path] = set()
    for column in zip_longest(*per_root):
        for path in column:
            if path is not None and path not in seen:
                seen.add(path)
                files.append(path)
    return files[:max_files] if max_files else files


def classify(result: subprocess.CompletedProcess[str]) -> tuple[str, str]:
    """-> (bucket, detail). Only 'crash' fails the sweep; a refusal with a diagnostic is a pass."""
    if result.returncode == 0:
        formats = re.findall(r"format: (\w+)", result.stdout)
        return "opened", formats[0] if formats else "unknown"
    if result.returncode == 1:
        message = result.stderr.strip().splitlines()[-1] if result.stderr.strip() else "(no message)"
        message = re.sub(r"^.*?: ", "", message, count=1)
        if "no registered format" in message:
            return "not-a-binary", message
        return "refused", message
    # Anything else is the bug this sweep exists to find: panic (101), abort, or a signal.
    return "crash", f"exit {result.returncode}: {(result.stderr.strip().splitlines() or ['(silent)'])[-1]}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--info", type=Path, default=default_info(), help="the `info` example binary")
    parser.add_argument("--root", action="append", help="sweep this directory instead of the host defaults")
    parser.add_argument("--max-files", type=int, default=4000, help="cap the corpus (0 = no cap)")
    args = parser.parse_args()

    if not args.info.is_file():
        print(f"{args.info} not found: cargo build --examples -p libstratum", file=sys.stderr)
        return 1

    roots = [(r, "*", True) for r in args.root] if args.root else ROOTS.get(platform.system(), [])
    if not roots:
        print(f"no sweep roots for {platform.system()}", file=sys.stderr)
        return 1

    files = collect(roots, args.max_files)
    if not files:
        print(f"no files found under {[r for r, _ in roots]}", file=sys.stderr)
        return 1

    buckets: Counter[str] = Counter()
    formats: Counter[str] = Counter()
    messages: Counter[str] = Counter()
    crashes: list[tuple[Path, str]] = []

    for path in files:
        try:
            result = subprocess.run(
                [str(args.info), str(path)], capture_output=True, text=True, errors="replace",
                timeout=TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            buckets["crash"] += 1
            crashes.append((path, f"hung for more than {TIMEOUT_SECONDS}s"))
            continue
        except OSError as err:
            buckets["refused"] += 1
            messages[f"could not execute: {err}"] += 1
            continue
        bucket, detail = classify(result)
        buckets[bucket] += 1
        if bucket == "opened":
            formats[detail] += 1
        elif bucket == "crash":
            crashes.append((path, detail))
        else:
            messages[detail] += 1

    lines = [
        f"host: {platform.system()} {platform.machine()}   files: {len(files)}",
        f"opened: {buckets['opened']}   not-a-binary: {buckets['not-a-binary']}   "
        f"refused: {buckets['refused']}   CRASHES: {buckets['crash']}",
        "formats: " + (", ".join(f"{name} {n}" for name, n in formats.most_common()) or "none"),
    ]
    for message, n in messages.most_common(8):
        lines.append(f"  {n:5} x {message[:110]}")
    for path, detail in crashes[:20]:
        lines.append(f"  CRASH {path}: {detail[:110]}")
    report = "\n".join(lines)
    print(report)

    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"### sweep: {platform.system()}\n\n```\n{report}\n```\n")

    return 1 if buckets["crash"] else 0


if __name__ == "__main__":
    sys.exit(main())
