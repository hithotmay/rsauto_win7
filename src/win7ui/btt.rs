//! BTT — Build-Time Template
//!
//! 将 DTT 声明式数据模型 (UiTree/Node) 渲染为实际 Win32 控件树的引擎。
//!
//! 核心职责：
//! - 递归遍历 UiTree，为每个 Node 创建对应的 Win32 控件
//! - 计算布局坐标（Row/Column/Split/绝对定位）
//! - 返回 BuiltTree 结构，持有所有创建的 HWND 和布局信息
//! - 提供 BuiltTree::on_resize() 自动重算布局
//! - 提供 built.id(N) 按 ID 查找已创建控件的 HWND

use super::controls as ctrl;
use super::dtt::*;
use super::code_editor::CodeEditor;
use super::log_view::LogView;
use super::rich_edit::RichEdit;
use super::font;
use super::layout;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOW};
use std::collections::HashMap;

// ─── 错误 ───────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum BttError {
    #[error("node not found: {0}")]
    NodeNotFound(String),
    #[error("invalid layout: {0}")]
    Layout(String),
}

// ─── 已构建控件信息 ─────────────────────────────────────────

/// 一个已创建的控件及其元数据
#[derive(Debug, Clone)]
pub struct BuiltNode {
    pub kind: NodeKind,
    pub hwnd: Option<HWND>, // None for containers (Row/Column/Split)
    pub id: Option<i32>,
    pub layout: LayoutDecl,
    pub props: Props,
    pub children: Vec<BuiltNode>,
    /// CodeEditor 复合控件的句柄（如果有）
    pub code_editor: Option<CodeEditor>,
    /// LogView 复合控件的句柄（如果有）
    pub log_view: Option<LogView>,
}

// ─── 构建结果 ───────────────────────────────────────────────

/// DTT 树构建后的完整结果
#[derive(Debug, Clone)]
pub struct BuiltTree {
    pub window: HWND,
    pub root_children: Vec<BuiltNode>,
    /// id -> BuiltNode 的快速查找表（扁平化）
    id_map: HashMap<i32, BuiltNodeRef>,
    /// 字体句柄
    pub ui_font: isize,
    pub fixed_font: isize,
    /// 动态 gutter 宽度覆盖（CodeEditor id -> gutter_width）
    editor_gutter_widths: HashMap<i32, i32>,
}

/// BuiltNode 的轻量引用（用于 id_map）
#[derive(Debug, Clone)]
pub struct BuiltNodeRef {
    pub kind: NodeKind,
    pub hwnd: Option<HWND>,
    pub code_editor: Option<CodeEditor>,
    pub log_view: Option<LogView>,
}

impl BuiltTree {
    /// 按 ID 查找已创建的控件 HWND
    pub fn hwnd_by_id(&self, id: i32) -> Option<HWND> {
        self.id_map.get(&id).and_then(|r| r.hwnd)
    }

    /// 按 ID 查找 CodeEditor
    pub fn code_editor_by_id(&self, id: i32) -> Option<&CodeEditor> {
        self.id_map.get(&id).and_then(|r| r.code_editor.as_ref())
    }

    /// 按 ID 查找 LogView
    pub fn log_view_by_id(&self, id: i32) -> Option<&LogView> {
        self.id_map.get(&id).and_then(|r| r.log_view.as_ref())
    }

    /// 按 ID 获取 BuiltNodeRef
    pub fn node_by_id(&self, id: i32) -> Option<&BuiltNodeRef> {
        self.id_map.get(&id)
    }

    /// 窗口尺寸变化时重新计算布局
    pub fn on_resize(&mut self, client_w: i32, client_h: i32) {
        layout_children(
            &mut self.root_children,
            0,
            0,
            client_w,
            client_h,
            &self.editor_gutter_widths,
        );
    }

    /// 动态设置某个 CodeEditor 的 gutter 宽度（用于行号开关）
    /// 调用后需触发 on_resize 或手动 layout 才能生效
    pub fn set_editor_gutter_width(&mut self, editor_id: i32, width: i32) {
        self.editor_gutter_widths.insert(editor_id, width);
    }

    /// 获取某个 CodeEditor 当前的 gutter 宽度（含动态覆盖）
    pub fn editor_gutter_width(&self, editor_id: i32, default_width: i32) -> i32 {
        self.editor_gutter_widths.get(&editor_id).copied().unwrap_or(default_width)
    }

