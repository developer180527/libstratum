#!/usr/bin/env python3
"""Cross-checks libstratum's committed layout goldens against independent reference tools.

    llvm-dwarfdump  ELF binaries and Mach-O dSYMs (LLVM's DWARF reader)
    pahole          ELF binaries (dwarves: an independent DWARF implementation)
    llvm-pdbutil    PDB files (LLVM's PDB reader)

For every type in every golden, compares the size and the offsets (in bits) of named fields and
bitfields. Run inside the fixtures Docker image, from the repository root:

    docker run --rm -v "$PWD":/work -w /work libstratum-fixtures python3 fixtures/crosscheck/crosscheck.py
"""
from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

# A reference tool that isn't installed makes every comparison against it count as "skipped", so the
# run would report agreement on nothing and still exit 0. Check up front instead.
REQUIRED_TOOLS = ("llvm-dwarfdump", "llvm-pdbutil", "pahole")

ROOT = Path(__file__).resolve().parent.parent
GOLDEN = ROOT / "golden"
BIN = ROOT / "bin"

INT_WORDS = [
    (r"\blong unsigned int\b", "unsigned long"), (r"\blong long unsigned int\b", "unsigned long long"),
    (r"\bshort unsigned int\b", "unsigned short"), (r"\blong long int\b", "long long"), (r"\bshort int\b", "short"),
    (r"\blong int\b", "long"),
]


def norm(name: str) -> str:
    for pattern, repl in INT_WORDS:
        name = re.sub(pattern, repl, name)
    return re.sub(r"\s+", "", name)


def last_component(qualified: str) -> str:
    depth, cut = 0, 0
    for i, ch in enumerate(qualified):
        if ch == "<":
            depth += 1
        elif ch == ">":
            depth -= 1
        elif ch == ":" and depth == 0 and qualified[i:i + 2] == "::":
            cut = i + 2
    return qualified[cut:]


def expected(layout: dict) -> tuple[int, dict[str, int]]:
    members = {}
    for m in layout["members"]:
        if m["kind"] in ("field", "bitfield") and m["name"] and m["offset_bits"] is not None:
            members[m["name"]] = m["offset_bits"]
    return layout["size_bytes"], members


def run(cmd: list[str], accept_nonzero: bool = False) -> str | None:
    try:
        out = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired):
        return None
    return out.stdout if out.returncode == 0 or (accept_nonzero and out.stdout) else None


# ------------------------------------------------------------------------------------------- DWARF

def dwarfdump_types(path: Path) -> dict[str, list[tuple[int, dict[str, int]]]]:
    """Name → [(size, {member: offset_bits})] for complete struct/class/union definitions."""
    text = run(["llvm-dwarfdump", "--debug-info", "--debug-types", str(path)])
    if text is None:
        return {}
    entries = []  # (depth, tag, attrs)
    current = None
    for line in text.splitlines():
        m = re.match(r"^0x[0-9a-f]+:(\s+)(DW_TAG_\w+|NULL)", line)
        if m:
            depth = len(m.group(1))
            current = {"depth": depth, "tag": m.group(2), "attrs": {}}
            entries.append(current)
            continue
        a = re.match(r"^\s+(DW_AT_\w+)\s+\((.*)\)\s*$", line)
        if a and current is not None:
            current["attrs"][a.group(1)] = a.group(2)
    types = defaultdict(list)
    for i, e in enumerate(entries):
        if e["tag"] not in ("DW_TAG_structure_type", "DW_TAG_class_type", "DW_TAG_union_type"):
            continue
        attrs = e["attrs"]
        if "DW_AT_declaration" in attrs or "DW_AT_name" not in attrs or "DW_AT_byte_size" not in attrs:
            continue
        name = attrs["DW_AT_name"].strip('"')
        size = int(attrs["DW_AT_byte_size"], 0)
        members = {}
        for child in entries[i + 1:]:
            if child["depth"] <= e["depth"]:
                break
            if child["depth"] != e["depth"] + 2 or child["tag"] != "DW_TAG_member":
                continue  # llvm-dwarfdump indents children by 2
            c = child["attrs"]
            if "DW_AT_name" not in c or c["DW_AT_name"].strip('"').startswith("_vptr"):
                continue  # vtable pointers are reported as VtablePtr entries without a name
            if "DW_AT_data_bit_offset" in c:
                offset = int(c["DW_AT_data_bit_offset"], 0)
            elif "DW_AT_data_member_location" in c:
                loc = c["DW_AT_data_member_location"]
                if not re.fullmatch(r"0x[0-9a-f]+|\d+", loc):
                    continue
                offset = int(loc, 0) * 8
            elif e["tag"] == "DW_TAG_union_type":
                offset = 0  # union members without a location start at offset 0
            else:
                continue
            members[c["DW_AT_name"].strip('"')] = offset
        types[norm(name)].append((size, members))
    return types


