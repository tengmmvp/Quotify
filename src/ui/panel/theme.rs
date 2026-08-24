//! 面板主题：Cursor 式「暖羊皮纸编辑风」token（颜色 / 圆角 / 字号 / 间距）。
//!
//! 奶油画布 + 暖墨文本 + 单一 Ember 强调色；平面纸感、hairline 分层、
//! 4px 锐利圆角；标题轻字重（weight 400）靠克制而非加粗建立层级。

/// 外观模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// 深浅色自适应的主题 token 集合。
pub struct Theme {
    /// 面板背景（Parchment 奶油画布，接近不透明的纸面）
    pub bg: [f32; 4],
    /// 主文本（Ink 暖墨，非纯黑）
    pub text_primary: [f32; 4],
    /// 次要文本（Driftwood）
    pub text_secondary: [f32; 4],
    /// 弱文本 / 标签（Ash）
    pub text_tertiary: [f32; 4],
    /// hairline 边框与分隔线（Stone）
    pub border: [f32; 4],
    /// 进度轨道 / 次级按钮填充（Linen）
    pub track: [f32; 4],
    /// 文字强调色（Ember——仅用于短文本与链接，不做大面积填充）
    pub accent: [f32; 4],
    /// 主按钮填充（浅色模式 Ink / 深色模式反转暖白）
    pub action: [f32; 4],
    /// 主按钮文字
    pub action_text: [f32; 4],
    /// 档位色：正常 Forest / 警告 Amber / 危险 Crimson（暖色系）
    pub ok: [f32; 4],
    pub warn: [f32; 4],
    pub danger: [f32; 4],
    /// logo 磁贴底色（与主按钮同源）
    pub logo_tile: [f32; 4],
}

/// 面板宽度（逻辑像素）。
pub const PANEL_WIDTH: i32 = 340;
/// 进度条高度。
pub const BAR_HEIGHT: f32 = 8.0;
/// 组件统一圆角（Cursor 的锐利纸感签名）。
pub const RADIUS: f32 = 4.0;

fn rgba(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self {
                bg: rgba(0xF7, 0xF7, 0xF4, 0.97),
                text_primary: rgba(0x26, 0x25, 0x1E, 0.94),
                text_secondary: rgba(0x84, 0x84, 0x7E, 1.0),
                text_tertiary: rgba(0x7A, 0x79, 0x74, 1.0),
                border: rgba(0xCD, 0xCD, 0xC9, 1.0),
                track: rgba(0xE6, 0xE5, 0xE0, 1.0),
                accent: rgba(0xF5, 0x4E, 0x00, 1.0),
                action: rgba(0x26, 0x25, 0x1E, 1.0),
                action_text: rgba(0xF7, 0xF7, 0xF4, 1.0),
                ok: rgba(0x34, 0x78, 0x5C, 1.0),
                warn: rgba(0xC0, 0x85, 0x32, 1.0),
                danger: rgba(0xCF, 0x2D, 0x56, 1.0),
                logo_tile: rgba(0x26, 0x25, 0x1E, 1.0),
            },
            // 暖深色：同色相的深纸墨反转（参考仅给出 light，dark 为对应创作）
            Appearance::Dark => Self {
                bg: rgba(0x20, 0x1F, 0x1B, 0.97),
                text_primary: rgba(0xEC, 0xEA, 0xE4, 0.94),
                text_secondary: rgba(0xA8, 0xA7, 0xA0, 1.0),
                text_tertiary: rgba(0x7D, 0x7C, 0x75, 1.0),
                border: rgba(0x3B, 0x3A, 0x34, 1.0),
                track: rgba(0x34, 0x33, 0x2E, 1.0),
                accent: rgba(0xFF, 0x6A, 0x2E, 1.0),
                action: rgba(0xEC, 0xE9, 0xE2, 1.0),
                action_text: rgba(0x26, 0x25, 0x1E, 1.0),
                ok: rgba(0x3F, 0xA3, 0x77, 1.0),
                warn: rgba(0xD4, 0xA0, 0x4C, 1.0),
                danger: rgba(0xE2, 0x5A, 0x77, 1.0),
                logo_tile: rgba(0x6A, 0x6A, 0x6A, 1.0),
            },
        }
    }

    /// 按用量百分比的档位色。
    pub fn tier_color(&self, used_percent: f64) -> [f32; 4] {
        if used_percent < 70.0 {
            self.ok
        } else if used_percent < 90.0 {
            self.warn
        } else {
            self.danger
        }
    }

    /// 检测系统外观（注册表 AppsUseLightTheme）。
    pub fn system_appearance() -> Appearance {
        let val = windows_registry::CURRENT_USER
            .open("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|k| k.get_u32("AppsUseLightTheme"))
            .unwrap_or(1);
        if val == 1 { Appearance::Light } else { Appearance::Dark }
    }
}
