//! Tray icon

use std::ffi::c_void;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

/// Minimum icon edge length (px)
const MIN_PX: i32 = 16;

/// Physical-pixel size of the tray icon: twice the small-icon metric,
/// with the floor sharing the same source as MIN_PX.
/// The icon content key and the bitmap build share this value.
pub(crate) fn tray_px() -> i32 {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_CXSMICON,
        )
    }
    .max(MIN_PX)
        * 2
}

/// Loads an owned icon handle at the given size from the embedded resource,
/// taking the matching layer of assets/icon.ico;
/// the caller destroys it. Shared-handle loading and the resource-loading
/// idiom live in the single platform-side source (`load_resource_icon`).
pub fn resource_icon(px: i32) -> Option<HICON> {
    crate::platform::load_resource_icon(px, false)
}

/// Critical usage threshold (percent): shared by the tray icon's red tier and
/// the panel's red text and red bar
pub(crate) const CRITICAL_PCT: f64 = 90.0;

/// Usage tier colors
fn tier_color(used_percent: f64) -> (u8, u8, u8) {
    if used_percent < 70.0 {
        (0x34, 0xC7, 0x59) // systemGreen
    } else if used_percent < CRITICAL_PCT {
        (0xFF, 0x9F, 0x0A) // systemOrange
    } else {
        (0xFF, 0x45, 0x3A) // systemRed
    }
}

/// The three pieces of the Q outline (30×30 grid): outer ring, inner ring, and
/// tail slash; shared by the panel's D2D path and
/// the tray pixel icon. The geometry shares its source with assets/icon.svg;
/// keep both sides in sync when changing it
pub fn q_outline() -> [Vec<(f32, f32)>; 3] {
    let ring = |r: f32| -> Vec<(f32, f32)> {
        (0..24)
            .map(|i| {
                let a = (i as f32) * (std::f32::consts::TAU / 24.0);
                let (s, c) = a.sin_cos();
                (15.0 + r * c, 14.4 + r * s)
            })
            .collect()
    };
    [
        ring(9.2),
        ring(4.2),
        // Tail slanted rectangle: its start point is buried into the ring band so
        // the junction shows no seam.
        vec![
            (17.62, 19.84),
            (21.69, 23.91),
            (24.51, 21.09),
            (20.44, 17.02),
        ],
    ]
}

/// Default logo icon
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
    let [outer, inner, tail] = q_outline();
    let mp = |p: Vec<(f32, f32)>| -> Vec<(f32, f32)> {
        p.into_iter().map(|(x, y)| (m(x), m(y))).collect()
    };
    let (outer, inner, tail) = (mp(outer), mp(inner), mp(tail));

    for y in 0..px {
        for x in 0..px {
            // 4× supersampling takes coverage for edge antialiasing; 16 = the 4×4 sample count.
            let mut hit = 0u32;
            for sy in 0..4u32 {
                for sx in 0..4u32 {
                    let fx = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let fy = y as f32 + (sy as f32 + 0.5) / 4.0;
                    let in_q = (in_polygon(fx, fy, &outer) && !in_polygon(fx, fy, &inner))
                        || in_polygon(fx, fy, &tail);
                    if in_q || in_rounded_rect(fx, fy, ins, ins, side, rr) {
                        hit += 1;
                    }
                }
            }
            let a = (hit * 255 / 16) as u8;
            if a > 0 {
                // Color is decided once at the pixel center, so translucent edges do not blend colors;
                // RGB is premultiplied by coverage, matching the premultiplied convention of HICON composition.
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let in_q = (in_polygon(cx, cy, &outer) && !in_polygon(cx, cy, &inner))
                    || in_polygon(cx, cy, &tail);
                let v = if in_q { 0xFF } else { 0x2D };
                let cov = a as f32 / 255.0;
                let i = ((y * px + x) * 4) as usize;
                buf[i] = (v as f32 * cov) as u8;
                buf[i + 1] = (v as f32 * cov) as u8;
                buf[i + 2] = (v as f32 * cov) as u8;
                buf[i + 3] = a;
            }
        }
    }
    pixels_to_hicon(&buf, px)
}

