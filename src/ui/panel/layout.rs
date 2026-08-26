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
    let mut y = 12.0 + NAV_H + SECTION_LABEL_H;
    // 有账号：卡片 40 + 间距 8；随后常驻的添加按钮行再占 36
    y += if has_account {
        40.0 + 8.0 + if auth_error { 18.0 } else { 0.0 } + 36.0
    } else {
        36.0
    };
    // 尾段：轮询区标题 rule 上隙 12 + 标题 + 分段体 + 段后隙
    y + 12.0 + SECTION_LABEL_H + SEGMENTED_H + SEGMENTED_GAP
}

/// 设置页高峰区间输入行顶 y，位于通知区之后
pub fn peak_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // 展开自定义间隔时渲染链从 interval_input_y + 38 续走：38 = 输入框 26 + 尾隙 12；
    // 未展开时分段控件返回值即 interval_input_y 本身，两态 y 流同点
    let after_interval = if customizing { 38.0 } else { 0.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + 33.0 // 通知区标题
        + 126.0 // 三个通知开关行
        + 33.0 // 高峰区间区标题
}

/// 设置页代理输入框顶 y，与 draw_settings 的 y 推进链逐段对齐
pub fn proxy_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // 展开自定义间隔时渲染链从 interval_input_y + 38 续走：38 = 输入框 26 + 尾隙 12；
    // 未展开时分段控件返回值即 interval_input_y 本身，两态 y 流同点
    let after_interval = if customizing { 38.0 } else { 0.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + 33.0 // 通知区标题
        + 126.0 // 三个通知开关行
        + 67.0 // 高峰区间区：标题 33 + 输入行 26 + 下隙 8
        + 33.0 // 通用区标题
        + 62.0 // 语言行：sub_label 21 + segmented 39 + 行后 2
        + 62.0 // 外观行，同语言行
        + 28.0 // 开机自启行
        + 33.0 // 网络代理区标题
        + 21.0 // 代理子标签
}

/// 添加页总高（逻辑像素），团队版追加组织/项目两行
pub fn add_page_height(team: bool) -> i32 {
    338 + if team { 106 } else { 0 }
}

/// 主视图总高（逻辑像素）：加载/失败态固定 300；数据态随指标行数与余额块伸缩，
/// 各段对照 draw_main 的 y 推进链（顶部留白 + 顶栏 52 + 刊头 42 +
/// 指标行 52×n + 余额块 40 + footer 40）
pub fn main_view_height(has_data: bool, rows: usize, has_balance: bool) -> i32 {
    if !has_data {
        return 300;
    }
    16 + 52 + 42 + rows as i32 * 52 + has_balance as i32 * 40 + 40
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
        assert_eq!(main_view_height(false, 0, false), 300);
        assert_eq!(main_view_height(true, 0, false), 150);
        assert_eq!(main_view_height(true, 1, false), 202);
        assert_eq!(main_view_height(true, 3, true), 346);
    }
}
