//! 托盘图标

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

/// 图标最小边长（px）
const MIN_PX: i32 = 16;

/// 加载嵌入资源中的应用图标
pub fn app_icon(hinst: windows::Win32::Foundation::HINSTANCE) -> Option<HICON> {
    #[allow(clippy::manual_dangling_ptr)]
    let resid = 1usize as *const u16;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::LoadImageW(
            Some(hinst),
            windows::core::PCWSTR(resid),
            windows::Win32::UI::WindowsAndMessaging::IMAGE_ICON,
            0,
            0,
            windows::Win32::UI::WindowsAndMessaging::LR_DEFAULTSIZE
                | windows::Win32::UI::WindowsAndMessaging::LR_SHARED,
        )
        .ok()
        .map(|h| HICON(h.0))
    }
}

/// 用量档位颜色
fn tier_color(used_percent: f64) -> (u8, u8, u8) {
    if used_percent < 70.0 {
        (0x34, 0xC7, 0x59) // systemGreen
    } else if used_percent < 90.0 {
        (0xFF, 0x9F, 0x0A) // systemOrange
    } else {
        (0xFF, 0x45, 0x3A) // systemRed
    }
}

/// 默认 logo 图标
pub fn logo_icon(px: i32) -> Option<HICON> {
    let px = px.max(MIN_PX);
    let mut buf = vec![0u8; (px * px * 4) as usize];
    let s = px as f32;
    let ins = s * 0.01;
    let side = s - 2.0 * ins;
    let rr = (side * 4.0 / 30.0).max(1.0);

    let scale = 0.98 * s / 30.0;
    let bias = 0.01 * s;
    let m = |v: f32| v * scale + bias;
    let polys: [Vec<(f32, f32)>; 3] = [
        vec![
            (15.47, 7.1),
            (14.17, 8.95),
            (13.27, 9.42),
            (6.17, 9.42),
            (6.17, 7.09),
        ],
        vec![(24.3, 7.1), (13.14, 22.91), (5.7, 22.91), (16.86, 7.1)],
        vec![
            (14.53, 22.91),
            (15.84, 21.05),
            (16.74, 20.58),
            (23.83, 20.58),
            (23.83, 22.91),
        ],
    ];
    let polys: Vec<Vec<(f32, f32)>> = polys
        .into_iter()
        .map(|p| p.into_iter().map(|(x, y)| (m(x), m(y))).collect())
        .collect();

    for y in 0..px {
        for x in 0..px {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = ((y * px + x) * 4) as usize;
            let in_z = polys.iter().any(|p| in_polygon(fx, fy, p));
            if in_z {
                buf[i] = 0xFF;
                buf[i + 1] = 0xFF;
                buf[i + 2] = 0xFF;
                buf[i + 3] = 0xFF;
            } else if in_rounded_rect(fx, fy, ins, ins, side, rr) {
                buf[i] = 0x2D;
                buf[i + 1] = 0x2D;
                buf[i + 2] = 0x2D;
                buf[i + 3] = 0xFF;
            }
        }
    }
    pixels_to_hicon(&buf, px)
}

