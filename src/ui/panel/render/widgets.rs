//! Cross-view shared widgets

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_F, D2D1_BEZIER_SEGMENT, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_CAP_STYLE_FLAT, D2D1_DASH_STYLE_DASH, D2D1_ELLIPSE,
    D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, ID2D1HwndRenderTarget, ID2D1PathGeometry,
};
use windows_numerics::{Matrix3x2, Vector2};

use super::{Hit, Renderer};

/// Badge height: even like the account-card height, so vertical centering lands on whole pixels
pub(super) const BADGE_H: f32 = 16.0;

/// Section-header row geometry shared across views — main's accent_label
/// and settings' section_label: inset from the accent bar and the row
/// height for vertical centering. Font size stays a caller decision
/// (12 main labels, 13 settings).
pub(super) const ACCENT_INSET: f32 = 7.0;
pub(super) const ACCENT_ROW_H: f32 = 17.0;

impl Renderer {
    /// Section-title accent bar: main's accent_label and settings'
    /// section_label share this geometry
    pub(super) unsafe fn fill_accent_bar(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        alpha: f32,
    ) {
        let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
        // Vertical centering of the bar in the row derives from
        // ACCENT_ROW_H — same source the label layout centers against.
        let top = y + (ACCENT_ROW_H - 13.0) / 2.0;
        target.FillRectangle(
            &D2D_RECT_F {
                left: x,
                top,
                right: x + 3.0,
                bottom: top + 13.0,
            },
            &bar,
        );
    }

    // The badge and outlined-button immediate-text versions were retired
    // with the frame caches: settings and popup draw prebuilt TextBlocks
    // now (settings-local badge/outline mirrors live in settings.rs).

