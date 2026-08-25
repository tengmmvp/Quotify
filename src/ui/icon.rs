//! 托盘图标：纯软件光栅化（直写 BGRA，不依赖 GDI+ 的内存布局语义）。
//!
//! 无数据时为默认 logo（深灰圆角方块 + 白 Z）；有数据时为环形余量
//! 图标：填充长度 = 5 小时窗口余量，颜色按档位绿 → 橙 → 红。

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, ICONINFO, CreateIconIndirect, HICON};

/// 图标最小边长（px）：防御调用侧传 0/负值导致像素缓冲区退化。
const MIN_PX: i32 = 16;

/// 加载嵌入资源中的应用图标（资源 id 1，logo 同源）。
pub fn app_icon(hinst: windows::Win32::Foundation::HINSTANCE) -> Option<HICON> {
    // MAKEINTRESOURCEW(1)：整数资源 id 直接编码进指针值，须精确为 1；
    // clippy 建议的 dangling() 按对齐取地址（u16 → 2），会错读资源 id 2，
    // 故此处保留整型转指针并豁免该 lint
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

/// 用量档位颜色（苹果系统色）。
fn tier_color(used_percent: f64) -> (u8, u8, u8) {
    if used_percent < 70.0 {
        (0x34, 0xC7, 0x59) // systemGreen
    } else if used_percent < 90.0 {
        (0xFF, 0x9F, 0x0A) // systemOrange
    } else {
        (0xFF, 0x45, 0x3A) // systemRed
    }
}

/// 默认图标（无数据时）：深灰圆角方块 + 白色 Z 字形（与 assets/logo.svg 同源）。
pub fn logo_icon(px: i32) -> Option<HICON> {
    let px = px.max(MIN_PX);
    let mut buf = vec![0u8; (px * px * 4) as usize];
    let s = px as f32;
    let ins = s * 0.01;
    let side = s - 2.0 * ins;
    let rr = (side * 4.0 / 30.0).max(1.0);

    // Z 字形三块（30 单位坐标 → 像素）
    let scale = 0.98 * s / 30.0;
    let bias = 0.01 * s;
    let m = |v: f32| v * scale + bias;
    let polys: [Vec<(f32, f32)>; 3] = [
        vec![(15.47, 7.1), (14.17, 8.95), (13.27, 9.42), (6.17, 9.42), (6.17, 7.09)],
        vec![(24.3, 7.1), (13.14, 22.91), (5.7, 22.91), (16.86, 7.1)],
        vec![(14.53, 22.91), (15.84, 21.05), (16.74, 20.58), (23.83, 20.58), (23.83, 22.91)],
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
                // 中性深灰 #2D2D2D（logo.svg 原版色）
                buf[i] = 0x2D;
                buf[i + 1] = 0x2D;
                buf[i + 2] = 0x2D;
                buf[i + 3] = 0xFF;
            }
        }
    }
    pixels_to_hicon(&buf, px)
}

/// 环形余量图标。`failed` 时仅画灰色轨道环（`used_percent` 不参与着色）。
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

/// 点是否在圆角矩形内。
fn in_rounded_rect(x: f32, y: f32, rx: f32, ry: f32, side: f32, r: f32) -> bool {
    let (right, bottom) = (rx + side, ry + side);
    if x < rx || x > right || y < ry || y > bottom {
        return false;
    }
    // 距四角的圆角检查
    let corners = [(rx + r, ry + r), (right - r, ry + r), (rx + r, bottom - r), (right - r, bottom - r)];
    for &(ccx, ccy) in &corners {
        let (dx, dy) = (x - ccx, y - ccy);
        let in_corner_zone = match (ccx, ccy) {
            (a, b) if a < x && b < y => (x > right - r) && (y > bottom - r),
            (a, b) if a > x && b < y => (x < rx + r) && (y > bottom - r),
            (a, b) if a < x && b > y => (x > right - r) && (y < ry + r),
            _ => (x < rx + r) && (y < ry + r),
        };
        if in_corner_zone && dx * dx + dy * dy > r * r {
            return false;
        }
    }
    true
}

/// 点是否在多边形内（射线法）。
fn in_polygon(x: f32, y: f32, pts: &[(f32, f32)]) -> bool {
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

/// 像素缓冲（BGRA）→ HICON（单色 AND 掩码全 0，透明度由 alpha 决定）。
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
            &BITMAPINFO { bmiHeader: hdr(32), ..Default::default() },
            DIB_RGB_COLORS,
            (&mut bits) as *mut *mut c_void,
            None,
            0,
        )
        .ok()?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());

        let mut mask_bits: *mut c_void = std::ptr::null_mut();
        let mask = windows::Win32::Graphics::Gdi::CreateDIBSection(
            None,
            &BITMAPINFO { bmiHeader: hdr(1), ..Default::default() },
            DIB_RGB_COLORS,
            (&mut mask_bits) as *mut *mut c_void,
            None,
            0,
        )
        .ok()?;
        if !mask_bits.is_null() {
            std::ptr::write_bytes(mask_bits as *mut u8, 0, (px as usize * px as usize).div_ceil(8));
        }

        let info = ICONINFO {
            fIcon: windows::core::BOOL::from(true),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let hicon = CreateIconIndirect(&info).ok()?;
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        Some(hicon)
    }
}

/// 释放 HICON。
pub fn destroy_icon(icon: HICON) {
    unsafe { DestroyIcon(icon) }.ok();
}
