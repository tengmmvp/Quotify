//! 账号切换页

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::{Align, Hit, Renderer};
use crate::ui::panel::model::PanelModel;

impl Renderer {
    /// 账号切换页：导航行 + 账号列表；当前项左竖条，hover 整行淡填充
    pub(super) unsafe fn draw_account_picker(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = 20.0;
        let mut y = dy + 12.0;

        // 导航行：返回 + 居中标题，同设置页
        self.back_arrow(target, Hit::Back, pad, y + 6.0);
        let title_rect = D2D_RECT_F {
            left: pad,
            top: y,
            right: w - pad,
            bottom: y + 26.0,
        };
        self.text_aligned(
            target,
            s.switch_account,
            &title_rect,
            16.0,
            400,
            self.theme.text_primary,
            alpha,
            Align::Center,
            false,
        );
        y += 30.0;

        let selected = model.account.map(|a| a.index);
        for (i, acc) in model.accounts.iter().enumerate() {
            if self.hover == Some(Hit::PickAccount(i)) {
                let row = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad - 6.0,
                        top: y + 4.0,
                        right: w - pad + 6.0,
                        bottom: y + 40.0,
                    },
                    radiusX: 6.0,
                    radiusY: 6.0,
                };
                let fill = self.brush(target, self.theme.track, alpha);
                target.FillRoundedRectangle(&row, &fill);
            }
            let cur = selected == Some(i);
            if cur {
                let bar = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad - 6.0,
                        top: y + 14.0,
                        right: pad - 3.0,
                        bottom: y + 30.0,
                    },
                    radiusX: 1.5,
                    radiusY: 1.5,
                };
                let b = self.brush(target, self.theme.accent, alpha);
                target.FillRoundedRectangle(&bar, &b);
            }
            // 右端徽标：平台最右，key 前缀徽标在其左；空 key 跳过。宽度先算后定名称可用区
            let platform = if acc.platform == crate::api::client::Platform::Cn {
                s.platform_cn
            } else {
                s.platform_intl
            };
            let platform_label = if acc.team {
                format!("{platform} · {}", s.team_badge)
            } else {
                platform.to_string()
            };
            let pw = self.measure(&platform_label, 10.5, 400, false) + 14.0;
            let px = w - pad - pw;
            self.badge(
                target,
                &platform_label,
                px,
                y + 13.5,
                pw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
                false,
            );
            // chars 截取规避多字节切片 panic；不足 4 位有多少显多少
            let prefix: String = acc.api_key.chars().take(4).collect();
            let mut name_right = px - 10.0;
            if !prefix.is_empty() {
                let key_label = format!("{prefix}…");
                let kw = self.measure(&key_label, 10.5, 400, true) + 14.0;
                let kx = px - 6.0 - kw;
                self.badge(
                    target,
                    &key_label,
                    kx,
                    y + 13.5,
                    kw,
                    self.theme.border,
                    self.theme.text_tertiary,
                    alpha,
                    true,
                );
                name_right = kx - 8.0;
            }
            let name_w = (name_right - (pad + 8.0)).max(60.0);
            let name = self.ellipsize(&acc.name, 14.0, name_w, 600, false);
            self.text(
                target,
                &name,
                pad + 8.0,
                y + 12.0,
                name_w,
                20.0,
                14.0,
                if cur { 600 } else { 500 },
                self.theme.text_primary,
                alpha,
            );
            self.hits.push((
                Hit::PickAccount(i),
                D2D_RECT_F {
                    left: pad - 6.0,
                    top: y,
                    right: w - pad + 6.0,
                    bottom: y + 44.0,
                },
            ));
            y += 44.0;
        }
    }
}
