//! DTT — Design-Time Template
//!
//! 声明式 UI 描述层。用 Rust 结构体（可从 TOML 反序列化）描述窗口和控件树，
//! 完全不涉及任何 Win32 API 调用，纯数据模型。
//!
//! 设计原则：
//! - 零 Win32 依赖：本模块不 import 任何 windows-sys 类型
//! - 可序列化：所有结构体 derive Serialize/Deserialize，可从 .win7ui.toml 加载
//! - 树形结构：UiTree -> Window -> children: Vec<Node>
//! - 布局声明：每个节点可声明 margin/padding/align/weight 等布局属性
//! - 事件绑定声明：通过 event_id 关联 WM_COMMAND 通知码

use serde::{Deserialize, Serialize};

// ─── 错误类型 ───────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum DttError {
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

// ─── 顶级容器 ───────────────────────────────────────────────

/// 一个 .win7ui.toml 文件的完整描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiTree {
    pub window: WindowDecl,
    #[serde(default)]
    pub fonts: FontDecl,
}

// ─── 窗口 ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowDecl {
    pub title: String,
    #[serde(default = "default_width")]
    pub width: i32,
    #[serde(default = "default_height")]
    pub height: i32,
    /// 窗口类名，默认 "Win7UiClass"
    #[serde(default = "default_class")]
    pub class: String,
    /// 根节点下的直接子控件
    #[serde(default)]
    pub children: Vec<Node>,
    /// 全局热键
    #[serde(default)]
    pub hotkeys: Vec<HotKeyDecl>,
}

fn default_width() -> i32 { 800 }
fn default_height() -> i32 { 600 }
fn default_class() -> String { "Win7UiClass".into() }

// ─── 字体 ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontDecl {
    #[serde(default = "default_ui_font")]
    pub ui: FontSpec,
    #[serde(default = "default_fixed_font")]
    pub fixed: FontSpec,
}

impl Default for FontDecl {
    fn default() -> Self {
        Self {
            ui: default_ui_font(),
            fixed: default_fixed_font(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontSpec {
    pub face: String,
    pub size: i32,
}

fn default_ui_font() -> FontSpec {
    FontSpec { face: "Microsoft YaHei".into(), size: 16 }
}
fn default_fixed_font() -> FontSpec {
    FontSpec { face: "NSimSun".into(), size: 16 }
}

// ─── 热键 ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotKeyDecl {
    pub id: i32,
    pub key: String,
    #[serde(default)]
    pub modifiers: Vec<String>,
}

// ─── 控件节点 ───────────────────────────────────────────────

/// UI 树中的一个节点：一个 Win32 控件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// 控件类型
    #[serde(rename = "type")]
    pub kind: NodeKind,
    /// 控件 ID（用于 WM_COMMAND 通知），可选
    pub id: Option<i32>,
    /// 显示文本 / 标签
    #[serde(default)]
    pub text: String,
    /// 布局属性
    #[serde(default)]
    pub layout: LayoutDecl,
    /// 类型特定属性
    #[serde(default)]
    pub props: Props,
    /// 子控件（如 Tab 的子页面）
    #[serde(default)]
    pub children: Vec<Node>,
}

// ─── 控件类型 ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    // 基础
    Button,
    Label,
    Edit,
    MultilineEdit,
    Checkbox,
    ComboBox,
    ListBox,
    ProgressBar,
    TabControl,
    // 复合
    CodeEditor,
    LogView,
    // 容器
    Group,
    Row,
    Column,
    Split,
    // 自绘
    Canvas,
}

// ─── 布局声明 ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutDecl {
    /// 绝对定位 (x, y)
    #[serde(default)]
    pub pos: Option<(i32, i32)>,
    /// 显式尺寸 (width, height)
    #[serde(default)]
    pub size: Option<(i32, i32)>,
    /// 弹性权重，0=固定，>0=按比例分配剩余空间
    #[serde(default)]
    pub weight: i32,
    /// 外边距 [top, right, bottom, left]
    #[serde(default)]
    pub margin: Vec<i32>,
    /// 对齐方式: start / center / end / stretch
    #[serde(default = "default_align")]
    pub align: String,
    /// 最小尺寸 (w, h)
    #[serde(default)]
    pub min_size: Option<(i32, i32)>,
    /// 最大尺寸 (w, h)
    #[serde(default)]
    pub max_size: Option<(i32, i32)>,
    /// split 模式下的固定侧宽度
    #[serde(default)]
    pub fixed_width: Option<i32>,
}