    /// 切换 TabControl 的活动页面
    /// `tab_ctrl_id`: TabControl 控件的 ID
    /// `page_index`: 要显示的 tab 页面索引（0-based）
    pub fn switch_tab(&mut self, tab_ctrl_id: i32, page_index: usize) {
        // 在树中查找指定的 TabControl
        let tab_node = self.root_children.iter_mut().find_map(|n| {
            find_tab_control_mut(n, tab_ctrl_id)
        });
        if let Some(tab) = tab_node {
            let count = tab.children.len();
            if page_index >= count {
                return;
            }
            for (i, child) in tab.children.iter().enumerate() {
                let show = i == page_index;
                set_node_visible_recursive(child, show);
            }
        }
    }
}

// ─── 构建入口 ───────────────────────────────────────────────

/// 构建选项
#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// 命令 ID 起始偏移（避免与已有控件 ID 冲突）
    pub id_offset: i32,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { id_offset: 0 }
    }
}

/// 将 DTT UiTree 渲染为 Win32 控件，挂载到已有窗口下
///
/// # Safety
/// 调用者必须确保 parent 是有效的 HWND
pub unsafe fn build(
    tree: &UiTree,
    parent: HWND,
    opts: &BuildOptions,
) -> Result<BuiltTree, BttError> {
    // 创建字体
    let ui_font = font::create_ui_font(tree.fonts.ui.size) as isize;
    let fixed_font = font::create_fixed_font(tree.fonts.fixed.size) as isize;

    // 构建子控件
    let mut id_map = HashMap::new();
    let mut built_children = Vec::new();

    for child in &tree.window.children {
        let built = build_node(child, parent, ui_font, fixed_font, opts, &mut id_map)?;
        built_children.push(built);
    }

    // 应用字体
    let all_hwnds: Vec<HWND> = collect_hwnds(&built_children);
    font::apply_font_handle_to_many(&all_hwnds, ui_font);

    // LogView 使用 fixed_font（等宽字体适合日志）
    let log_hwnds: Vec<HWND> = collect_log_view_hwnds(&built_children);
    font::apply_font_handle_to_many(&log_hwnds, fixed_font);

    // RichEdit 控件需要 EM_SETCHARFORMAT 同步字体（WM_SETFONT 不够）
    // 对所有 RichEdit 控件同步字体，确保初始化/插入/粘贴字体一致
    {
        let rich_edits = collect_rich_edit_hwnds(&built_children);
        for re_hwnd in &rich_edits {
            // CodeEditor uses fixed_font, LogView uses fixed_font too
            unsafe {
                RichEdit::new(*re_hwnd).sync_font(fixed_font);
            }
        }
    }
    init_tab_pages(&built_children);

    Ok(BuiltTree {
        window: parent,
        root_children: built_children,
        id_map,
        ui_font,
        fixed_font,
        editor_gutter_widths: HashMap::new(),
    })
}

// ─── 节点构建 ───────────────────────────────────────────────