/// Screen angle in degrees measured clockwise from 12 o'clock, in [0, 360):
/// the arc predicate compares against remain×360 directly, so a raw atan2
/// range would silently offset the arc's start (and thus its length).
fn ring_angle(fx: f32, fy: f32) -> f32 {
    let a = fy.atan2(fx).to_degrees() + 90.0;
    if a < 0.0 { a + 360.0 } else { a }
}

/// Arc membership: the remaining arc grows clockwise from 12 o'clock;
/// below 0.4% remaining counts as exhausted (gray track).
fn ring_on_arc(a: f32, remain: f32, failed: bool) -> bool {
    !failed && remain > 0.004 && a <= remain * 360.0
}

/// Ring remaining-quota icon
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
            let edge = (d - r_mid).abs() - stroke / 2.0;
            if edge > 0.5 {
                continue;
            }
            let cov = (0.5 - edge).clamp(0.0, 1.0);
            let a = ring_angle(fx, fy);
            let i = ((y * px + x) * 4) as usize;
            if ring_on_arc(a, remain, failed) {
                // Tier color (BGRA); RGB premultiplied by coverage,
                // matching the premultiplied convention of HICON composition.
                buf[i] = (tb as f32 * cov) as u8;
                buf[i + 1] = (tg as f32 * cov) as u8;
                buf[i + 2] = (tr as f32 * cov) as u8;
                buf[i + 3] = (0xFF as f32 * cov) as u8;
            } else {
                // Translucent dark-gray track, visible under both light and dark taskbars.
                buf[i] = (0x66 as f32 * cov) as u8;
                buf[i + 1] = (0x66 as f32 * cov) as u8;
                buf[i + 2] = (0x66 as f32 * cov) as u8;
                buf[i + 3] = (0x88 as f32 * cov) as u8;
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

/// Pixel buffer (BGRA) → HICON: the AND mask is all zeros; opacity is decided by alpha.
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
                // Mask creation failed: reclaim the already-built color bitmap, then give up.
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
                // Icon composition failed: ownership of both bitmaps is still here, so reclaim them together.
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

/// Destroys an HICON built by this process (`CreateIconIndirect` family)
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

    /// Ring arc orientation: 12 o'clock is 0°, clockwise positive, in
    /// [0, 360); the arc covers exactly remain×360 degrees from the top.
    #[test]
    fn ring_arc_orientation() {
        let close = |a: f32, b: f32| (a - b).abs() < 0.001;
        assert!(close(ring_angle(0.0, -1.0), 0.0), "正上=0°");
        assert!(close(ring_angle(1.0, 0.0), 90.0), "正右=90°");
        assert!(close(ring_angle(0.0, 1.0), 180.0), "正下=180°");
        assert!(close(ring_angle(-1.0, 0.0), 270.0), "正左=270°");

        // 25% remaining: exactly the 12→3 o'clock quarter lights up
        let remain = 0.25f32;
        assert!(ring_on_arc(0.0, remain, false), "起点亮");
        assert!(ring_on_arc(90.0, remain, false), "终点亮");
        assert!(!ring_on_arc(90.1, remain, false), "终点后不亮");
        assert!(!ring_on_arc(270.0, remain, false), "对侧不亮");
        // Exhausted and error states: no arc, the gray track draws instead
        assert!(!ring_on_arc(0.0, 0.0, false), "余 0 无弧");
        assert!(!ring_on_arc(0.0, 0.003, false), "低于 0.4% 视为耗尽");
        assert!(!ring_on_arc(0.0, 0.5, true), "失败态无弧");
        // Full ring
        assert!(ring_on_arc(359.9, 1.0, false));
    }
}