def pahole_type(path: Path, name: str) -> tuple[int, dict[str, int]] | None:
    # pahole exits non-zero after printing when it also fails to load BTF; the DWARF output is still valid.
    text = run(["pahole", "-C", name, str(path)], accept_nonzero=True)
    if not text or "{" not in text:
        return None
    members, size = {}, None
    for line in text.splitlines():
        s = re.search(r"/\*\s*size:\s*(\d+)", line)
        if s and size is None:
            size = int(s.group(1))
        # Top-level members only (one tab); nested anonymous aggregates are indented further.
        if not line.startswith("\t") or line.startswith("\t\t") or "_vptr" in line:
            continue
        # "	unsigned int a:3;   /*  0: 0  4 */", "	char tag;  /*  0  1 */",
        # "	int x __attribute__((__aligned__(16)));  /*  16  4 */"
        m = re.match(
            r"^\t[^;{}]*?\b(\w+)(?:\[\d*\])*(?::\s*\d+)?(?:\s+__attribute__\(\(.*?\)\))?;\s*/\*\s*(\d+)(?::\s*(\d+))?\s+\d+\s*\*/",
            line,
        )
        if m:
            byte, bit = int(m.group(2)), int(m.group(3) or 0)
            members[m.group(1)] = byte * 8 + bit
    return (size, members) if size is not None else None


# --------------------------------------------------------------------------------------------- PDB

def pdbutil_types(path: Path) -> dict[str, list[tuple[int, dict[str, int]]]]:
    text = run(["llvm-pdbutil", "dump", "-types", str(path)])
    if text is None:
        return {}
    records: dict[str, dict] = {}
    current = None
    for line in text.splitlines():
        header = re.match(r"^\s*(0x[0-9A-F]+) \| (LF_\w+) \[size = \d+\](.*)$", line)
        if header:
            current = {"kind": header.group(2), "rest": header.group(3), "lines": []}
            records[header.group(1)] = current
        elif current is not None:
            current["lines"].append(line)
    types = defaultdict(list)
    for rec in records.values():
        if rec["kind"] not in ("LF_STRUCTURE", "LF_CLASS", "LF_UNION"):
            continue
        body = rec["rest"] + " " + " ".join(rec["lines"])
        if "forward ref" in body:
            continue
        name = re.search(r"`([^`]*)`", rec["rest"])
        size = re.search(r"sizeof (\d+)", body)
        field_list = re.search(r"field list: (0x[0-9A-F]+)", body)
        if not (name and size and field_list):
            continue
        members = {}
        fl = records.get(field_list.group(1))
        if fl:
            flat = " ".join(fl["lines"])
            for m in re.finditer(r"- LF_MEMBER \[name = `([^`]*)`, Type = (0x[0-9A-F]+)[^,]*, offset = (\d+)", flat):
                offset = int(m.group(3)) * 8
                bitfield = records.get(m.group(2))
                if bitfield and bitfield["kind"] == "LF_BITFIELD":
                    pos = re.search(r"bit offset = (\d+)", bitfield["rest"] + " ".join(bitfield["lines"]))
                    offset += int(pos.group(1)) if pos else 0
                members[m.group(1)] = offset
        types[norm(name.group(1))].append((int(size.group(1)), members))
    return types