unsafe fn build_node(
    node: &Node,
    parent: HWND,
    ui_font: isize,
    fixed_font: isize,
    opts: &BuildOptions,
    id_map: &mut HashMap<i32, BuiltNodeRef>,
) -> Result<BuiltNode, BttError> {
    let ctrl_id = node.id.map(|id| id + opts.id_offset);

    // 先用占位位置创建控件，后面 layout_children 会调整
    let (hwnd, code_editor, log_view) = match node.kind {
        NodeKind::Button => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_button_at(parent, &node.text, id, 0, 0, 80, 28);
            (Some(h), None, None)
        }
        NodeKind::Label => {
            let h = ctrl::create_label(parent, &node.text, 0, 0, 80, 20);
            (Some(h), None, None)
        }
        NodeKind::Checkbox => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_checkbox(parent, &node.text, id, 0, 0, 120, 20);
            if node.props.checked {
                ctrl::checkbox_set_checked(h, true);
            }
            (Some(h), None, None)
        }
        NodeKind::Edit => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_single_line_edit(parent, &node.text, id, 0, 0, 120, 24);
            (Some(h), None, None)
        }
        NodeKind::MultilineEdit => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_multiline_edit(
                parent, &node.text, id, 0, 0, 200, 100,
                node.props.readonly, node.props.hscroll,
            );
            (Some(h), None, None)
        }
        NodeKind::ComboBox => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_combo_box(parent, id, 0, 0, 120, 200);
            for item in &node.props.items {
                ctrl::combo_add_string(h, item);
            }
            if node.props.selected > 0 {
                ctrl::combo_set_selected(h, node.props.selected);
            }
            (Some(h), None, None)
        }
        NodeKind::ListBox => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_list_box(parent, id, 0, 0, 120, 100);
            for item in &node.props.items {
                ctrl::listbox_add_string(h, item);
            }
            if node.props.selected > 0 {
                ctrl::listbox_set_selected(h, node.props.selected);
            }
            (Some(h), None, None)
        }
        NodeKind::ProgressBar => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_progress_bar(parent, id, 0, 0, 200, 20);
            ctrl::progress_set_range(h, node.props.min, node.props.max);
            if node.props.value > 0 {
                ctrl::progress_set_value(h, node.props.value);
            }
            (Some(h), None, None)
        }
        NodeKind::TabControl => {
            let id = ctrl_id.unwrap_or(0);
            let h = ctrl::create_tab_control(parent, id, 0, 0, 300, 200);
            for (i, tab) in node.props.tabs.iter().enumerate() {
                ctrl::tab_insert_item(h, i as i32, tab);
            }
            ctrl::tab_set_selected(h, 0);
            (Some(h), None, None)
        }
        NodeKind::CodeEditor => {
            let id = ctrl_id.unwrap_or(0);
            let gw = node.props.gutter_width.unwrap_or(48);
            let ce = CodeEditor::create(parent, &node.text, id, 0, 0, 400, 300, gw, fixed_font);
            let script_hwnd = ce.script_hwnd();
            (Some(script_hwnd), Some(ce), None)
        }
        NodeKind::LogView => {
            let id = ctrl_id.unwrap_or(0);
            let re = RichEdit::create(parent, &node.text, id, 0, 0, 400, 200);
            let mut lv = LogView::new(re.hwnd(), node.props.log_max_chars);
            lv.set_font(fixed_font as isize);
            (Some(re.hwnd()), None, Some(lv))
        }
        // GroupBox 容器——创建 BS_GROUPBOX 视觉边框
        NodeKind::Group => {
            let h = ctrl::create_group_box(
                parent, &node.text, ctrl_id.unwrap_or(0),
                0, 0, 200, 100,
            );
            (Some(h), None, None)
        }
        // 纯逻辑容器——无 HWND
        NodeKind::Row | NodeKind::Column | NodeKind::Split => {
            (None, None, None)
        }
        NodeKind::Canvas => {
            // Canvas 保留——将来用于自绘区域
            (None, None, None)
        }
    };

    // ── 扁平化：对普通控件禁用视觉主题，但保留 RichEdit 滚动条主题 ──
    // RichEdit（编辑器、日志框）的滚动条依赖系统主题绘制，去掉后变经典灰色不好看
    if let Some(h) = hwnd {
        if !matches!(node.kind, NodeKind::CodeEditor | NodeKind::LogView) {
            ctrl::set_flat_theme(h);
        }
    }

    // 注册 ID 查找表
    if let Some(id) = node.id {
        id_map.insert(id, BuiltNodeRef {
            kind: node.kind.clone(),
            hwnd,
            code_editor: code_editor.clone(),
            log_view: log_view.clone(),
        });
    }

    // 递归构建子节点
    // 对于容器类型，子节点也挂到 parent 上（Win32 不支持真正的嵌套 HWND）
    // 但 TabControl 是例外：其子页面不能作为 TabControl 的子窗口（会被 Tab 头遮挡），
    // 而应作为 TabControl 的 parent（即主窗口）的子窗口
    let effective_parent = match node.kind {
        NodeKind::TabControl => parent, // 页面内容挂到 TabControl 的父窗口
        _ => hwnd.unwrap_or(parent),
    };
    let mut built_children = Vec::new();
    for child in &node.children {
        let built = build_node(child, effective_parent, ui_font, fixed_font, opts, id_map)?;
        built_children.push(built);
    }

    // 如果控件 disabled
    if let Some(h) = hwnd {
        if node.props.disabled {
            ctrl::enable_window(h, false);
        }
        ShowWindow(h, SW_SHOW);
    }

    Ok(BuiltNode {
        kind: node.kind.clone(),
        hwnd,
        id: node.id,
        layout: node.layout.clone(),
        props: node.props.clone(),
        children: built_children,
        code_editor,
        log_view,
    })
}

// ─── 布局计算 ───────────────────────────────────────────────

