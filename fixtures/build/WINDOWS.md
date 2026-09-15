# Windows fixture session (msvc, clang-cl)

One planned session on the Windows 11 install of the dual-boot machine (the Debian server is offline meanwhile).
Expected time: 1–2 hours, mostly installs.

## 1. Install (once)
- **Visual Studio Build Tools** (2022 or 2026). Workloads/components:
  - Desktop development with C++ (MSVC x64/x86 build tools, Windows SDK)
  - MSVC ARM64 build tools (matching the installed toolset version)
  - C++ Clang tools for Windows (provides `clang-cl` and `lld-link`)
  - Review the license terms yourself before accepting.
- **Python 3** (python.org installer; tick "Add python.exe to PATH")
- **Git for Windows**
- **rustup** (default MSVC host), for `cargo test`

## 2. Get the repository
Copy the repo onto the Windows drive: `git clone` from GitHub once a remote exists, or copy it via USB.
(Linux can read NTFS later, so outputs can also be picked up from the Windows drive after rebooting.)

## 3. Build fixtures
From **"x64 Native Tools Command Prompt"**, in the repo root:

```bat
python fixtures\build\build.py --toolchain msvc --arch x64
python fixtures\build\build.py --toolchain clang-cl --arch x64
```

From **"x64_arm64 Cross Tools Command Prompt"**:

```bat
python fixtures\build\build.py --toolchain msvc --arch arm64
python fixtures\build\build.py --toolchain clang-cl --arch arm64
```

Record versions for `TOOLCHAINS.md`:

```bat
cl 2>&1 | findstr Version > fixtures\build\windows-versions.txt
clang-cl --version >> fixtures\build\windows-versions.txt
lld-link --version >> fixtures\build\windows-versions.txt
```

## 4. Test the library on Windows

```bat
cargo test --workspace
```

## 5. Bring results back
Commit or copy `fixtures/bin/msvc`, `fixtures/bin/clang-cl`, `fixtures/build/windows-versions.txt`, and any build errors.

## If a build fails
Save the full console output. The driver is untested on Windows; path quoting (`/Fo`, `/Fd`) and
`clang-cl --target` for arm64 are the most likely problems.
