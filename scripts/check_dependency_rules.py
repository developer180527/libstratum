#!/usr/bin/env python3
"""Enforces the crate dependency rules from docs/02-architecture.md §2 and ADR-0018.

Run: python3 scripts/check_dependency_rules.py
"""
import json
import subprocess
import sys

PARSERS = {"object", "gimli", "ms-pdb", "pdb2", "pdb", "addr2line"}
PLUGIN_PREFIXES = ("libstratum-format-", "libstratum-debug-", "libstratum-lang-", "libstratum-demangle", "libstratum-arch")

meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"]))
deps = {p["name"]: {d["name"] for d in p["dependencies"] if d["kind"] in (None, "build")} for p in meta["packages"]}
internal = {n for n in deps if n.startswith("libstratum")}
errors = []

def is_plugin(name):
    return name.startswith(PLUGIN_PREFIXES)

for name, ds in deps.items():
    inner = ds & internal
    if name == "libstratum-model" and inner:
        errors.append(f"libstratum-model must not depend on internal crates: {sorted(inner)}")
    if name == "libstratum-core":
        if bad := {d for d in inner if is_plugin(d)}:
            errors.append(f"libstratum-core must not depend on plugins: {sorted(bad)}")
        if bad := ds & PARSERS:
            errors.append(f"libstratum-core must not depend on parser crates: {sorted(bad)}")
    if name == "libstratum-model" and (bad := ds & PARSERS):
        errors.append(f"libstratum-model must not depend on parser crates: {sorted(bad)}")
    if is_plugin(name) and name != "libstratum-arch":
        # Plugins may use the shared arch facts crate, never each other.
        if bad := {d for d in inner if is_plugin(d) and d != "libstratum-arch"}:
            errors.append(f"{name} must not depend on other plugins: {sorted(bad)}")

if errors:
    print("dependency rule violations:\n  " + "\n  ".join(errors))
    sys.exit(1)
print(f"dependency rules OK ({len(internal)} internal crates)")