/// 递归计算子节点布局并移动窗口
///
/// `vertical`: Some(true) 强制垂直排列，Some(false) 强制水平排列，None 自动推断
fn layout_children_impl(
    children: &mut [BuiltNode],
    x: i32, y: i32, w: i32, h: i32,
    vertical_override: Option<bool>,
    gutter_overrides: &HashMap<i32, i32>,
) {
    if children.is_empty() || w <= 0 || h <= 0 {
        return;
    }

    // 检查是否所有子节点都是绝对定位
    let all_absolute = children.iter().all(|c| c.layout.pos.is_some());
    if all_absolute {
        for child in children {
            let (cx, cy) = child.layout.pos.unwrap();
            let (cw, ch) = child.layout.size.unwrap_or((w, h));
            apply_node_position(child, x + cx, y + cy, cw, ch, gutter_overrides);
        }
        return;
    }

    // 区分固定尺寸和权重尺寸的子节点
    let total_weight: i32 = children.iter().map(|c| c.layout.weight.max(0)).sum();
    let has_weights = total_weight > 0;

    // 方向：优先使用外部指定的方向，否则自动推断
    let vertical = vertical_override.unwrap_or(true);

    if has_weights {
        layout_weighted(children, x, y, w, h, vertical, total_weight, gutter_overrides);
    } else {
        layout_fixed(children, x, y, w, h, vertical, gutter_overrides);
    }
}

/// 公开入口：根级布局，自动垂直
fn layout_children(children: &mut [BuiltNode], x: i32, y: i32, w: i32, h: i32, gutter_overrides: &HashMap<i32, i32>) {
    layout_children_impl(children, x, y, w, h, None, gutter_overrides);
}

/// 按权重弹性分配空间
fn layout_weighted(
    children: &mut [BuiltNode],
    x: i32, y: i32, w: i32, h: i32,
    vertical: bool,
    total_weight: i32,
    gutter_overrides: &HashMap<i32, i32>,
) {
    // 先算出 fixed-size 子节点占用的空间，剩余空间按 weight 分配
    let fixed_total: i32 = children.iter()
        .filter(|c| c.layout.weight == 0)
        .map(|c| {
            let (fw, fh) = c.layout.size.unwrap_or((0, 0));
            if vertical { fh.max(20) } else { fw.max(20) }
        })
        .sum();
    let remain_v = (h - fixed_total).max(0);
    let remain_h = (w - fixed_total).max(0);

    let mut offset = 0;
    for child in children {
        let (fixed_w, fixed_h) = child.layout.size.unwrap_or((0, 0));
        let weight = child.layout.weight.max(0);

        let (cw, ch, cx, cy) = if vertical {
            let ch = if weight > 0 && total_weight > 0 {
                (remain_v as f64 * weight as f64 / total_weight as f64) as i32
            } else {
                fixed_h.max(20)
            };
            let cw = if fixed_w > 0 { fixed_w } else { w };
            let cx = 0;
            let cy = offset;
            (cw, ch, cx, cy)
        } else {
            let cw = if weight > 0 && total_weight > 0 {
                (remain_h as f64 * weight as f64 / total_weight as f64) as i32
            } else {
                fixed_w.max(20)
            };
            let ch = if fixed_h > 0 { fixed_h } else { h };
            let cx = offset;
            let cy = 0;
            (cw, ch, cx, cy)
        };

        apply_node_position(child, x + cx, y + cy, cw, ch, gutter_overrides);

        if vertical {
            offset += ch;
        } else {
            offset += cw;
        }
    }
}

/// 固定尺寸顺序排列
fn layout_fixed(
    children: &mut [BuiltNode],
    x: i32, y: i32, w: i32, h: i32,
    vertical: bool,
    gutter_overrides: &HashMap<i32, i32>,
) {
    let mut offset = 0;
    for child in children {
        let (fixed_w, fixed_h) = child.layout.size.unwrap_or(if vertical {
            (w, 28)
        } else {
            (80, h)
        });

        let (cw, ch, cx, cy) = if vertical {
            let cw = if fixed_w > 0 { fixed_w } else { w };
            let ch = fixed_h;
            (cw, ch, 0, offset)
        } else {
            let cw = fixed_w;
            let ch = if fixed_h > 0 { fixed_h } else { h };
            (cw, ch, offset, 0)
        };

        apply_node_position(child, x + cx, y + cy, cw, ch, gutter_overrides);

        if vertical {
            offset += ch;
        } else {
            offset += cw;
        }
    }
}