impl Default for LayoutDecl {
    fn default() -> Self {
        Self {
            pos: None,
            size: None,
            weight: 0,
            margin: vec![],
            align: default_align(),
            min_size: None,
            max_size: None,
            fixed_width: None,
        }
    }
}

fn default_align() -> String { "stretch".into() }

// ─── 控件属性 ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Props {
    // Button / Checkbox
    #[serde(default)]
    pub checked: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub default_button: bool,

    // ComboBox / ListBox
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub selected: i32,

    // Edit / MultilineEdit
    #[serde(default)]
    pub readonly: bool,
    #[serde(default)]
    pub hscroll: bool,
    #[serde(default)]
    pub placeholder: String,

    // ProgressBar
    #[serde(default)]
    pub min: i32,
    #[serde(default = "default_pb_max")]
    pub max: i32,
    #[serde(default)]
    pub value: i32,

    // Split
    #[serde(default)]
    pub split_side: Option<String>,  // "left" | "right"
    #[serde(default = "default_split_right_width")]
    pub split_right_width: i32,

    // TabControl
    #[serde(default)]
    pub tabs: Vec<String>,

    // CodeEditor
    #[serde(default)]
    pub gutter_width: Option<i32>,

    // LogView
    #[serde(default = "default_log_max_chars")]
    pub log_max_chars: i32,

    // 通用
    #[serde(default)]
    pub font: Option<String>, // "ui" | "fixed"
}

fn default_pb_max() -> i32 { 100 }
fn default_split_right_width() -> i32 { 300 }
fn default_log_max_chars() -> i32 { 50000 }

impl Default for Props {
    fn default() -> Self {
        Self {
            checked: false,
            disabled: false,
            default_button: false,
            items: vec![],
            selected: 0,
            readonly: false,
            hscroll: false,
            placeholder: String::new(),
            min: 0,
            max: default_pb_max(),
            value: 0,
            split_side: None,
            split_right_width: default_split_right_width(),
            tabs: vec![],
            gutter_width: None,
            log_max_chars: default_log_max_chars(),
            font: None,
        }
    }
}

// ─── 解析入口 ───────────────────────────────────────────────

impl UiTree {
    /// 从 TOML 文本解析
    pub fn from_toml(input: &str) -> Result<Self, DttError> {
        let tree: UiTree = toml::from_str(input)?;
        tree.validate()?;
        Ok(tree)
    }

    /// 从 .win7ui.toml 文件加载
    pub fn load_file(path: &std::path::Path) -> Result<Self, DttError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| DttError::Validation(format!("cannot read {}: {e}", path.display())))?;
        Self::from_toml(&text)
    }

    /// 验证树结构完整性
    pub fn validate(&self) -> Result<(), DttError> {
        if self.window.title.is_empty() {
            return Err(DttError::Validation("window.title is required".into()));
        }
        for (i, child) in self.window.children.iter().enumerate() {
            child.validate(format!("window.children[{i}]"))?;
        }
        Ok(())
    }
}

impl Node {
    fn validate(&self, path: String) -> Result<(), DttError> {
        // 容器类型必须有 children
        match &self.kind {
            NodeKind::Row | NodeKind::Column | NodeKind::Group => {
                if self.children.is_empty() {
                    return Err(DttError::Validation(
                        format!("{path}: {:?} must have at least one child", self.kind),
                    ));
                }
            }
            NodeKind::Split => {
                if self.children.len() != 2 {
                    return Err(DttError::Validation(
                        format!("{path}: Split must have exactly 2 children"),
                    ));
                }
            }
            _ => {}
        }
        for (i, child) in self.children.iter().enumerate() {
            child.validate(format!("{path}.children[{i}]"))?;
        }
        Ok(())
    }
}

// ─── 查询工具 ───────────────────────────────────────────────

impl Node {
    /// 按深度优先搜索找到第一个匹配 kind 的节点
    pub fn find(&self, kind: NodeKind) -> Option<&Node> {
        if self.kind == kind {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find(kind.clone()) {
                return Some(found);
            }
        }
        None
    }

    /// 按 ID 查找节点
    pub fn find_by_id(&self, id: i32) -> Option<&Node> {
        if self.id == Some(id) {
            return Some(self);
        }
        for child in &self.children {
            if let Some(found) = child.find_by_id(id) {
                return Some(found);
            }
        }
        None
    }

