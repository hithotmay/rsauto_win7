# Windows 7 Notes

Rust's normal `x86_64-pc-windows-msvc` target uses modern Windows assumptions. For Windows 7, this project builds the native UI binary with Rust's Win7 target:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-win7.ps1
```

Output:

```text
target\x86_64-win7-windows-msvc\release\pyauto-rs-win7-native.exe
```

## Why a Native Win32 UI

The modern `egui/glow` UI can require OpenGL support that is unreliable on older Win7 machines. The Win7 binary therefore uses pure Win32 controls:

- no OpenGL
- no WebView
- no `winit/glutin`
- native edit controls, buttons, dialogs, hotkeys, and overlays

## Build Requirements

The build script installs/uses:

- nightly Rust
- `rust-src`
- target `x86_64-win7-windows-msvc`
- static CRT flags

The script also passes a compatibility library path for `windows_x86_64_msvc`.

## GUI Library Plan

The reusable GUI layer starts in:

```text
src/win7ui/
```

Current scope:

- HWND helpers
- UTF-16 text helpers
- controls
- edit/log helpers
- native file dialogs

Planned scope:

- global hotkey abstraction
- capture overlay abstraction
- log view with latest-line retention
- simple layouts: absolute, row, split, anchor
- optional `.win7ui.toml` UI description files
- eventual visual designer

The design target is practical Windows 7 desktop tools: small, fast, and packageable as a single executable.
