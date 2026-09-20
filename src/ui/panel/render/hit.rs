//! Hit identifier: clickable-element enum plus its exhaustive registries

use super::{AppearanceChoice, LanguageChoice, ScopeChoice};

/// Clickable element identifier used for hit testing; grouped by UI scene,
/// mirroring the match-arm grouping of handle_panel_hit
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    // ── Main view ──
    Refresh,
    Settings,
    AccountSwitch,
    UsageInfo,

    // ── Navigation ──
    Back,
    ClosePanel,

    // ── Settings · Poll interval ──
    IntervalPreset(u64),
    CustomizeInterval,
    ApplyInterval,
    InputInterval,

    // ── Settings · General ──
    Language(LanguageChoice),
    Appearance(AppearanceChoice),
    ToggleAutostart,

    // ── Settings · Network proxy ──
    InputProxy,
    ApplyProxy,

    // ── Settings · Usage notifications ──
    ToggleThreshold,
    ToggleReset5h,
    ToggleResetWeekly,

    // ── Settings · Peak hours ──
    InputPeakStart,
    InputPeakEnd,
    ApplyPeak,

    // ── Settings · Accounts ──
    AddAccount,
    RemoveAccount(usize),
    PickAccount(usize),
    AccountType(ScopeChoice),
    InputName,
    InputKey,
    RevealKey,
    InputOrg,
    InputProject,
    SaveAccount,
    Platform(crate::api::Platform),

    // ── Settings · Config management & about ──
    ExportConfig,
    ImportConfig,
    CheckUpdate,
    OpenDownload,

    // ── About window ──
    LinkRepo,
    LinkIssues,
    CopyDiagnostics,
    NewsItem(usize),
    AboutLogo,
}

/// Exhaustive registry of non-slot hits that keep the input state after
/// a click (apply buttons keep the field for editing when validation
/// fails, same semantics as the Enter path): every Hit variant is listed,
/// so a new variant that must keep the input state fails to compile here
/// instead of silently clearing it (caret gone, in-flight IME
/// composition dropped).
pub(crate) const fn keeps_input_state_hit(h: Hit) -> bool {
    match h {
        Hit::CustomizeInterval
        | Hit::AddAccount
        | Hit::RevealKey
        | Hit::ApplyProxy
        | Hit::ApplyInterval
        | Hit::ApplyPeak
        | Hit::SaveAccount => true,
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::InputInterval
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::InputProxy
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::InputPeakStart
        | Hit::InputPeakEnd
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::InputName
        | Hit::InputKey
        | Hit::InputOrg
        | Hit::InputProject
        | Hit::Platform(_)
        | Hit::ExportConfig
        | Hit::ImportConfig
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => false,
    }
}

/// Exhaustive modal registry: every Hit variant is listed, so a new
/// modal variant fails to compile here until registered — and this
/// match's true arm and crate::app::dispatch_modal's dispatch arm must
/// be extended in the same change (a dispatch arm missed after
/// registration is still caught by dispatch_modal's fallback log).
pub(crate) const fn is_modal_hit(h: Hit) -> bool {
    match h {
        Hit::ExportConfig | Hit::ImportConfig => true,
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::CustomizeInterval
        | Hit::ApplyInterval
        | Hit::InputInterval
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::InputProxy
        | Hit::ApplyProxy
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::InputPeakStart
        | Hit::InputPeakEnd
        | Hit::ApplyPeak
        | Hit::AddAccount
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::InputName
        | Hit::InputKey
        | Hit::RevealKey
        | Hit::InputOrg
        | Hit::InputProject
        | Hit::SaveAccount
        | Hit::Platform(_)
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => false,
    }
}

/// Hit predicates are kept next to the enum; new input-field variants must be registered
/// here in sync
impl Hit {
    /// Hits that enter/keep the input state or the key plain-text view state after a click;
    /// WM_LBUTTONUP uses this to decide whether to end the input state after the click.
    /// Input-field slots are decided exhaustively via input_field_of_hit;
    /// the non-slot additions come from the exhaustive keeps_input_state_hit.
    pub(crate) fn is_input_hit(&self) -> bool {
        crate::ui::panel::input_field_of_hit(*self).is_some() || keeps_input_state_hit(*self)
    }

    /// Modal hits: dispatch must go through the window-procedure-layer interception into
    /// the direct two-stage call;
    /// reaching handle_panel_hit is a wiring error. The registry is the
    /// exhaustive is_modal_hit; when adding a modal variant, register it in
    /// crate::app::dispatch_modal in sync.
    pub(crate) fn is_modal(&self) -> bool {
        is_modal_hit(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the manual allowlist of is_input_hit: a new input-adjacent Hit
    /// variant that must keep focus (an apply-button pattern) compiles
    /// silently unregistered otherwise — no exhaustiveness check covers this
    /// list. When adding a variant, extend both the enum and this table.
    #[test]
    fn is_input_hit_allowlist_pinned() {
        let yes: &[Hit] = &[
            Hit::InputInterval,
            Hit::InputProxy,
            Hit::InputPeakStart,
            Hit::InputPeakEnd,
            Hit::InputName,
            Hit::InputKey,
            Hit::InputOrg,
            Hit::InputProject,
            Hit::CustomizeInterval,
            Hit::AddAccount,
            Hit::RevealKey,
            Hit::ApplyProxy,
            Hit::ApplyInterval,
            Hit::ApplyPeak,
            Hit::SaveAccount,
        ];
        let no: &[Hit] = &[
            Hit::Refresh,
            Hit::Settings,
            Hit::AccountSwitch,
            Hit::UsageInfo,
            Hit::Back,
            Hit::ClosePanel,
            Hit::IntervalPreset(300),
            Hit::Language(LanguageChoice::System),
            Hit::Appearance(AppearanceChoice::System),
            Hit::ToggleAutostart,
            Hit::ToggleThreshold,
            Hit::ToggleReset5h,
            Hit::ToggleResetWeekly,
            Hit::RemoveAccount(0),
            Hit::PickAccount(0),
            Hit::AccountType(ScopeChoice::Personal),
            Hit::Platform(crate::api::Platform::Cn),
            Hit::ExportConfig,
            Hit::ImportConfig,
            Hit::CheckUpdate,
            Hit::OpenDownload,
            Hit::LinkRepo,
            Hit::LinkIssues,
            Hit::CopyDiagnostics,
            Hit::NewsItem(0),
            Hit::AboutLogo,
        ];
        for h in yes {
            assert!(h.is_input_hit(), "should keep input state: {h:?}");
        }
        for h in no {
            assert!(!h.is_input_hit(), "should drop input state: {h:?}");
        }
    }
}