    /// 收集所有指定类型的节点
    pub fn collect_kind<'a>(&'a self, kind: NodeKind, out: &mut Vec<&'a Node>) {
        if self.kind == kind {
            out.push(self);
        }
        for child in &self.children {
            child.collect_kind(kind.clone(), out);
        }
    }
}

// ─── TOML 序列化（用于生成模板） ────────────────────────────

impl UiTree {
    /// 生成示例 .win7ui.toml 模板
    pub fn example_toml() -> String {
        let example = UiTree {
            window: WindowDecl {
                title: "My App".into(),
                width: 900,
                height: 650,
                class: "MyAppClass".into(),
                children: vec![
                    Node {
                        kind: NodeKind::Row,
                        id: None,
                        text: String::new(),
                        layout: LayoutDecl {
                            size: Some((0, 36)),
                            ..Default::default()
                        },
                        props: Props::default(),
                        children: vec![
                            Node {
                                kind: NodeKind::Button,
                                id: Some(1001),
                                text: "Run".into(),
                                layout: LayoutDecl { size: Some((80, 28)), ..Default::default() },
                                props: Props { default_button: true, ..Default::default() },
                                children: vec![],
                            },
                            Node {
                                kind: NodeKind::Button,
                                id: Some(1002),
                                text: "Stop".into(),
                                layout: LayoutDecl { size: Some((80, 28)), ..Default::default() },
                                props: Props::default(),
                                children: vec![],
                            },
                            Node {
                                kind: NodeKind::Label,
                                id: None,
                                text: "Status: Ready".into(),
                                layout: LayoutDecl { weight: 1, align: "center".into(), ..Default::default() },
                                props: Props::default(),
                                children: vec![],
                            },
                        ],
                    },
                    Node {
                        kind: NodeKind::Split,
                        id: None,
                        text: String::new(),
                        layout: LayoutDecl { weight: 1, ..Default::default() },
                        props: Props { split_right_width: 250, ..Default::default() },
                        children: vec![
                            Node {
                                kind: NodeKind::CodeEditor,
                                id: Some(2001),
                                text: String::new(),
                                layout: LayoutDecl { weight: 1, ..Default::default() },
                                props: Props { gutter_width: Some(48), ..Default::default() },
                                children: vec![],
                            },
                            Node {
                                kind: NodeKind::LogView,
                                id: Some(2002),
                                text: String::new(),
                                layout: LayoutDecl { weight: 1, ..Default::default() },
                                props: Props { log_max_chars: 50000, ..Default::default() },
                                children: vec![],
                            },
                        ],
                    },
                ],
                hotkeys: vec![
                    HotKeyDecl { id: 1, key: "F5".into(), modifiers: vec![] },
                    HotKeyDecl { id: 2, key: "F11".into(), modifiers: vec![] },
                ],
            },
            fonts: FontDecl::default(),
        };
        toml::to_string_pretty(&example).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_toml() {
        let toml = r#"
[window]
title = "Test"
width = 400
height = 300
"#;
        let tree = UiTree::from_toml(toml).unwrap();
        assert_eq!(tree.window.title, "Test");
        assert_eq!(tree.window.children.len(), 0);
    }

    #[test]
    fn parse_full_example() {
        let toml = UiTree::example_toml();
        let tree = UiTree::from_toml(&toml).unwrap();
        assert_eq!(tree.window.children.len(), 2);
        // Row with 3 children
        let row = &tree.window.children[0];
        assert_eq!(row.kind, NodeKind::Row);
        assert_eq!(row.children.len(), 3);
        // Split with 2 children
        let split = &tree.window.children[1];
        assert_eq!(split.kind, NodeKind::Split);
        assert_eq!(split.children.len(), 2);
    }

    #[test]
    fn validate_split_needs_2_children() {
        let tree = UiTree {
            window: WindowDecl {
                title: "T".into(),
                width: 100,
                height: 100,
                class: "C".into(),
                children: vec![Node {
                    kind: NodeKind::Split,
                    id: None,
                    text: String::new(),
                    layout: LayoutDecl::default(),
                    props: Props::default(),
                    children: vec![],
                }],
                hotkeys: vec![],
            },
            fonts: FontDecl::default(),
        };
        assert!(tree.validate().is_err());
    }

    #[test]
    fn find_by_id() {
        let toml = UiTree::example_toml();
        let tree = UiTree::from_toml(&toml).unwrap();
        let btn = tree.window.children[0].find_by_id(1001).unwrap();
        assert_eq!(btn.kind, NodeKind::Button);
        assert_eq!(btn.text, "Run");
    }

    #[test]
    fn roundtrip_toml() {
        let original = UiTree::example_toml();
        let tree = UiTree::from_toml(&original).unwrap();
        let serialized = toml::to_string_pretty(&tree).unwrap();
        let tree2 = UiTree::from_toml(&serialized).unwrap();
        assert_eq!(tree.window.title, tree2.window.title);
    }

    /// BDD: 主程序 TOML 应描述完整的三段式布局
    ///
    /// Given: main.win7ui.toml 定义了工具栏 Row + 状态栏 Label + Split(编辑器+日志)
    /// When:  解析为 UiTree 并验证结构
    /// Then:  根级 children=3, Row 内 8 个按钮, Split 内 CodeEditor+LogView
    #[test]
    fn parse_main_win7ui_toml() {
        let toml = r#"
[fonts]
ui = { face = "Microsoft YaHei", size = 16 }
fixed = { face = "NSimSun", size = 16 }

[window]
title = "PyAuto Rust Win7 Native"
width = 1120
height = 780

[[window.hotkeys]]
id = 201
key = "F5"

[[window.hotkeys]]
id = 202
key = "F11"

[[window.children]]
type = "row"
layout = { size = [0, 38], margin = [10, 10, 0, 10] }

  [[window.children.children]]
  type = "button"
  id = 105
  text = "打开"
  layout = { size = [76, 28] }

  [[window.children.children]]
  type = "button"
  id = 106
  text = "保存"
  layout = { size = [76, 28] }

  [[window.children.children]]
  type = "button"
  id = 107
  text = "另存为"
  layout = { size = [86, 28] }

  [[window.children.children]]
  type = "button"
  id = 103
  text = "运行 F5"
  layout = { size = [92, 28] }

  [[window.children.children]]
  type = "button"
  id = 104
  text = "停止 F11"
  layout = { size = [96, 28] }

  [[window.children.children]]
  type = "button"
  id = 108
  text = "框选截图"
  layout = { size = [98, 28] }

  [[window.children.children]]
  type = "button"
  id = 109
  text = "点击截图"
  layout = { size = [98, 28] }

  [[window.children.children]]
  type = "button"
  id = 110
  text = "捕获坐标"
  layout = { size = [98, 28] }

[[window.children]]
type = "label"
id = 120
text = "就绪"
layout = { size = [0, 28], margin = [6, 10, 0, 10] }

[[window.children]]
type = "split"
layout = { weight = 1, margin = [4, 10, 10, 10] }
props = { split_right_width = 410 }

  [[window.children.children]]
  type = "code_editor"
  id = 101
  layout = { weight = 1 }
  props = { gutter_width = 48 }

  [[window.children.children]]
  type = "log_view"
  id = 102
  layout = { weight = 1 }
  props = { log_max_chars = 80000 }
"#;
        let tree = UiTree::from_toml(toml).unwrap();

        // Then: 根级结构
        assert_eq!(tree.window.title, "PyAuto Rust Win7 Native");
        assert_eq!(tree.window.children.len(), 3);
        assert_eq!(tree.window.hotkeys.len(), 2);

        // Then: 第 1 段 = Row 工具栏
        let row = &tree.window.children[0];
        assert_eq!(row.kind, NodeKind::Row);
        assert_eq!(row.children.len(), 8);
        // 验证按钮 ID 映射
        assert_eq!(row.children[0].id, Some(105)); // 打开
        assert_eq!(row.children[3].id, Some(103)); // 运行
        assert_eq!(row.children[4].id, Some(104)); // 停止

        // Then: 第 2 段 = Label 状态栏
        let label = &tree.window.children[1];
        assert_eq!(label.kind, NodeKind::Label);
        assert_eq!(label.id, Some(120));
        assert_eq!(label.text, "就绪");
        assert_eq!(label.layout.size, Some((0, 28)));

        // Then: 第 3 段 = Split(编辑器+日志)
        let split = &tree.window.children[2];
        assert_eq!(split.kind, NodeKind::Split);
        assert_eq!(split.children.len(), 2);
        assert_eq!(split.children[0].kind, NodeKind::CodeEditor);
        assert_eq!(split.children[0].id, Some(101));
        assert_eq!(split.children[1].kind, NodeKind::LogView);
        assert_eq!(split.children[1].id, Some(102));
        assert_eq!(split.props.split_right_width, 410);
    }
}