/// 将计算好的位置应用到节点（及其子节点）
fn apply_node_position(node: &mut BuiltNode, x: i32, y: i32, w: i32, h: i32, gutter_overrides: &HashMap<i32, i32>) {
    // 应用 margin
    let margin = &node.layout.margin;
    let (mt, mr, mb, ml) = if margin.len() == 4 {
        (margin[0], margin[1], margin[2], margin[3])
    } else if margin.len() == 1 {
        let m = margin[0];
        (m, m, m, m)
    } else {
        (0, 0, 0, 0)
    };

    let x = x + ml;
    let y = y + mt;
    let w = (w - ml - mr).max(0);
    let h = (h - mt - mb).max(0);

    if let Some(hwnd) = node.hwnd {
        unsafe { layout::move_window(hwnd, x, y, w, h); }
    }

    // 容器类型需要递归布局子控件
    match &node.kind {
        NodeKind::Row => {
            layout_children_impl(&mut node.children, x, y, w, h, Some(false), gutter_overrides);
        }
        NodeKind::Column => {
            layout_children_impl(&mut node.children, x, y, w, h, Some(true), gutter_overrides);
        }
        NodeKind::Split => {
            let right_w = node.props.split_right_width;
            let gap: i32 = 2; // 编辑器和输出框之间的间距
            let left_w = (w - right_w - gap).max(0);
            if node.children.len() == 2 {
                apply_node_position(&mut node.children[0], x, y, left_w, h, gutter_overrides);
                apply_node_position(&mut node.children[1], x + left_w + gap, y, right_w, h, gutter_overrides);
            }
        }
        NodeKind::Group => {
            layout_children_impl(&mut node.children, x, y, w, h, None, gutter_overrides);
        }
        NodeKind::TabControl => {
            // Tab 页面子节点全部叠加在 tab 显示区域
            // Tab 头部高度约 28px，子页面占据剩余空间
            let tab_header_h = 28i32;
            let page_y = y + tab_header_h;
            let page_h = (h - tab_header_h).max(0);
            for child in &mut node.children {
                apply_node_position(child, x, page_y, w, page_h, gutter_overrides);
            }
        }
        NodeKind::CodeEditor => {
            if let Some(ref ce) = node.code_editor {
                let default_gw = node.props.gutter_width.unwrap_or(48);
                let gw = node.id
                    .and_then(|id| gutter_overrides.get(&id).copied())
                    .unwrap_or(default_gw);
                unsafe { ce.layout(x, y, w, h, gw); }
            }
        }
        _ => {}
    }
}

// ─── 辅助 ───────────────────────────────────────────────────

fn collect_hwnds(nodes: &[BuiltNode]) -> Vec<HWND> {
    let mut result = Vec::new();
    for node in nodes {
        // CodeEditor and LogView manage their own fonts — skip them
        match node.kind {
            NodeKind::CodeEditor | NodeKind::LogView => {}
            _ => {
                if let Some(h) = node.hwnd {
                    result.push(h);
                }
            }
        }
        result.extend(collect_hwnds(&node.children));
    }
    result
}

/// Collect LogView HWNDs for fixed-font application
fn collect_log_view_hwnds(nodes: &[BuiltNode]) -> Vec<HWND> {
    let mut result = Vec::new();
    for node in nodes {
        if node.log_view.is_some() {
            if let Some(h) = node.hwnd {
                result.push(h);
            }
        }
        result.extend(collect_log_view_hwnds(&node.children));
    }
    result
}

/// Collect HWNDs of all RichEdit-based controls (CodeEditor + LogView)
fn collect_rich_edit_hwnds(nodes: &[BuiltNode]) -> Vec<HWND> {
    let mut result = Vec::new();
    for node in nodes {
        if let Some(ref ce) = node.code_editor {
            result.push(ce.script_hwnd());
        }
        if node.log_view.is_some() {
            if let Some(h) = node.hwnd {
                result.push(h);
            }
        }
        result.extend(collect_rich_edit_hwnds(&node.children));
    }
    result
}

// ─── Tab 页面管理 ──────────────────────────────────────────

/// 初始化所有 TabControl 页面：只显示第一个 tab 的内容，隐藏其余
fn init_tab_pages(nodes: &[BuiltNode]) {
    for node in nodes {
        if node.kind == NodeKind::TabControl {
            // TabControl 的 children 是各个 tab 页面
            for (i, child) in node.children.iter().enumerate() {
                let show = i == 0;
                set_node_visible_recursive(child, show);
            }
        }
        // 递归处理子节点中的 TabControl
        init_tab_pages(&node.children);
    }
}

