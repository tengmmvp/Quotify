//! 面板布局的单一事实源

/// 轮询间隔预设档（秒）
pub const INTERVAL_PRESETS: [u64; 4] = [60, 300, 900, 1800];

/// 顶部导航栏高度
pub const NAV_H: f32 = 30.0;
/// section_label 返回值内含的段高
pub const SECTION_LABEL_H: f32 = 21.0;
/// segmented_raw 段体高度
pub const SEGMENTED_H: f32 = 30.0;
/// segmented_raw 返回值内含的段后间距
pub const SEGMENTED_GAP: f32 = 9.0;
/// 自绘输入框高度
pub const INPUT_H: f32 = 26.0;
/// 设置/主视图内容区左右留白
pub const CONTENT_PAD: f32 = 20.0;
/// 输入框左侧 x，与内容区 pad 一致
pub const INPUT_X: f32 = CONTENT_PAD;
/// 输入框后到下一 sub_label 的间距
pub const INPUT_GAP: f32 = 6.0;

// ── 主视图段高：draw_main 的 y 推进链与 main_view_height 两方同引一组常量 ──

/// 主视图顶部留白
pub(crate) const MAIN_TOP_PAD: f32 = 16.0;
/// 顶栏行高（账号刊头与右侧双钮所在行）
pub(crate) const MAIN_TOPBAR_H: f32 = 52.0;
/// 刊头：段起点到标题的上隙[实线分隔线低 2px 挂在段起点]
pub(crate) const MAIN_MASTHEAD_RULE_GAP: f32 = 14.0;
/// 刊头：标题行占高
pub(crate) const MAIN_MASTHEAD_ROW_H: f32 = 26.0;
/// 刊头段整高：段起点到首个指标行
pub(crate) const MAIN_MASTHEAD_H: f32 = MAIN_MASTHEAD_RULE_GAP + MAIN_MASTHEAD_ROW_H;
/// 指标行高
pub(crate) const MAIN_METRIC_ROW_H: f32 = 52.0;
/// MCP 构成区：框顶到 MCP 指标行底的下隙
pub(crate) const MAIN_MCP_COMP_TOP_GAP: f32 = 6.0;
/// MCP 构成框内边距（左右 8 上下 6）
pub(crate) const MAIN_MCP_COMP_PAD_X: f32 = 8.0;
pub(crate) const MAIN_MCP_COMP_PAD_Y: f32 = 6.0;
/// 能量格高（格宽 12、右斜切 4、缝 2 在渲染侧）
pub(crate) const MAIN_MCP_CELL_H: f32 = 10.0;
/// 能量条行到图例行的推进
pub(crate) const MAIN_MCP_LEGEND_ADV: f32 = 4.0;
/// 图例行高（11px 徽标文本所在行）
pub(crate) const MAIN_MCP_LEGEND_H: f32 = 15.0;
/// MCP 构成区整高：下隙 + 框线 2 + 上下边距 + 条 + 推进 + 图例行
pub(crate) const MAIN_MCP_COMP_H: f32 = MAIN_MCP_COMP_TOP_GAP
    + 2.0
    + 2.0 * MAIN_MCP_COMP_PAD_Y
    + MAIN_MCP_CELL_H
    + MAIN_MCP_LEGEND_ADV
    + MAIN_MCP_LEGEND_H;
/// 数据段（Token/余额）段前隙：到虚线分隔线
pub(crate) const MAIN_SECTION_GAP: f32 = 6.0;
/// 数据段：虚线分隔线到段标题的推进
pub(crate) const MAIN_SECTION_HEAD: f32 = 14.0;
/// Token 块：标题行到首条票据行的推进
pub(crate) const MAIN_TOKEN_ROWS_ADV: f32 = 22.0;
/// 票据合计行高
pub(crate) const MAIN_LEADER_ROW_H: f32 = 19.0;
/// Token 消耗块整高：段前隙 + 段头 + 标题推进 + 两行票据
pub(crate) const MAIN_TOKEN_BLOCK_H: f32 =
    MAIN_SECTION_GAP + MAIN_SECTION_HEAD + MAIN_TOKEN_ROWS_ADV + 2.0 * MAIN_LEADER_ROW_H;
/// 余额行文本占高
pub(crate) const MAIN_BALANCE_ROW_H: f32 = 18.0;
/// 余额块整高：可视为段前隙 + 段头 + 文本行，另含 2px 底部呼吸隙入高
pub(crate) const MAIN_BALANCE_BLOCK_H: f32 =
    MAIN_SECTION_GAP + MAIN_SECTION_HEAD + MAIN_BALANCE_ROW_H + 2.0;
