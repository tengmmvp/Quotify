//! 动画系统

/// 三次缓出：展开方向用，快起慢收。
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// 三次缓入：收缩方向用，慢起快收。
pub fn ease_in_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t.powi(3)
}

/// 单条动画的时间状态
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

/// 是否尊重系统「减少动态效果」
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
}
