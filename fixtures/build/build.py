#!/usr/bin/env python3
"""Builds the fixture corpus (docs/06-testing.md).

Outputs go to fixtures/bin/<toolchain>/<arch>/<opt>/<fixture>/ and are committed.
Every output directory gets a build.json recording the exact commands and tool versions.

    python3 fixtures/build/build.py --list
    python3 fixtures/build/build.py --toolchain apple-clang
    python3 fixtures/build/build.py --toolchain msvc --fixture padding --arch x64 --dry-run

Toolchains run where their tools exist: apple-clang on macOS, linux-*/embedded in the
Docker image (Dockerfile.linux), msvc/clang-cl on Windows from a developer prompt.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

FIXTURES = Path(__file__).resolve().parent.parent
SRC = FIXTURES / "src"
BIN = FIXTURES / "bin"
EMBEDDED = FIXTURES / "build" / "embedded"

OPT_FLAGS = {"O0": "-O0", "O2": "-O2"}
MSVC_OPT_FLAGS = {"O0": "/Od", "O2": "/O2"}
CXX_STD = "-std=c++17"
C_STD = "-std=c11"


def is_cpp(src: str) -> bool:
    return src.endswith((".cpp", ".cc", ".cxx"))


@dataclass
class Toolchain:
    name: str
    archs: list[str]
    host: str  # "darwin", "linux", "windows"
    version_cmds: list[list[str]] = field(default_factory=list)

    def compile(self, src: str, obj: Path, arch: str, opt: str) -> list[str]:
        raise NotImplementedError

    def obj_suffix(self) -> str:
        return ".o"

    def link(self, objs: list[Path], out: Path, arch: str, opt: str, cpp: bool) -> list[list[str]]:
        raise NotImplementedError

    def binary_name(self, fixture: str) -> str:
        return fixture

    def link_cwd(self, out_dir: Path) -> Path:
        """Working directory for link steps (compile steps always run in fixtures/src)."""
        return SRC

    def env(self) -> dict[str, str]:
        return {}


# ---------------------------------------------------------------------------- Apple


def macos_sdk() -> str:
    """The Xcode SDK, not the Command Line Tools one (docs/04 §6)."""
    if sdk := os.environ.get("STRATUM_MACOS_SDK"):
        return sdk
    xcode = Path("/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk")
    if xcode.exists():
        return str(xcode)
    return subprocess.check_output(["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()


class AppleClang(Toolchain):
    def __init__(self) -> None:
        super().__init__("apple-clang", ["arm64", "x86_64"], "darwin", [["clang", "--version"], ["ld", "-v"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("clang++", CXX_STD) if is_cpp(src) else ("clang", C_STD)
        return [driver, "-arch", arch, "-isysroot", macos_sdk(), std, "-g", OPT_FLAGS[opt],
                "-ffile-compilation-dir=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        # One output serves both debug-info paths: the binary's debug map points at the
        # kept .o files, and dsymutil produces a dSYM next to it (docs/04 §3).
        # Runs inside the output directory with relative names, so the map file, OSO entries and
        # dSYM relocation records carry no machine-specific absolute paths.
        driver = "clang++" if cpp else "clang"
        name = out.name
        return [
            [driver, "-arch", arch, "-isysroot", macos_sdk(), *(o.name for o in objs), "-o", name,
             f"-Wl,-map,{name}.map", "-Wl,-oso_prefix,."],
            ["dsymutil", name, "-o", f"{name}.dSYM"],
        ]

    def link_cwd(self, out_dir: Path) -> Path:
        return out_dir

    def env(self):
        return {"ZERO_AR_DATE": "1"}


# ---------------------------------------------------------------------------- Linux (hosted)


class LinuxClang(Toolchain):
    TRIPLES = {"x86_64": "x86_64-linux-gnu", "aarch64": "aarch64-linux-gnu"}

    def __init__(self) -> None:
        super().__init__("linux-clang", ["x86_64", "aarch64"], "linux", [["clang", "--version"], ["ld.lld", "--version"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("clang++", CXX_STD) if is_cpp(src) else ("clang", C_STD)
        return [driver, f"--target={self.TRIPLES[arch]}", std, "-g", OPT_FLAGS[opt], "-ffunction-sections",
                "-fdata-sections", "-ffile-compilation-dir=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        driver = "clang++" if cpp else "clang"
        return [[driver, f"--target={self.TRIPLES[arch]}", "-fuse-ld=lld", *map(str, objs), "-o", str(out),
                 "-Wl,--build-id", f"-Wl,-Map,{out}.map"]]


class LinuxClangTypeUnits(LinuxClang):
    """Type definitions in type units (`-fdebug-types-section`): DWARF 5 in `.debug_info`,
    DWARF 4 in `.debug_types`. Members reference types by signature (DW_FORM_ref_sig8)."""

    def __init__(self, dwarf_version: int) -> None:
        Toolchain.__init__(self, f"linux-clang-types{dwarf_version}", ["x86_64"], "linux",
                           [["clang", "--version"], ["ld.lld", "--version"]])
        self.dwarf_version = dwarf_version

    def compile(self, src, obj, arch, opt):
        cmd = super().compile(src, obj, arch, opt)
        at = cmd.index("-g")
        cmd[at:at + 1] = ["-g", f"-gdwarf-{self.dwarf_version}", "-fdebug-types-section"]
        return cmd


class LinuxGcc(Toolchain):
    PREFIX = {"x86_64": "x86_64-linux-gnu-", "aarch64": "aarch64-linux-gnu-"}

    def __init__(self) -> None:
        super().__init__("linux-gcc", ["x86_64", "aarch64"], "linux",
                         [["x86_64-linux-gnu-gcc", "--version"], ["aarch64-linux-gnu-gcc", "--version"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("g++", CXX_STD) if is_cpp(src) else ("gcc", C_STD)
        return [self.PREFIX[arch] + driver, std, "-g", OPT_FLAGS[opt], "-ffunction-sections", "-fdata-sections",
                "-ffile-prefix-map=" + str(SRC) + "=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        driver = "g++" if cpp else "gcc"
        return [[self.PREFIX[arch] + driver, *map(str, objs), "-o", str(out), "-Wl,--build-id",
                 f"-Wl,-Map,{out}.map"]]


# ---------------------------------------------------------------------------- Embedded

EMBEDDED_CFLAGS = ["-ffreestanding", "-fno-exceptions", "-ffunction-sections", "-fdata-sections"]
EMBEDDED_CXXFLAGS = ["-fno-rtti", "-fno-threadsafe-statics"]


class ArmNoneEabiGcc(Toolchain):
    CPU = {
        "thumbv6m": ["-mcpu=cortex-m0plus", "-mthumb", "-mfloat-abi=soft"],
        "thumbv7em": ["-mcpu=cortex-m4", "-mthumb", "-mfloat-abi=hard", "-mfpu=fpv4-sp-d16"],
        "thumbv8m": ["-mcpu=cortex-m33", "-mthumb", "-mfloat-abi=soft"],
    }

    def __init__(self) -> None:
        super().__init__("arm-none-eabi-gcc", list(self.CPU), "linux", [["arm-none-eabi-gcc", "--version"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("arm-none-eabi-g++", CXX_STD) if is_cpp(src) else ("arm-none-eabi-gcc", C_STD)
        extra = EMBEDDED_CXXFLAGS if is_cpp(src) else []
        return [driver, *self.CPU[arch], std, "-g", OPT_FLAGS[opt], *EMBEDDED_CFLAGS, *extra,
                "-ffile-prefix-map=" + str(SRC) + "=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        return [["arm-none-eabi-g++", *self.CPU[arch], "-nostdlib", "-T", str(EMBEDDED / "cortex-m.ld"),
                 *map(str, objs), "-o", str(out), "-Wl,--gc-sections", "-Wl,--build-id", f"-Wl,-Map,{out}.map",
                 "-lgcc"]]

    def extra_sources(self) -> list[str]:
        return ["../build/embedded/support.cpp", "../build/embedded/vectors.c"]


# Clang treats `main` as an ordinary (mangled) C++ function under -ffreestanding, unlike GCC,
# so LLVM embedded builds use -fno-builtin instead (docs/spikes/m0-report.md S9).
LLVM_EMBEDDED_CFLAGS = ["-fno-builtin", "-fno-exceptions", "-ffunction-sections", "-fdata-sections"]


class ArmLlvm(Toolchain):
    TRIPLE = {
        "thumbv6m": ["--target=thumbv6m-none-eabi", "-mcpu=cortex-m0plus", "-mfloat-abi=soft"],
        "thumbv7em": ["--target=thumbv7em-none-eabihf", "-mcpu=cortex-m4", "-mfpu=fpv4-sp-d16"],
        "thumbv8m": ["--target=thumbv8m.main-none-eabi", "-mcpu=cortex-m33", "-mfloat-abi=soft"],
    }

    def __init__(self) -> None:
        super().__init__("arm-llvm", list(self.TRIPLE), "linux", [["clang", "--version"], ["ld.lld", "--version"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("clang++", CXX_STD) if is_cpp(src) else ("clang", C_STD)
        extra = EMBEDDED_CXXFLAGS if is_cpp(src) else []
        return [driver, *self.TRIPLE[arch], std, "-g", OPT_FLAGS[opt], *LLVM_EMBEDDED_CFLAGS, *extra,
                "-ffile-compilation-dir=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        # compiler-rt builtins for bare-metal Arm aren't packaged in the image; GCC's libgcc for the
        # same CPU provides the EABI helpers (e.g. __aeabi_d2iz on soft-float cores).
        libgcc = subprocess.check_output(
            ["arm-none-eabi-gcc", *ArmNoneEabiGcc.CPU[arch], "-print-libgcc-file-name"], text=True).strip()
        return [["ld.lld", "-T", str(EMBEDDED / "cortex-m.ld"), *map(str, objs), libgcc, "-o", str(out),
                 "--gc-sections", "--build-id", f"-Map={out}.map"]]

    def extra_sources(self) -> list[str]:
        return ["../build/embedded/support.cpp", "../build/embedded/vectors.c"]


class RiscvGcc(Toolchain):
    def __init__(self) -> None:
        super().__init__("riscv-gcc", ["rv32imac"], "linux", [["riscv64-unknown-elf-gcc", "--version"]])

    def compile(self, src, obj, arch, opt):
        driver, std = ("riscv64-unknown-elf-g++", CXX_STD) if is_cpp(src) else ("riscv64-unknown-elf-gcc", C_STD)
        extra = EMBEDDED_CXXFLAGS if is_cpp(src) else []
        return [driver, "-march=rv32imac", "-mabi=ilp32", std, "-g", OPT_FLAGS[opt], *EMBEDDED_CFLAGS, *extra,
                "-ffile-prefix-map=" + str(SRC) + "=.", "-c", src, "-o", str(obj)]

    def link(self, objs, out, arch, opt, cpp):
        return [["riscv64-unknown-elf-g++", "-march=rv32imac", "-mabi=ilp32", "-nostdlib", "-T",
                 str(EMBEDDED / "riscv.ld"), *map(str, objs), "-o", str(out), "-Wl,--gc-sections",
                 "-Wl,--build-id", f"-Wl,-Map,{out}.map", "-lgcc"]]

    def extra_sources(self) -> list[str]:
        return ["../build/embedded/support.cpp", "../build/embedded/vectors.c"]


# ---------------------------------------------------------------------------- Windows


class Msvc(Toolchain):
    """Run from a Developer Command Prompt for the target arch (vcvarsall x64 / arm64)."""

    def __init__(self) -> None:
        super().__init__("msvc", ["x64", "arm64"], "windows", [["cl"], ["link"]])

    def obj_suffix(self):
        return ".obj"

    def binary_name(self, fixture):
        return fixture + ".exe"

    def compile(self, src, obj, arch, opt):
        std = ["/std:c++17", "/TP"] if is_cpp(src) else ["/std:c11", "/TC"]
        # /MD links the CRT dynamically so PDBs and maps describe the fixture, not the whole static CRT.
        return ["cl", "/nologo", *std, "/Zi", MSVC_OPT_FLAGS[opt], "/MD", "/Gy", "/GR-", "/EHs-c-",
                f"/Fd{obj.with_suffix('.compile.pdb')}", "/c", src, f"/Fo{obj}"]

    def link(self, objs, out, arch, opt, cpp):
        opt_link = ["/OPT:REF", "/OPT:ICF"] if opt == "O2" else []
        return [["link", "/nologo", "/DEBUG:FULL", "/INCREMENTAL:NO", *opt_link, f"/MAP:{out.with_suffix('.map')}",
                 f"/PDB:{out.with_suffix('.pdb')}", f"/OUT:{out}", *map(str, objs)]]


class ClangCl(Msvc):
    def __init__(self) -> None:
        Toolchain.__init__(self, "clang-cl", ["x64", "arm64"], "windows", [["clang-cl", "--version"], ["lld-link", "--version"]])

    TARGET = {"x64": "x86_64-pc-windows-msvc", "arm64": "aarch64-pc-windows-msvc"}

    def compile(self, src, obj, arch, opt):
        std = ["/std:c++17", "/TP"] if is_cpp(src) else ["/TC"]
        return ["clang-cl", "/nologo", f"--target={self.TARGET[arch]}", *std, "/Z7", MSVC_OPT_FLAGS[opt], "/MD", "/Gy",
                "/GR-", "/c", src, f"/Fo{obj}"]

    def link(self, objs, out, arch, opt, cpp):
        opt_link = ["/OPT:REF", "/OPT:ICF"] if opt == "O2" else []
        return [["lld-link", "/nologo", "/DEBUG:FULL", "/INCREMENTAL:NO", *opt_link, f"/MAP:{out.with_suffix('.map')}",
                 f"/PDB:{out.with_suffix('.pdb')}", f"/OUT:{out}", *map(str, objs)]]


TOOLCHAINS: dict[str, Toolchain] = {t.name: t for t in [
    AppleClang(), LinuxClang(), LinuxClangTypeUnits(5), LinuxClangTypeUnits(4), LinuxGcc(), ArmNoneEabiGcc(),
    ArmLlvm(), RiscvGcc(), Msvc(), ClangCl(),
]}


# ---------------------------------------------------------------------------- driver


def run(cmd: list[str], env: dict[str, str], log: list[dict], dry_run: bool, cwd: Path = SRC) -> None:
    log.append({"cmd": cmd, "cwd": cwd})
    print("  $", " ".join(cmd))
    if dry_run:
        return
    result = subprocess.run(cmd, cwd=cwd, env={**os.environ, **env}, capture_output=True, text=True)
    output = result.stdout + result.stderr
    # dsymutil exits 0 even when it can't read object files, silently producing an empty dSYM.
    if result.returncode != 0 or "unable to open object file" in output or "no debug symbols" in output:
        sys.stderr.write(output)
        raise SystemExit(f"command failed ({result.returncode}): {' '.join(cmd)}")


def portable(arg: str) -> str:
    """Records paths relative to the repo with forward slashes, independent of host and checkout."""
    arg = arg.replace(str(FIXTURES) + os.sep, "fixtures/").replace(str(FIXTURES), "fixtures")
    return arg.replace("\\", "/") if "fixtures" in arg else arg


def relative(path: Path) -> str:
    return "fixtures/" + path.relative_to(FIXTURES).as_posix()


def tool_versions(tc: Toolchain) -> dict[str, str]:
    versions = {}
    for cmd in tc.version_cmds:
        try:
            out = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
            text = (out.stderr + "\n" + out.stdout).strip().splitlines()
            versions[" ".join(cmd)] = next((line for line in text if "ersion" in line), text[0])
        except (OSError, subprocess.SubprocessError, IndexError):
            versions[" ".join(cmd)] = "unavailable"
    return versions


def build(tc: Toolchain, fixture: dict, arch: str, opt: str, dry_run: bool) -> None:
    out_dir = BIN / tc.name / arch / opt / fixture["name"]
    print(f"[{tc.name} {arch} {opt}] {fixture['name']}")
    if not dry_run:
        shutil.rmtree(out_dir, ignore_errors=True)
        out_dir.mkdir(parents=True)

    sources = list(fixture["sources"])
    if hasattr(tc, "extra_sources"):
        sources += tc.extra_sources()
    log: list[dict] = []
    objs = []
    for src in sources:
        obj = out_dir / (Path(src).stem + tc.obj_suffix())
        run(tc.compile(src, obj, arch, opt), tc.env(), log, dry_run)
        objs.append(obj)
    cpp = any(is_cpp(s) for s in sources)
    binary = out_dir / tc.binary_name(fixture["name"])
    for cmd in tc.link(objs, binary, arch, opt, cpp):
        run(cmd, tc.env(), log, dry_run, tc.link_cwd(out_dir))

    if not dry_run:
        record = {
            "fixture": fixture["name"],
            "toolchain": tc.name,
            "arch": arch,
            "opt": opt,
            "host": f"{platform.system()} {platform.machine()}",
            "tools": tool_versions(tc),
            # Paths are made relative so records don't depend on the checkout location.
            "commands": [
                {"cwd": relative(e["cwd"]), "argv": [portable(a) for a in e["cmd"]]}
                for e in log
            ],
        }
        (out_dir / "build.json").write_text(json.dumps(record, indent=2) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--toolchain", choices=sorted(TOOLCHAINS))
    parser.add_argument("--fixture", action="append", help="limit to these fixtures")
    parser.add_argument("--arch", action="append", help="limit to these arches")
    parser.add_argument("--opt", action="append", choices=sorted(OPT_FLAGS), help="limit to these opt levels")
    parser.add_argument("--dry-run", action="store_true", help="print commands without running them")
    parser.add_argument("--list", action="store_true", help="list toolchains and fixtures")
    args = parser.parse_args()

    manifest = tomllib.loads((FIXTURES / "fixtures.toml").read_text())
    if args.list or not args.toolchain:
        for name, tc in TOOLCHAINS.items():
            print(f"{name:20} host={tc.host:8} archs={','.join(tc.archs)}")
        print("fixtures:", ", ".join(f["name"] for f in manifest["fixtures"]))
        return

    tc = TOOLCHAINS[args.toolchain]
    for fixture in manifest["fixtures"]:
        group = manifest["groups"][fixture["group"]]
        if tc.name not in group["toolchains"] or (args.fixture and fixture["name"] not in args.fixture):
            continue
        for arch in tc.archs:
            if args.arch and arch not in args.arch:
                continue
            for opt in group["opts"]:
                if args.opt and opt not in args.opt:
                    continue
                build(tc, fixture, arch, opt, args.dry_run)


if __name__ == "__main__":
    main()
