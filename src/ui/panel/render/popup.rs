//! 账号切换弹窗视图

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::{Align, Hit, Renderer};
use crate::ui::panel::anim::ease_out_cubic;
use crate::ui::panel::model::PanelModel;

/// 弹窗逻辑宽
pub const POPUP_W: f32 = 220.0;
/// 账号行高
pub const ROW_H: f32 = 36.0;

/// 弹窗逻辑高：标题区 + 账号行 + 上下留白
pub fn popup_height(accounts: usize) -> i32 {
    (36.0 + accounts as f32 * ROW_H + 12.0).round() as i32
}

impl Renderer {
    /// 弹窗一帧；骨架同面板 paint：ensure_target + BeginDraw/EndDraw，
    /// 返回 false 表示设备已丢失，调用方须丢弃整个 Renderer
    pub fn paint_popup(
        &mut self,
        hwnd: windows::Win32::Foundation::HWND,
        rect_phys: &RECT,
        model: &PanelModel,
        dpi: f32,
    ) -> bool {
        unsafe {
            let Some(target) = self.ensure_target(hwnd, rect_phys, dpi) else {
                return true;
            };
            let w = (rect_phys.right - rect_phys.left) as f32 / dpi;
            let h = (rect_phys.bottom - rect_phys.top) as f32 / dpi;
            self.hits.clear();
            self.frame_measures.clear();
            target.BeginDraw();
            self.draw_account_popup(&target, model, w, h);
            match target.EndDraw(None, None) {
                Ok(()) => true,
                Err(e) => {
                    crate::platform::log(&format!("[Quotify] 弹窗 EndDraw 失败: {e}"));
                    false
                }
            }
        }
    }

    /// 账号列表：弱化标题 + 行；当前项左竖条，hover 整行淡填充
    unsafe fn draw_account_popup(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        w: f32,
        h: f32,
    ) {
        let s = model.strings;
        let pad = 10.0;

        // 弹出动画：内容上浮渐入；背景必须不透明全幅绘制，否则首帧露交换链
        let (dy, alpha) = match &self.anim.appear {
            Some(t) if self.anim_allowed => {
                let p = ease_out_cubic(t.progress());
                ((1.0 - p) * 6.0, p)
            }
            _ => (0.0, 1.0),
        };
        let bg = self.brush(target, self.theme.bg, 1.0);
        target.FillRectangle(
            &D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: w,
                bottom: h,
            },
            &bg,
        );

        let title_rect = D2D_RECT_F {
            left: 0.0,
            top: dy + 13.0,
            right: w,
            bottom: dy + 29.0,
        };
        self.text_aligned(
            target,
            s.switch_account,
            &title_rect,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
            Align::Center,
            false,
        );

        let selected = model.account.map(|a| a.index);
        let mut y = dy + 36.0;
        for (i, acc) in model.accounts.iter().enumerate() {
            if self.hover == Some(Hit::PickAccount(i)) {
                let row = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: pad - 4.0,
                        top: y + 2.0,
                        right: w - pad + 4.0,
                        bottom: y + ROW_H - 2.0,
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
                        left: pad - 4.0,
                        top: y + 12.0,
                        right: pad - 1.0,
                        bottom: y + 24.0,
                    },
                    radiusX: 1.5,
                    radiusY: 1.5,
                };
                let b = self.brush(target, self.theme.accent, alpha);
                target.FillRoundedRectangle(&bar, &b);
            }
            // 右端徽标：平台，团队版追加团队标；弹窗窄，放不下 key 前缀徽标
            let platform = if acc.platform == crate::api::Platform::Cn {
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
                y + 12.5,
                pw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
                false,
            );
            let name_right = px - 8.0;
            let name_w = (name_right - (pad + 6.0)).max(56.0);
            let (name, _) = self.ellipsize(&acc.name, 13.5, name_w, 600, false);
            self.text(
                target,
                &name,
                pad + 6.0,
                y + 9.0,
                name_w,
                18.0,
                13.5,
                if cur { 600 } else { 500 },
                self.theme.text_primary,
                alpha,
            );
            self.hits.push((
                Hit::PickAccount(i),
                D2D_RECT_F {
                    left: pad - 4.0,
                    top: y,
                    right: w - pad + 4.0,
                    bottom: y + ROW_H,
                },
            ));
            y += ROW_H;
        }
    }
}
