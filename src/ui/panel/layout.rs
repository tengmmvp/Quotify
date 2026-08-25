//! 面板布局的单一事实源

/// 轮询间隔预设档（秒）
pub const INTERVAL_PRESETS: [u64; 4] = [60, 300, 900, 1800];

/// 导航栏高度
pub const NAV_H: f32 = 30.0;
/// section_label 返回的段高
pub const SECTION_LABEL_H: f32 = 21.0;
/// segmented_raw 段体高度
pub const SEGMENTED_H: f32 = 30.0;
/// segmented_raw 返回值内含的段后间距
pub const SEGMENTED_GAP: f32 = 10.0;
/// 自绘输入框高度
pub const INPUT_H: f32 = 26.0;
/// 输入框左侧 x，与内容区 pad 一致
pub const INPUT_X: f32 = 20.0;
/// 输入框后到下一 sub_label 的间距
pub const INPUT_GAP: f32 = 6.0;

/// 添加页：名称输入框顶部 y
pub const ADD_NAME_Y: f32 = 206.0;
/// 添加页：API Key 输入框顶部 y
pub const ADD_KEY_Y: f32 = 259.0;
/// 添加页团队版：组织 ID 输入框顶部 y
pub const ADD_ORG_Y: f32 = 312.0;
/// 添加页团队版：项目 ID 输入框顶部 y
pub const ADD_PROJECT_Y: f32 = 365.0;

/// caret 高 16 在 26 高框内垂直居中的偏移
pub const CARET_Y_OFFSET: f32 = 5.0;

/// 高峰区间输入框 x（设置页内容区 pad = 20）
pub const PEAK_START_X: f32 = 48.0;
pub const PEAK_END_X: f32 = 148.0;

/// 设置页自定义间隔输入框顶 y，随账号块与错误行伸缩
pub fn interval_input_y(has_account: bool, auth_error: bool) -> f32 {
    let mut y = 12.0 + NAV_H + SECTION_LABEL_H;
    // 有账号：卡片 40 + 间距 8；随后常驻的添加按钮行再占 36
    y += if has_account {
        40.0 + 8.0 + if auth_error { 18.0 } else { 0.0 } + 36.0
    } else {
        36.0
    };
    y + 12.0 + SECTION_LABEL_H + SEGMENTED_H + SEGMENTED_GAP + 2.0
}

/// 设置页高峰区间输入行顶 y，位于通知区之后
pub fn peak_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // 自定义间隔时渲染链从 IY+38 续走；非自定义时 y 流已含 +10 间隙，比 IY 多 8
    let after_interval = if customizing { 38.0 } else { 8.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + 33.0 // 通知区标题
        + 126.0 // 三个通知开关行
        + 33.0 // 高峰区间区标题
}

/// 设置页代理输入框顶 y，与 draw_settings 的 y 推进链逐段对齐
pub fn proxy_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // 自定义间隔时渲染链从 IY+38 续走；非自定义时 y 流已含 +10 间隙，比 IY 多 8
    let after_interval = if customizing { 38.0 } else { 8.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + 33.0 // 通知区标题
        + 126.0 // 三个通知开关行
        + 67.0 // 高峰区间区：标题 33 + 输入行 26 + 下隙 8
        + 33.0 // 通用区标题
        + 63.0 // 语言行
        + 63.0 // 外观行
        + 28.0 // 开机自启行
        + 33.0 // 网络代理区标题
        + 21.0 // 代理子标签
}

/// 添加页总高（逻辑像素），团队版追加组织/项目两行
pub fn add_page_height(team: bool) -> i32 {
    338 + if team { 106 } else { 0 }
}