/// 递归设置节点及其所有子节点的可见性
fn set_node_visible_recursive(node: &BuiltNode, show: bool) {
    if let Some(hwnd) = node.hwnd {
        unsafe {
            ShowWindow(hwnd, if show { SW_SHOW } else { SW_HIDE });
        }
    }
    // CodeEditor 有额外的子 HWND
    if let Some(ref ce) = node.code_editor {
        unsafe {
            ShowWindow(ce.gutter_hwnd(), if show { SW_SHOW } else { SW_HIDE });
        }
    }
    for child in &node.children {
        set_node_visible_recursive(child, show);
    }
}

/// 收集节点及其所有子节点的 HWND（包含 CodeEditor gutter）
fn collect_all_hwnds_recursive(node: &BuiltNode) -> Vec<HWND> {
    let mut result = Vec::new();
    if let Some(h) = node.hwnd {
        result.push(h);
    }
    if let Some(ref ce) = node.code_editor {
        result.push(ce.gutter_hwnd());
    }
    for child in &node.children {
        result.extend(collect_all_hwnds_recursive(child));
    }
    result
}

/// 在树中递归查找指定 ID 的 TabControl 节点（可变引用）
fn find_tab_control_mut<'a>(node: &'a mut BuiltNode, target_id: i32) -> Option<&'a mut BuiltNode> {
    if node.kind == NodeKind::TabControl && node.id == Some(target_id) {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_tab_control_mut(child, target_id) {
            return Some(found);
        }
    }
    None
}

// ─── Builder 便捷 API ───────────────────────────────────────

/// 声明式 UI Builder — 用 Rust 代码构建 DTT 树，然后 BTT 渲染
///
/// 用法：
/// ```ignore
/// use win7ui::dtt::*;
/// use win7ui::btt::*;
///
/// let tree = UiTree::from_toml(include_str!("my_ui.win7ui.toml")).unwrap();
/// // 在 WM_CREATE 中：
/// let built = unsafe { build(&tree, hwnd, &BuildOptions::default()).unwrap() };
/// // 在 WM_SIZE 中：
/// built.on_resize(client_w, client_h);
/// // 查找控件：
/// let run_btn = built.hwnd_by_id(1001);
/// ```
pub struct Ui;

impl Ui {
    /// 从 TOML 文本构建 UI
    ///
    /// # Safety
    /// parent 必须是有效 HWND
    pub unsafe fn from_toml(
        toml: &str,
        parent: HWND,
    ) -> Result<BuiltTree, Box<dyn std::error::Error>> {
        let tree = UiTree::from_toml(toml)?;
        let built = build(&tree, parent, &BuildOptions::default())?;
        Ok(built)
    }

    /// 从已解析的 UiTree 构建 UI（跨平台入口）
    ///
    /// # Safety
    /// parent 必须是有效 HWND
    pub unsafe fn from_tree(
        tree: &crate::ui::dtt::UiTree,
        parent: HWND,
    ) -> Result<BuiltTree, Box<dyn std::error::Error>> {
        let built = build(tree, parent, &BuildOptions::default())?;
        Ok(built)
    }

    /// 从 .win7ui.toml 文件构建 UI
    ///
    /// # Safety
    /// parent 必须是有效 HWND
    pub unsafe fn from_file(
        path: &std::path::Path,
        parent: HWND,
    ) -> Result<BuiltTree, Box<dyn std::error::Error>> {
        let tree = UiTree::load_file(path)?;
        let built = build(&tree, parent, &BuildOptions::default())?;
        Ok(built)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_validate_example() {
        let toml = UiTree::example_toml();
        let tree = UiTree::from_toml(&toml).unwrap();
        assert!(tree.validate().is_ok());
        // 验证 id_map 构建逻辑（非 Win32 部分）
        let mut id_map: HashMap<i32, BuiltNodeRef> = HashMap::new();
        for child in &tree.window.children {
            if let Some(id) = child.id {
                id_map.insert(id, BuiltNodeRef {
                    kind: child.kind.clone(),
                    hwnd: None,
                    code_editor: None,
                    log_view: None,
                });
            }
        }
        // Row 没有 id，它的子节点有
        let row = &tree.window.children[0];
        for child in &row.children {
            if let Some(id) = child.id {
                id_map.insert(id, BuiltNodeRef {
                    kind: child.kind.clone(),
                    hwnd: None,
                    code_editor: None,
                    log_view: None,
                });
            }
        }
        assert!(id_map.contains_key(&1001)); // Run button
        assert!(id_map.contains_key(&1002)); // Stop button
    }
}