    /// Icon button
    pub(super) unsafe fn icon_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        cx: f32,
        cy: f32,
        spin: f32,
    ) {
        const R: f32 = 16.0;
        let hovered = self.hover == Some(hit);
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: R,
            radiusY: R,
        };
        let base = if hovered {
            self.theme.track
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        if base[3] > 0.0 {
            let b = self.brush(target, base, 1.0);
            target.FillEllipse(&ellipse, &b);
        }
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        if self.refresh_geo.is_none() {
            self.refresh_geo = self.build_refresh_glyph();
        }
        if let Some(geo) = self.refresh_geo.clone() {
            let (s, c) = (-spin).sin_cos();
            target.SetTransform(&Matrix3x2 {
                M11: c,
                M12: s,
                M21: -s,
                M22: c,
                M31: cx,
                M32: cy,
            });
            target.DrawGeometry(&geo, &stroke, 1.6, None);
            target.SetTransform(&Matrix3x2::identity());
        }
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: cx - 15.0,
                top: cy - 15.0,
                right: cx + 15.0,
                bottom: cy + 15.0,
            },
        ));
    }

    /// The refresh button's two arc segments plus tail-arrow polylines
    fn build_refresh_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            let rr = 16.0 * 0.47;
            let add = |x0: f32, y0: f32, x1: f32, y1: f32| {
                sink.BeginFigure(Vector2 { X: x0, Y: y0 }, D2D1_FIGURE_BEGIN_HOLLOW);
                sink.AddLine(Vector2 { X: x1, Y: y1 });
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            };
            for (a0, a1) in [(-150f32, -20f32), (30f32, 160f32)] {
                let (r0, r1) = (a0.to_radians(), a1.to_radians());
                let steps = 12;
                for i in 0..steps {
                    let t0 = r0 + (r1 - r0) * i as f32 / steps as f32;
                    let t1 = r0 + (r1 - r0) * (i + 1) as f32 / steps as f32;
                    add(rr * t0.cos(), rr * t0.sin(), rr * t1.cos(), rr * t1.sin());
                }
                let (fx, fy) = (-r1.sin(), r1.cos());
                let (px, py) = (rr * r1.cos(), rr * r1.sin());
                let al = 4.0;
                let (fs, fc) = 150f32.to_radians().sin_cos();
                add(
                    px,
                    py,
                    px + (fx * fc - fy * fs) * al,
                    py + (fx * fs + fy * fc) * al,
                );
                add(
                    px,
                    py,
                    px + (fx * fc + fy * fs) * al,
                    py + (-fx * fs + fy * fc) * al,
                );
            }
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// Settings-entry sliders icon
    pub(super) unsafe fn sliders(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        cx: f32,
        cy: f32,
        r: f32,
    ) {
        let hovered = self.hover == Some(hit);
        let base = if hovered {
            self.theme.track
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };
        if base[3] > 0.0 {
            let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: r,
                radiusY: r,
            };
            let b = self.brush(target, base, 1.0);
            target.FillEllipse(&ellipse, &b);
        }
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let hole_c = if hovered {
            self.theme.track
        } else {
            let bg = self.theme.bg;
            [bg[0], bg[1], bg[2], 1.0]
        };
        let hole = self.brush(target, hole_c, 1.0);
        let half = r * 0.62;
        let dot_r = r * 0.16;
        let rows = [(-0.40f32, -0.20f32), (0.0, 0.22), (0.40, -0.12)];
        for (dy, dx) in rows {
            let ly = cy + dy * r;
            self.line(target, cx - half, ly, cx + half, ly, &stroke, 1.5);
            let (px, py) = (cx + dx * r, ly);
            let he = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
                point: Vector2 { X: px, Y: py },
                radiusX: dot_r,
                radiusY: dot_r,
            };
            target.FillEllipse(&he, &hole);
            target.DrawEllipse(&he, &stroke, 1.5, None);
        }
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: cx - 15.0,
                top: cy - 15.0,
                right: cx + 15.0,
                bottom: cy + 15.0,
            },
        ));
    }

    /// Back arrow
    pub(super) unsafe fn back_arrow(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
    ) {
        let stroke = self.brush(target, self.theme.text_secondary, 1.0);
        let (cx, cy) = (x + 8.0, y + 6.0);
        self.line(target, cx + 5.0, cy - 6.0, cx - 4.0, cy, &stroke, 1.8);
        self.line(target, cx - 4.0, cy, cx + 5.0, cy + 6.0, &stroke, 1.8);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 6.0,
                top: y - 6.0,
                right: x + 24.0,
                bottom: y + 20.0,
            },
        ));
    }

    /// X button: shared by account deletion and panel close
    pub(super) unsafe fn x_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
    ) {
        let stroke = self.brush(target, self.theme.text_tertiary, 1.0);
        self.line(target, x, y, x + 10.0, y + 10.0, &stroke, 1.4);
        self.line(target, x + 10.0, y, x, y + 10.0, &stroke, 1.4);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 6.0,
                top: y - 6.0,
                right: x + 16.0,
                bottom: y + 16.0,
            },
        ));
    }

    /// Divider line
    pub(super) unsafe fn divider(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        width: f32,
        alpha: f32,
    ) {
        let b = self.brush(target, self.theme.border, alpha * 0.7);
        self.line(target, x, y, x + width, y, &b, 1.0);
    }

    /// Receipt tear line
    pub(super) unsafe fn dashed_divider(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        width: f32,
        alpha: f32,
    ) {
        if self.dash_style.is_none() {
            self.dash_style = self
                .factory
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_FLAT,
                        endCap: D2D1_CAP_STYLE_FLAT,
                        dashCap: D2D1_CAP_STYLE_FLAT,
                        dashStyle: D2D1_DASH_STYLE_DASH,
                        ..Default::default()
                    },
                    None,
                )
                .ok();
        }
        let b = self.brush(target, self.theme.border, alpha * 0.8);
        match self.dash_style.clone() {
            Some(style) => target.DrawLine(
                Vector2 { X: x, Y: y },
                Vector2 { X: x + width, Y: y },
                &b,
                1.0,
                &style,
            ),
            None => self.line(target, x, y, x + width, y, &b, 1.0),
        }
    }

    /// Dropdown chevron
    /// Points down when collapsed, flips up when expanded.
    pub(super) unsafe fn chevron(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        cy: f32,
        color: [f32; 4],
        alpha: f32,
        up: bool,
    ) {
        let b = self.brush(target, color, alpha);
        let half = 5.0;
        let d = if up { -3.0 } else { 3.0 };
        self.line(target, x, cy - d, x + half, cy + d, &b, 1.8);
        self.line(target, x + half, cy + d, x + half * 2.0, cy - d, &b, 1.8);
    }

    /// Eye icon
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn eye(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        cx: f32,
        cy: f32,
        w: f32,
        revealed: bool,
        color: [f32; 4],
        alpha: f32,
    ) {
        if self.eye_geo.is_none() {
            self.eye_geo = self.build_eye_glyph();
        }
        let s = w / 14.0;
        let b = self.brush(target, color, alpha);
        if let Some(geo) = self.eye_geo.clone() {
            let m = Matrix3x2 {
                M11: s,
                M12: 0.0,
                M21: 0.0,
                M22: s,
                M31: cx - 7.0 * s,
                M32: cy - 5.0 * s,
            };
            target.SetTransform(&m);
            target.DrawGeometry(&geo, &b, 1.1 / s, None);
            target.SetTransform(&Matrix3x2::identity());
        }
        let pupil = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: 2.2 * s,
            radiusY: 2.2 * s,
        };
        target.FillEllipse(&pupil, &b);
        if revealed {
            self.line(
                target,
                cx - 6.5 * s,
                cy + 5.5 * s,
                cx + 6.5 * s,
                cy - 5.5 * s,
                &b,
                1.1,
            );
        }
    }

    /// App logo
    pub(super) unsafe fn logo(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        x: f32,
        y: f32,
        size: f32,
        alpha: f32,
    ) {
        let tile = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + size,
                bottom: y + size,
            },
            radiusX: size * (4.0 / 30.0),
            radiusY: size * (4.0 / 30.0),
        };
        let tb = self.brush(target, self.theme.logo_tile, alpha);
        target.FillRoundedRectangle(&tile, &tb);

        if self.logo_geo.is_none() {
            self.logo_geo = self.build_logo_glyph();
        }
        let Some((ring, tail)) = self.logo_geo.clone() else {
            return;
        };
        let qb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let m = Matrix3x2 {
            M11: size / 30.0,
            M12: 0.0,
            M21: 0.0,
            M22: size / 30.0,
            M31: x,
            M32: y,
        };
        target.SetTransform(&m);
        target.FillGeometry(&ring, &qb, None);
        target.FillGeometry(&tail, &qb, None);
        target.SetTransform(&Matrix3x2::identity());
    }

    /// Multiple closed polylines → one filled geometry; non-intersecting contours form a union
    fn build_polys(&self, polys: Vec<Vec<(f32, f32)>>) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            for poly in polys {
                let mut pts = poly.into_iter();
                let Some((x0, y0)) = pts.next() else {
                    continue;
                };
                sink.BeginFigure(Vector2 { X: x0, Y: y0 }, D2D1_FIGURE_BEGIN_FILLED);
                for (x, y) in pts {
                    sink.AddLine(Vector2 { X: x, Y: y });
                }
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// Build the white Q glyph path. The ring and the tail must be split into two
    /// geometries: two figures
    /// intersecting inside one geometry get hollowed out under the default evenodd rule;
    /// filling them separately yields the union
    fn build_logo_glyph(&self) -> Option<(ID2D1PathGeometry, ID2D1PathGeometry)> {
        let [outer, inner, tail] = crate::ui::icon::q_outline();
        let ring = self.build_polys(vec![outer, inner])?;
        let tail = self.build_polys(vec![tail])?;
        Some((ring, tail))
    }

    /// Build the bolt icon path
    pub(super) fn build_bolt_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(Vector2 { X: 4.5, Y: 0.0 }, D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(Vector2 { X: 0.5, Y: 7.5 });
            sink.AddLine(Vector2 { X: 3.2, Y: 7.5 });
            sink.AddLine(Vector2 { X: 2.5, Y: 13.0 });
            sink.AddLine(Vector2 { X: 6.5, Y: 5.0 });
            sink.AddLine(Vector2 { X: 3.8, Y: 5.0 });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// Build the eye's almond outline path
    fn build_eye_glyph(&self) -> Option<ID2D1PathGeometry> {
        unsafe {
            let geo = self.factory.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(Vector2 { X: 0.0, Y: 5.0 }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: Vector2 { X: 3.5, Y: 0.9 },
                point2: Vector2 { X: 10.5, Y: 0.9 },
                point3: Vector2 { X: 14.0, Y: 5.0 },
            });
            sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: Vector2 { X: 10.5, Y: 9.1 },
                point2: Vector2 { X: 3.5, Y: 9.1 },
                point3: Vector2 { X: 0.0, Y: 5.0 },
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close().ok()?;
            Some(geo)
        }
    }

    /// The Pac-Man sprite: cx/cy is the center, p the animation progress
    /// 0..1, r the drawn radius (the cached geometry is PACMAN_R and
    /// scales by r/PACMAN_R), color the fill/stroke brush. The mouth
    /// faces right, chomping as it walks; the open/close is built from
    /// the upper and lower half-discs counter-rotating around the center
    /// transform matrix. Shared by the footer swap and the about-window
    /// easter egg.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn pacman_at(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        cx: f32,
        cy: f32,
        r: f32,
        p: f32,
        color: [f32; 4],
        alpha: f32,
    ) {
        if self.pacman_geo.is_none() {
            self.pacman_geo = self.build_pacman_geo();
        }
        let Some((upper, lower)) = self.pacman_geo.clone() else {
            return;
        };
        let mouth = pacman_mouth(p);
        let b = self.brush(target, color, alpha);
        let k = r / PACMAN_R;
        for (geo, ang) in [(upper, -mouth), (lower, mouth)] {
            let (s, c) = ang.sin_cos();
            target.SetTransform(&Matrix3x2 {
                M11: k * c,
                M12: k * s,
                M21: -k * s,
                M22: k * c,
                M31: cx,
                M32: cy,
            });
            target.FillGeometry(&geo, &b, None);
            target.DrawGeometry(&geo, &b, 1.0, None);
        }
        target.SetTransform(&Matrix3x2::identity());
    }

    /// Build the Pac-Man's upper and lower half-disc geometries (radius PACMAN_R,
    /// local origin at the center).
    /// Each half-disc's diameter edge passes through the center, so
    /// rotating around the center opens and closes the mouth
    pub(super) fn build_pacman_geo(&self) -> Option<(ID2D1PathGeometry, ID2D1PathGeometry)> {
        unsafe {
            let build_half = |clockwise: bool| -> Option<ID2D1PathGeometry> {
                let geo = self.factory.CreatePathGeometry().ok()?;
                let sink = geo.Open().ok()?;
                sink.BeginFigure(
                    Vector2 {
                        X: -PACMAN_R,
                        Y: 0.0,
                    },
                    D2D1_FIGURE_BEGIN_FILLED,
                );
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: Vector2 {
                        X: PACMAN_R,
                        Y: 0.0,
                    },
                    size: D2D_SIZE_F {
                        width: PACMAN_R,
                        height: PACMAN_R,
                    },
                    rotationAngle: 0.0,
                    sweepDirection: if clockwise {
                        D2D1_SWEEP_DIRECTION_CLOCKWISE
                    } else {
                        D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
                    },
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
                sink.AddLine(Vector2 { X: 0.0, Y: 0.0 });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close().ok()?;
                Some(geo)
            };
            // Screen coordinates have y pointing down: the upper half-disc takes the
            // clockwise small arc (through the top); the lower is symmetric.
            let upper = build_half(true)?;
            let lower = build_half(false)?;
            Some((upper, lower))
        }
    }
}

/// Pac-Man radius
pub(super) const PACMAN_R: f32 = 8.0;

/// Pac-Man mouth opening angle: the footer swap and the about-window easter egg share
/// this one animation curve
pub(super) fn pacman_mouth(p: f32) -> f32 {
    let sm = |t: f32| {
        let c = t.clamp(0.0, 1.0);
        c * c * (3.0 - 2.0 * c)
    };
    let phase = (p * 8.0).fract();
    let ratio = if phase < 0.65 {
        sm(phase / 0.65)
    } else {
        1.0 - sm((phase - 0.65) / 0.35)
    };
    0.02 + ratio * 0.94
}