/// 主视图内容底隙：最后一个数据块底到页脚顶
pub(crate) const MAIN_CONTENT_BOTTOM_GAP: f32 = 2.0;
/// 页脚区高（钉底）
pub(crate) const MAIN_FOOTER_H: f32 = 36.0;
/// 主视图尾段整高：内容底隙 + 页脚
pub(crate) const MAIN_TAIL_H: f32 = MAIN_CONTENT_BOTTOM_GAP + MAIN_FOOTER_H;

// ── 设置页区块段高：draw_settings 的 y 推进、下方 *_input_y 与 settings_view_height 三方共用 ──

/// 设置页上下边距：nav 前顶部留白与版本行后底部余量
pub const SETTINGS_EDGE_PAD: f32 = 12.0;
/// section_label 带 rule 时分隔线到标题的上隙
pub const SECTION_RULE_GAP: f32 = 12.0;
/// 带 rule 的区块标题整段高[轮询/通知/高峰/通用/网络/配置管理区共用]
pub const SECTION_RULE_H: f32 = SECTION_RULE_GAP + SECTION_LABEL_H;
/// 账号卡片整行高：卡片 40 + 卡后隙 8
pub const ACCOUNT_CARD_H: f32 = 48.0;
/// 鉴权失败提示行高
pub const AUTH_ERROR_H: f32 = 18.0;
/// 常驻添加账号按钮行高：按钮 30 含上下余量
pub const ADD_BTN_ROW_H: f32 = 36.0;
/// 自定义间隔展开增量：输入框 26 + 框后尾隙 12[值同 rule 上隙，语义各自独立]
pub const CUSTOMIZE_EXTRA_H: f32 = INPUT_H + 12.0;
/// 开关行高（带描述）：标题 19 + 描述 14 + 行后 9
pub const TOGGLE_ROW_H: f32 = 42.0;
/// 开关行高（无描述）：标题 19 + 行后 9
pub const TOGGLE_ROW_PLAIN_H: f32 = 28.0;
/// 高峰输入行后的下隙
pub const PEAK_TAIL_GAP: f32 = 8.0;
/// 选择行（语言/外观）行后余隙
pub const CHOICE_ROW_TAIL: f32 = 2.0;
/// 选择行整段高（语言/外观）：子标签 + 分段控件 + 行后余隙
pub const CHOICE_ROW_H: f32 = SECTION_LABEL_H + SEGMENTED_H + SEGMENTED_GAP + CHOICE_ROW_TAIL;
/// 代理输入框后的下隙[提示文字为框内占位，不另占行]
pub const PROXY_TAIL_GAP: f32 = 6.0;
/// 配置管理按钮行高：按钮 28 + 行后 9
pub const BACKUP_ROW_H: f32 = 37.0;
/// 关于区纯分隔（空标题）：rule 上隙 + 尾隙 6
pub const ABOUT_DIVIDER_H: f32 = SECTION_RULE_GAP + 6.0;
/// 版本行高：按钮顶偏移 1 + 按钮 28
pub const VERSION_ROW_H: f32 = 29.0;

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

/// 高峰起/止输入框 x，跟随各自文本标签右侧；设置页内容区 pad = 20
pub const PEAK_START_X: f32 = 48.0;
pub const PEAK_END_X: f32 = 148.0;

/// 设置页自定义间隔输入框顶 y，随账号块与错误行伸缩
pub fn interval_input_y(has_account: bool, auth_error: bool) -> f32 {
    let mut y = SETTINGS_EDGE_PAD + NAV_H + SECTION_LABEL_H;
    // 有账号：卡片 + 可选错误行；随后常驻的添加按钮行
    y += if has_account {
        ACCOUNT_CARD_H + if auth_error { AUTH_ERROR_H } else { 0.0 } + ADD_BTN_ROW_H
    } else {
        ADD_BTN_ROW_H
    };
    // 尾段：轮询区标题 + 分段体 + 段后隙
    y + SECTION_RULE_H + SEGMENTED_H + SEGMENTED_GAP
}

/// 设置页高峰区间输入行顶 y，位于通知区之后
pub fn peak_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // 展开自定义间隔时渲染链从 interval_input_y + CUSTOMIZE_EXTRA_H 续走；
    // 未展开时分段控件返回值即 interval_input_y 本身，两态 y 流同点
    let after_interval = if customizing { CUSTOMIZE_EXTRA_H } else { 0.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + SECTION_RULE_H // 通知区标题
        + 3.0 * TOGGLE_ROW_H // 三个通知开关行
        + SECTION_RULE_H // 高峰区间区标题
}

