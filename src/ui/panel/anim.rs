//! Animation system

/// Cubic ease-out: for the expanding direction — fast start, slow settle.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Cubic ease-in: for the collapsing direction — slow start, fast finish.
pub fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t.powi(3)
}

/// Time state of a single animation
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    pub start: std::time::Instant,
    pub duration_ms: u32,
}

impl Tween {
    pub fn now(duration_ms: u32) -> Self {
        Self {
            start: std::time::Instant::now(),
            duration_ms,
        }
    }

    pub fn progress(&self) -> f32 {
        let elapsed = self.start.elapsed().as_millis() as f32;
        (elapsed / self.duration_ms as f32).clamp(0.0, 1.0)
    }

    pub fn finished(&self) -> bool {
        self.progress() >= 1.0
    }
}

/// Whether the system's "reduced motion" setting is respected
pub fn animations_allowed() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SYSTEM_PARAMETERS_INFO_ACTION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    const SPI_GETCLIENTAREAANIMATION: u32 = 0x1042;
    let mut enabled = windows::core::BOOL::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SYSTEM_PARAMETERS_INFO_ACTION(SPI_GETCLIENTAREAANIMATION),
            0,
            Some(&mut enabled as *mut _ as *mut core::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    !(ok.is_ok() && !enabled.as_bool())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_bounds() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert_eq!(ease_in_cubic(1.0), 1.0);
        assert_eq!(ease_in_cubic(0.5), 0.125);
    }

    #[test]
    fn tween_progress_clamped() {
        let t = Tween::now(100);
        assert_eq!(t.progress(), 0.0);
    }

    /// Pins compose_anim_period: coexisting tiers take the earlier, a lone wake is capped at one minute, idle falls back to one frame, zero never disarms below 1ms.
    #[test]
    fn compose_anim_period_tiers() {
        use crate::ui::panel::compose_anim_period;
        // Frame tier alone paces the clock: 16ms full frame, 33ms marquee tier
        assert_eq!(compose_anim_period(Some(16), None), 16);
        assert_eq!(compose_anim_period(Some(33), None), 33);
        // Frame + precise wake coexist: the earlier one wins either way
        assert_eq!(compose_anim_period(Some(16), Some(30)), 16);
        assert_eq!(compose_anim_period(Some(16), Some(5)), 5);
        assert_eq!(compose_anim_period(Some(16), Some(16)), 16);
        // Lone wake: as-is below the ceiling, clamped to one minute beyond
        assert_eq!(compose_anim_period(None, Some(250)), 250);
        assert_eq!(compose_anim_period(None, Some(60_000)), 60_000);
        assert_eq!(compose_anim_period(None, Some(u32::MAX)), 60_000);
        // Idle: one frame
        assert_eq!(compose_anim_period(None, None), 16);
        // A composed 0 would read as "timer stopped": floored to 1ms
        assert_eq!(compose_anim_period(Some(0), None), 1);
        assert_eq!(compose_anim_period(None, Some(0)), 1);
        assert_eq!(compose_anim_period(Some(2), Some(0)), 1);
    }
}
