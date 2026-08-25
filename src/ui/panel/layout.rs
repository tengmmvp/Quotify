//! 面板布局的单一事实源。
//!
//! 历史教训：输入框 y 坐标曾在 draw_settings（y 累加链）、update_caret、
//! attach_ime、view_height 四处各维护一份手抄表，两次失同步（自定义
//! 间隔框的光标一度落到框外 16px）。本模块把几何数值收敛为一处，
//! 绘制、光标、IME、高度公式四方引用同一常量。
//!
//! 添加页坐标推导（逻辑像素，dy=0 静止态）：
//! 12 起 → 导航 +30 → section_label +21 → sub_label(平台) +21
//! → segmented +40 → sub_label(类型) +21 → segmented +40 → 输入组
//! （sub_label +21 → input 顶）。

/// 导航栏高度（标题区 + 下间距）
pub const NAV_H: f32 = 30.0;
/// section_label 返回的段高（含下间距）
pub const SECTION_LABEL_H: f32 = 21.0;
/// segmented_raw 段体高度
pub const SEGMENTED_H: f32 = 30.0;
/// segmented_raw 返回值内含的段后间距
pub const SEGMENTED_GAP: f32 = 10.0;
/// 自绘输入框高度
pub const INPUT_H: f32 = 26.0;
/// 输入框左侧 x（与内容区 pad 一致）
pub const INPUT_X: f32 = 20.0;
/// 输入框后到下一 sub_label 的间距
pub const INPUT_GAP: f32 = 6.0;

/// 添加页：名称输入框顶部 y
pub const ADD_NAME_Y: f32 = 206.0;
/// 添加页：API Key 输入框顶部 y
pub const ADD_KEY_Y: f32 = 259.0;
/// 添加页（团队版）：组织 ID 输入框顶部 y
pub const ADD_ORG_Y: f32 = 312.0;
/// 添加页（团队版）：项目 ID 输入框顶部 y
pub const ADD_PROJECT_Y: f32 = 365.0;

/// 光标 y 相对输入框顶的偏移：caret 高 16 垂直居中于 26 高的框
/// （(26−16)/2 = 5，与文字块 6–22 的顶部对齐差 1px，视觉居中）
pub const CARET_Y_OFFSET: f32 = 5.0;

/// 设置页：自定义间隔输入框顶部 y。随账号块与鉴权错误行伸缩：
/// 12 + 导航 30 + 账号区标题 21 +（有账号：卡片 48 + 错误行 18｜无账号：
/// 添加按钮 36）+ 轮询区标题 33 + 分段 40 + 输入行偏移 2。
pub fn interval_input_y(has_account: bool, auth_error: bool) -> f32 {
    let mut y = 12.0 + NAV_H + SECTION_LABEL_H;
    y += if has_account {
        40.0 + 8.0 + if auth_error { 18.0 } else { 0.0 }
    } else {
        36.0
    };
    y + 12.0 + SECTION_LABEL_H + SEGMENTED_H + SEGMENTED_GAP + 2.0
}

/// 添加页总高（逻辑像素）：338；团队版追加组织/项目两行（106）。
pub fn add_page_height(team: bool) -> i32 {
    338 + if team { 106 } else { 0 }
}