/// 环形余量图标
pub fn ring_icon(px: i32, used_percent: f64, failed: bool) -> Option<HICON> {
    let px = px.max(MIN_PX);
    let mut buf = vec![0u8; (px * px * 4) as usize];
    let s = px as f32;
    let stroke = (s * 0.22).clamp(2.5, 7.0);
    let r_mid = (s - stroke) / 2.0 - 0.5;
    let (cx, cy) = ((s - 1.0) / 2.0, (s - 1.0) / 2.0);
    let (tr, tg, tb) = tier_color(used_percent);
    let remain = (1.0 - used_percent.clamp(0.0, 100.0) / 100.0) as f32;

    for y in 0..px {
        for x in 0..px {
            let (fx, fy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let d = (fx * fx + fy * fy).sqrt();
            if (d - r_mid).abs() > stroke / 2.0 {
                continue;
            }
            // 屏幕角度：12 点方向为 -90°，顺时针增加
            let mut a = fy.atan2(fx).to_degrees();
            if a < -90.0 {
                a += 360.0;
            }
            let i = ((y * px + x) * 4) as usize;
            // 余量不足 0.4% 视为耗尽，画灰轨道
            let on_arc = !failed && remain > 0.004 && a <= remain * 360.0;
            if on_arc {
                // 档位色（BGRA）
                buf[i] = tb;
                buf[i + 1] = tg;
                buf[i + 2] = tr;
                buf[i + 3] = 0xFF;
            } else {
                // 半透明深灰轨道，深浅任务栏下均可见
                buf[i] = 0x66;
                buf[i + 1] = 0x66;
                buf[i + 2] = 0x66;
                buf[i + 3] = 0x88;
            }
        }
    }
    pixels_to_hicon(&buf, px)
}

fn in_rounded_rect(x: f32, y: f32, rx: f32, ry: f32, side: f32, r: f32) -> bool {
    let (right, bottom) = (rx + side, ry + side);
    if x < rx || x > right || y < ry || y > bottom {
        return false;
    }
    let cx = if x < rx + r {
        rx + r
    } else if x > right - r {
        right - r
    } else {
        x
    };
    let cy = if y < ry + r {
        ry + r
    } else if y > bottom - r {
        bottom - r
    } else {
        y
    };
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= r * r
}

fn in_polygon(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
    if pts.is_empty() {
        return false;
    }
    let mut inside = false;
    let n = pts.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = pts[i];
        let (xj, yj) = pts[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 像素缓冲（BGRA）→ HICON：AND 掩码全 0，透明度由 alpha 决定。
fn pixels_to_hicon(pixels: &[u8], px: i32) -> Option<HICON> {
    let hdr = |bpp: u16| BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: px,
        biHeight: -px,
        biPlanes: 1,
        biBitCount: bpp,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    unsafe {
        let mut bits: *mut c_void = std::ptr::null_mut();
        let color = windows::Win32::Graphics::Gdi::CreateDIBSection(
            None,
            &BITMAPINFO {
                bmiHeader: hdr(32),
                ..Default::default()
            },
            DIB_RGB_COLORS,
            (&mut bits) as *mut *mut c_void,
            None,
            0,
        )
        .ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());

        let mut mask_bits: *mut c_void = std::ptr::null_mut();
        let mask = match windows::Win32::Graphics::Gdi::CreateDIBSection(
            None,
            &BITMAPINFO {
                bmiHeader: hdr(1),
                ..Default::default()
            },
            DIB_RGB_COLORS,
            (&mut mask_bits) as *mut *mut c_void,
            None,
            0,
        ) {
            Ok(m) => m,
            Err(_) => {
                // 掩码创建失败：回收已建的颜色位图再放弃
                let _ = DeleteObject(color.into());
                return None;
            }
        };
        if !mask_bits.is_null() {
            std::ptr::write_bytes(
                mask_bits as *mut u8,
                0,
                (px as usize * px as usize).div_ceil(8),
            );
        }

        let info = ICONINFO {
            fIcon: windows::core::BOOL::from(true),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let hicon = match CreateIconIndirect(&info) {
            Ok(h) => h,
            Err(_) => {
                // 图标合成失败：两张位图所有权仍在自己手里，一并回收
                let _ = DeleteObject(color.into());
                let _ = DeleteObject(mask.into());
                return None;
            }
        };
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        Some(hicon)
    }
}

/// 释放本进程自建（`CreateIconIndirect` 系）的 HICON
pub fn destroy_owned(h: HICON) {
    unsafe { DestroyIcon(h) }.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_hit_test() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(in_polygon(5.0, 5.0, &square));
        assert!(!in_polygon(15.0, 5.0, &square));
        assert!(!in_polygon(-1.0, -1.0, &square));
        assert!(!in_polygon(0.0, 0.0, &[]));
        let notch = [
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (6.0, 5.0),
            (4.0, 5.0),
            (0.0, 10.0),
        ];
        assert!(in_polygon(2.0, 3.0, &notch), "凹口左侧的主体内部");
        assert!(!in_polygon(5.0, 6.0, &notch), "凹口内");
    }

    #[test]
    fn rounded_rect_hit_test() {
        assert!(in_rounded_rect(10.0, 10.0, 0.0, 0.0, 20.0, 5.0), "中心");
        assert!(
            in_rounded_rect(0.5, 10.0, 0.0, 0.0, 20.0, 5.0),
            "边中段贴边仍算"
        );
        assert!(
            !in_rounded_rect(-0.5, 10.0, 0.0, 0.0, 20.0, 5.0),
            "左侧出界"
        );
        assert!(
            !in_rounded_rect(20.5, 10.0, 0.0, 0.0, 20.0, 5.0),
            "右侧出界"
        );
        assert!(
            !in_rounded_rect(0.5, 0.5, 0.0, 0.0, 20.0, 5.0),
            "圆弧外的角点"
        );
        assert!(
            in_rounded_rect(1.5, 1.5, 0.0, 0.0, 20.0, 5.0),
            "圆弧内近角点"
        );
    }

    #[test]
    fn tier_thresholds() {
        assert_eq!(tier_color(0.0), (0x34, 0xC7, 0x59));
        assert_eq!(tier_color(69.9), (0x34, 0xC7, 0x59));
        assert_eq!(tier_color(70.0), (0xFF, 0x9F, 0x0A), "70 起进橙档");
        assert_eq!(tier_color(89.9), (0xFF, 0x9F, 0x0A));
        assert_eq!(tier_color(90.0), (0xFF, 0x45, 0x3A), "90 起进红档");
        assert_eq!(tier_color(100.0), (0xFF, 0x45, 0x3A));
    }
}