/// 设置页代理输入框顶 y：高峰区之后接通用区、网络区，续 peak_input_y 的链
pub fn proxy_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    peak_input_y(has_account, auth_error, customizing)
        + INPUT_H + PEAK_TAIL_GAP // 高峰输入行
        + SECTION_RULE_H // 通用区标题
        + CHOICE_ROW_H // 语言行
        + CHOICE_ROW_H // 外观行
        + TOGGLE_ROW_PLAIN_H // 开机自启行
        + SECTION_RULE_H // 网络代理区标题
        + SECTION_LABEL_H // 代理子标签
}

/// 设置页总高（逻辑像素）：从代理输入框续走配置管理、关于、版本行收尾，
/// 消除 view_height 里独立的整页求和
pub fn settings_view_height(has_account: bool, auth_error: bool, customizing: bool) -> i32 {
    (proxy_input_y(has_account, auth_error, customizing)
        + INPUT_H + PROXY_TAIL_GAP // 代理输入框
        + SECTION_RULE_H + BACKUP_ROW_H // 配置管理区
        + ABOUT_DIVIDER_H // 关于区纯分隔
        + VERSION_ROW_H // 版本行
        + SETTINGS_EDGE_PAD) as i32 // 底部余量
}

/// 添加页总高（逻辑像素），团队版追加组织/项目两行
pub fn add_page_height(team: bool) -> i32 {
    338 + if team { 106 } else { 0 }
}

/// 主视图总高（逻辑像素）：加载/失败态固定 300；数据态由上方主视图段
/// 常量链求和，与 draw_main 的 y 推进同源——两侧同引一组常量，几何改动
/// 不再可能出现绘制侧与高度公式各改一半的漂移
pub fn main_view_height(
    has_data: bool,
    rows: usize,
    has_mcp_comp: bool,
    has_stats: bool,
    has_balance: bool,
) -> i32 {
    if !has_data {
        return 300;
    }
    (MAIN_TOP_PAD
        + MAIN_TOPBAR_H
        + MAIN_MASTHEAD_H
        + rows as f32 * MAIN_METRIC_ROW_H
        + has_mcp_comp as i32 as f32 * MAIN_MCP_COMP_H
        + has_stats as i32 as f32 * MAIN_TOKEN_BLOCK_H
        + has_balance as i32 as f32 * MAIN_BALANCE_BLOCK_H
        + MAIN_TAIL_H) as i32
}

/// 钉位回归：期望值由渲染 y 链推导而来，布局改动须同步更新
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_input_y_pinned() {
        assert_eq!(interval_input_y(false, false), 171.0);
        assert_eq!(interval_input_y(false, true), 171.0);
        assert_eq!(interval_input_y(true, false), 219.0);
        assert_eq!(interval_input_y(true, true), 237.0);
    }

    #[test]
    fn peak_input_y_pinned() {
        assert_eq!(peak_input_y(false, false, false), 363.0);
        assert_eq!(peak_input_y(true, false, false), 411.0);
        assert_eq!(peak_input_y(true, true, false), 429.0);
        assert_eq!(peak_input_y(true, true, true), 467.0);
        assert_eq!(peak_input_y(false, false, true), 401.0);
    }

    #[test]
    fn proxy_input_y_pinned() {
        assert_eq!(proxy_input_y(false, false, false), 636.0);
        assert_eq!(proxy_input_y(true, false, false), 684.0);
        assert_eq!(proxy_input_y(true, true, false), 702.0);
        assert_eq!(proxy_input_y(true, true, true), 740.0);
        assert_eq!(proxy_input_y(false, false, true), 674.0);
    }

    #[test]
    fn add_page_height_pinned() {
        assert_eq!(add_page_height(false), 338);
        assert_eq!(add_page_height(true), 444);
    }

    #[test]
    fn main_view_height_pinned() {
        assert_eq!(main_view_height(false, 0, false, false, false), 300);
        assert_eq!(main_view_height(true, 0, false, false, false), 146);
        assert_eq!(main_view_height(true, 1, false, false, false), 198);
        assert_eq!(main_view_height(true, 3, false, false, true), 342);
        assert_eq!(main_view_height(true, 3, false, true, true), 422);
        assert_eq!(main_view_height(true, 2, false, true, false), 330);
        // MCP 构成区：无数据明细时零增量，有则 +49
        assert_eq!(main_view_height(true, 3, true, false, false), 351);
        assert_eq!(main_view_height(true, 1, true, false, false), 247);
    }
}
