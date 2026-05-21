# pyauto-rs

`pyauto-rs` is a Rust automation editor/runner with a Windows 7 compatible native UI path.

The project currently has two UI targets:

- `pyauto-rs`: modern `egui` UI for newer Windows systems.
- `pyauto-rs-win7-native`: pure Win32 UI for Windows 7, no OpenGL, no WebView, single-file friendly.

## Win7 GUI Library Direction

This repository now includes the start of a reusable Win7-oriented Rust GUI layer:

```text
src/win7ui/
```

The goal is to grow this into a practical Rust GUI library for Windows 7 tools:

- pure Win32 controls
- UTF-16/Chinese text support
- no OpenGL dependency
- no browser/WebView dependency
- static CRT friendly
- easy single-file packaging
- automation-friendly widgets such as editor, log view, file dialogs, global hotkeys, and screen capture overlays

The extracted layer currently provides:

- wide string conversion
- HWND helpers
- button and label creation
- edit text read/replace/append helpers
- safe line insertion at the end of an editor
- native open/save file dialogs
- global hotkey registration helper
- `LogView` with clear, append, latest-output snapshot, and max-size protection
- UI-thread event sender with `PostMessageW` wakeups
- multiline editor/log controls and positioned buttons
- path literal conversion for scripts

Current module layout:

```text
src/win7ui/
  controls.rs
  dialogs.rs
  event.rs
  hotkey.rs
  layout.rs
  log_view.rs
  overlay.rs
  text.rs
  window.rs
  mod.rs
```

The current Win7 application still owns the higher-level automation workflow. Future steps should move these into `win7ui` modules:

```text
src/win7ui/
  app.rs
  menu.rs
  status_bar.rs
```

## Build

Modern UI:

```powershell
cargo build --release --bin pyauto-rs
```

Windows 7 native UI:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-win7.ps1
```

Output:

```text
target\x86_64-win7-windows-msvc\release\pyauto-rs-win7-native.exe
```

## Script Examples

```python
x = 1
print(f'hello {x}')

click(500, 300)
sleep(500)
find_click("captures/click_image.png", 0.92, 3000)

for i in range(10001):
    print(i)
```

Supported automation commands include:

```text
click x y
move x y
screenshot output.png
find image.png 0.92
find_click image.png 0.92 3000
sleep 500
type hello
```

Chinese command aliases are also supported:

```text
点击坐标 500 300
移动鼠标 500 300
截图 output.png
查找图片 button.png 0.92
查找图片并点击 button.png 0.92 3000
等待 500
输入文本 hello
```

## Win7 Native UI Features

- script editor
- run/stop buttons
- global F5 run and F11 stop
- log view with per-run clearing and latest-output retention
- open/save/save-as dialogs
- screen region capture
- image-click capture that inserts `find_click(...)`
- point capture that inserts `click(x, y)`
- semi-transparent full-screen capture overlay compatible with Win7

## Repository Status

This is an early implementation. The Win7 UI layer is intentionally small and conservative. The priority is compatibility and reliability first, then a friendlier API and optional UI descriptor files later.
