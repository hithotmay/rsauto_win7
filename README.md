# rsauto_win7

> Python办公助手 (PyAuto) 的 Rust 重写版本 —— 轻量级桌面自动化脚本编辑器/运行器，纯 Win32 原生 UI，兼容 Windows 7+。

## 项目简介

rsauto_win7 是 [Python办公助手](https://github.com/hithotmay/pyauto-rs) 的 Rust 重写，目标是提供一个**零依赖、单文件可执行、Win7 原生风格**的桌面自动化工具。用户可以在内置编辑器中编写自动化脚本，一键运行，支持鼠标键盘模拟、截图识别、图片查找等常见自动化操作。

### 技术栈

- **Rust** — 零成本抽象，无运行时
- **windows-sys 0.61** — 底层 Win32 API 绑定（raw FFI）
- **egui** — 现代风格 UI（开发中）
- **enigo** — 跨平台键鼠模拟
- **image** — 图像处理（截图、裁剪、模板匹配）
- **serde + toml** — DTT 声明式 UI 模板解析

### 双 UI 架构

| 目标 | 入口 | 状态 |
|------|------|------|
| **Win7 纯 Win32 原生** | `pyauto-rs-win7-native` | 主要开发线 |
| **egui 现代风格** | `pyauto-rs-egui` | 基础框架已有，暂未接入 |

---

## 架构设计

### DTT + BTT（声明式 UI 架构）

本项目独创了一套声明式 Win32 UI 架构：

- **DTT (Design-Time Template)** — `dtt.rs`：用 `.win7ui.toml` 文件声明 UI 结构（控件类型、布局、属性），纯数据模型，不涉及任何 HWND 或 Win32 API。
- **BTT (Build-Time Template)** — `btt.rs`：将 DTT 数据模型转换为实际 Win32 HWND 控件树，处理布局（Row/Column/Split/Weight）、权重弹性分配、TabControl 页面管理。

```
main.win7ui.toml (DTT) → Ui::from_toml() → BuiltTree (BTT) → HWND 控件树
```

### 代码模块

```
src/
├── bin/
│   ├── win7_native.rs      # Win32 原生主程序入口
│   └── main.win7ui.toml    # UI 声明式布局模板
├── win7ui/
│   ├── mod.rs              # 模块导出 & 公共工具函数
│   ├── dtt.rs              # DTT 数据模型（~670行）
│   ├── btt.rs              # BTT 渲染引擎（~700行）
│   ├── controls.rs         # Win32 控件封装（按钮/编辑框/下拉框等）
│   ├── rich_edit.rs        # RichEdit 封装（日志框基础）
│   ├── code_editor.rs      # 代码编辑器（语法高亮/行号/错误标记）
│   ├── log_view.rs         # 日志输出视图
│   ├── font.rs             # 字体管理
│   ├── layout.rs           # 窗口布局辅助
│   └── overlay.rs          # 截图覆盖层
├── core.rs                 # 脚本解释器（词法分析/执行引擎/内置函数）
├── lib.rs                  # 库入口
└── capture.rs              # 屏幕截图 & 图像搜索
```

---

## 脚本语言

内置轻量级 Python 风格脚本语言，支持：

- 变量赋值、算术/比较/逻辑表达式
- `if/elif/else`、`while`、`for..in`、`break/continue`
- 函数定义 `def`（含嵌套作用域）
- 字符串插值 `f"result={x}"`、列表操作
- 自动化命令：`click`, `type_text`, `screenshot`, `find_image`, `wait`, `move_to` 等

示例脚本：

```python
# 点击屏幕坐标
click(500, 300)

# 输入文字
type_text("Hello World")

# 截图保存
screenshot("capture.png")

# 图片查找 + 点击
pos = find_image("button.png")
if pos:
    click(pos[0], pos[1])
```

---

## 完成进度

### 已完成 ✅

| 模块 | 功能 | 状态 |
|------|------|------|
| **DTT 数据模型** | TOML 解析 → Node 树，支持所有控件类型 | ✅ 完成 |
| **BTT 渲染引擎** | 自动构建 HWND 控件树，布局引擎 | ✅ 完成 |
| **布局系统** | Row/Column/Split/Weight 弹性布局 | ✅ 完成 |
| **TabControl** | 多页面切换，自动显隐子控件 | ✅ 完成 |
| **代码编辑器** | RichEdit 封装，行号栏，当前行高亮 | ✅ 完成 |
| **语法高亮** | 关键字/字符串/注释/数字/f-string 着色 | ✅ 完成 |
| **错误行标记** | 脚本运行出错时高亮对应行 | ✅ 完成 |
| **日志输出** | RichEdit 日志框，尾部追加，自动滚动 | ✅ 完成 |
| **脚本解释器** | 完整的 Python 风格脚本执行引擎 | ✅ 完成 |
| **鼠标模拟** | click/move_to/drag/scroll | ✅ 完成 |
| **键盘模拟** | type_text/hotkey | ✅ 完成 |
| **截图功能** | 全屏/区域截图，覆盖层框选 | ✅ 完成 |
| **图片搜索** | 模板匹配，支持缩放和阈值 | ✅ 完成 |
| **文件操作** | 新建/打开/保存脚本，另存为 | ✅ 完成 |
| **快捷键** | F10 运行，F11 停止 | ✅ 完成 |
| **扁平化 UI** | 去除 3D 效果，统一配色，SetWindowTheme | ✅ 完成 |
| **进度条扁平** | PBS_SMOOTH + 自定义颜色 | ✅ 完成 |
| **状态栏** | 运行状态提示 | ✅ 完成 |
| **GroupBox** | BS_FLAT 扁平分组框 | ✅ 完成 |
| **TDD+BDD** | 单元测试覆盖核心逻辑 | ✅ 完成 |

### 进行中 🚧

| 模块 | 功能 | 状态 | 说明 |
|------|------|------|------|
| **扁平化细节** | 控件间间距/对齐微调 | 🚧 80% | 编辑器和日志框间距已加 |
| **RichEdit 渲染** | 日志框内容需框选才可见的 bug | 🚧 调查中 | 可能是 WM_CTLCOLOREDIT 与 RichEdit 冲突 |

### 未接入功能 📋

以下控件已在 DTT/BTT 中实现创建，但主程序尚未接入交互逻辑：

| 控件 | ID | 说明 |
|------|----|------|
| **ComboBox** | 130 | 脚本语言选择下拉框 |
| **搜索框** | 131 | 编辑器内搜索功能 |
| **ProgressBar** | 132 | 脚本执行进度（颜色已设，逻辑未接） |
| **自动换行** | 133 | Checkbox，编辑器自动换行切换 |
| **行号显示** | 134 | Checkbox，行号栏开关 |
| **多行编辑** | 137 | MultilineEdit，代码片段编辑 |
| **代码片段** | 138 | ListBox，常用脚本片段管理 |
| **变量监视** | 140 | ListBox，运行时变量查看 |

---

## 未来计划

### 短期（v0.2）

- [ ] **搜索替换** — 编辑器内 Ctrl+F 搜索、Ctrl+H 替换
- [ ] **代码片段管理** — 保存/加载常用脚本片段
- [ ] **变量监视窗口** — 运行时实时查看变量值
- [ ] **脚本语言切换** — ComboBox 切换不同脚本引擎
- [ ] **进度条接入** — 长脚本执行进度反馈
- [ ] **行号/自动换行开关** — Checkbox 控制编辑器行为
- [ ] **RichEdit 渲染修复** — 日志框内容无需框选即可显示
- [ ] **右键菜单** — 编辑器右键菜单（复制/粘贴/全选）

### 中期（v0.3）

- [ ] **代码补全** — 自动弹出命令补全列表
- [ ] **脚本调试** — 单步执行、断点、调用栈
- [ ] **多标签编辑** — 同时打开多个脚本文件
- [ ] **脚本导入** — `import` 语句支持加载外部脚本
- [ ] **错误恢复** — 脚本出错后可从断点继续
- [ ] **配置持久化** — 保存窗口大小/位置/最近文件

### 长期（v1.0）

- [ ] **egui 现代风格 UI** — 完成 egui 版本的完整功能
- [ ] **插件系统** — 允许用户扩展自定义命令
- [ ] **录制回放** — 录制用户操作自动生成脚本
- [ ] **多语言脚本引擎** — 支持 JavaScript/Lua 等脚本语言
- [ ] **网络操作** — HTTP 请求、WebSocket 等网络命令
- [ ] **OCR 识别** — 屏幕文字识别
- [ ] **打包分发** — 单文件 exe，无需安装

---

## 构建与运行

```bash
# 构建 Win32 原生版本
cargo build --bin pyauto-rs-win7-native --release

# 运行
cargo run --bin pyauto-rs-win7-native

# 运行测试
cargo test
```

### 依赖

- Rust 1.75+
- Windows SDK（通过 windows-sys crate 自动链接）
- 无需额外安装

---

## 项目结构关系

```
Python办公助手 (原始 Python 版本)
  └── pyauto-rs (Rust 重写)
        ├── rsauto_win7     ← 本项目（Win32 原生 UI）
        └── pyauto-rs-egui  （egui 现代风格 UI，开发中）
```

---

## License

MIT