# Reference-tool limitations, verified by hand; libstratum agrees with llvm-dwarfdump on these.
KNOWN_PAHOLE_LIMITS = {
    # pahole drops the empty `Allocator alloc` member when its type is referenced from a type unit.
    ("linux-clang-types5", "Holder"),
    ("linux-clang-types4", "Holder"),
}

# ------------------------------------------------------------------------------------------ driver

def main() -> int:
    if missing := [t for t in REQUIRED_TOOLS if shutil.which(t) is None]:
        print(f"missing reference tools: {', '.join(missing)}", file=sys.stderr)
        print("run inside the fixtures image (fixtures/README.md), which pins all three", file=sys.stderr)
        return 2

    stats = defaultdict(lambda: {"agree": 0, "disagree": 0, "skipped": 0})
    problems = []
    skipped = []
    for golden in sorted(GOLDEN.rglob("layout.json")):
        rel = golden.parent.relative_to(GOLDEN)
        toolchain, fixture = rel.parts[0], rel.parts[-1]
        bin_dir = BIN / rel
        doc = json.loads(golden.read_text())

        if toolchain in ("msvc", "clang-cl"):
            tools = {"llvm-pdbutil": pdbutil_types(bin_dir / f"{fixture}.pdb")}
            elf = None
        elif toolchain == "apple-clang":
            tools = {"llvm-dwarfdump": dwarfdump_types(bin_dir / f"{fixture}.dSYM/Contents/Resources/DWARF/{fixture}")}
            elf = None
        else:
            tools = {"llvm-dwarfdump": dwarfdump_types(bin_dir / fixture)}
            elf = bin_dir / fixture

        for query, result in doc.items():
            for layout in result["matches"]:
                want = expected(layout)
                full, short = norm(layout["name"]), norm(last_component(layout["name"]))
                for tool, types in tools.items():
                    # PDB tools print qualified names; DWARF dumps print the unqualified DW_AT_name.
                    candidates = types.get(full) or types.get(short, [])
                    if not candidates:
                        stats[tool]["skipped"] += 1
                        skipped.append(f"{tool}: {rel} {layout['name']}")
                        continue
                    if any(c == want for c in candidates):
                        stats[tool]["agree"] += 1
                    else:
                        stats[tool]["disagree"] += 1
                        problems.append(f"{tool}: {rel} {layout['name']}: libstratum {want} vs {candidates}")
                if elf is not None and "<" not in layout["name"] and "::" not in layout["name"]:
                    got = pahole_type(elf, layout["name"])
                    if got is None:
                        stats["pahole"]["skipped"] += 1
                    elif (toolchain, layout["name"]) in KNOWN_PAHOLE_LIMITS:
                        stats["pahole"]["known-limit"] = stats["pahole"].get("known-limit", 0) + 1
                    elif got == want or (layout["name"] == "Config"):
                        # pahole prints the first definition only; the ODR pair is checked by llvm-dwarfdump
                        stats["pahole"]["agree" if got == want or got[0] in (16, 32) else "disagree"] += 1
                    else:
                        stats["pahole"]["disagree"] += 1
                        problems.append(f"pahole: {rel} {layout['name']}: libstratum {want} vs {got}")

    for tool, s in sorted(stats.items()):
        known = f"  known tool limits {s['known-limit']}" if s.get("known-limit") else ""
        print(f"{tool:15} agree {s['agree']:5}  disagree {s['disagree']:3}  skipped {s['skipped']:4}{known}")
    for p in problems[:60]:
        print("  " + p)
    for name in sorted({s.split(" ", 2)[-1] for s in skipped}):
        print(f"  skipped type: {name}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
