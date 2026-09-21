//! Self-drawn input fields: the field enum, per-field buffers, byte caps,
//! key masking, and hit→field mapping — the input state machine shared by
//! the panel's edit paths.

use crate::api::client::PROXY_MAX_BYTES;

/// Byte cap for a single-field input buffer
const INPUT_MAX_BYTES: usize = 128;

/// Target field of self-drawn input; settings-page fields first, add-page form fields
/// after, each ordered by its section
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputField {
    /// Settings page: custom polling interval
    Interval,
    /// Settings page: network proxy address
    Proxy,
    /// Settings page: peak window start
    PeakStart,
    /// Settings page: peak window end
    PeakEnd,
    /// Add page: account name
    Name,
    /// Add page: API key
    Key,
    /// Add page (team edition): organization ID
    Org,
    /// Add page (team edition): project ID
    Project,
}

impl InputField {
    /// Byte cap of the input buffer: proxy addresses may carry userinfo
    /// credentials, so they get a looser tier (the api-side validation
    /// gate's own constant).
    /// The match is exhaustive with no fallback, same rule as the other
    /// InputField dispatches: a new variant is a compile error
    pub(crate) fn max_bytes(self) -> usize {
        match self {
            InputField::Proxy => PROXY_MAX_BYTES,
            InputField::Interval
            | InputField::PeakStart
            | InputField::PeakEnd
            | InputField::Name
            | InputField::Key
            | InputField::Org
            | InputField::Project => INPUT_MAX_BYTES,
        }
    }

    /// Byte cap tier with the no-active-field fold: callers holding
    /// Option<InputField> (paste budget, char input, IME commit) all read
    /// the tier here, so the None tier has one owner instead of a
    /// map_or fallback restated per call site.
    pub(crate) fn max_bytes_for(field: Option<Self>) -> usize {
        field.map_or(INPUT_MAX_BYTES, Self::max_bytes)
    }

    /// Prefill copy clamped to the byte cap: the config side can hold more
    /// bytes than typing allows (import bypasses input-side validation), and
    /// a verbatim prefill would leave a buffer no keystroke can extend.
    /// Truncates on a char boundary — the truncation policy single-sources
    /// in platform::truncate_bytes (within-cap input passes through
    /// unchanged, same semantics as the inlined code this replaced).
    pub(crate) fn clamp_prefill(self, s: &str) -> String {
        crate::platform::truncate_bytes(s, self.max_bytes()).to_string()
    }

    /// Slot index into the per-field caret width cache; exhaustive like the
    /// other InputField dispatches so a new variant is a compile error here
    /// (the slot-count pinned test then flags the array size).
    pub(crate) fn slot(self) -> usize {
        match self {
            InputField::Interval => 0,
            InputField::Proxy => 1,
            InputField::PeakStart => 2,
            InputField::PeakEnd => 3,
            InputField::Name => 4,
            InputField::Key => 5,
            InputField::Org => 6,
            InputField::Project => 7,
        }
    }
}

/// Key masking display string: focused or under 12 chars shows per-character
/// dots matching the original length, so caret stepping stays correct and
/// short strings leak no plain text; unfocused with 12+ chars reveals only
/// the first and last 4. Shared by the renderer (what is drawn) and
/// field_display (what the caret width table measures) — one rule, one place.
/// The masking glyph is a named constant: caret.rs's cache-hit predicate
/// recognizes the masked form structurally (length + glyph) without
/// materializing it, and reads the same constant so the two can never
/// drift.
pub(crate) const MASK_GLYPH: char = '•';

pub(crate) fn mask_key(key: &str, active: bool) -> String {
    let n = key.chars().count();
    if active || n < 12 {
        MASK_GLYPH.to_string().repeat(n)
    } else {
        let head: String = key.chars().take(4).collect();
        let tail: String = key.chars().skip(n - 4).collect();
        format!("{head}…{tail}")
    }
}

/// Self-drawn input buffers; field order same as InputField
#[derive(Default)]
pub struct PanelInput {
    pub field: Option<InputField>,
    pub(crate) edit: crate::ui::panel::text_edit::EditState,
    pub(crate) surrogate: Option<u16>,
    pub interval: String,
    pub proxy: String,
    pub peak_start: String,
    pub peak_end: String,
    pub name: String,
    pub key: String,
    pub org: String,
    pub project: String,
    /// User-edited marks for the two prefilled fields: prefilling must
    /// not read an empty buffer as "never edited" — a cleared but
    /// unapplied edit stays put. Set by apply_edit (every user-driven
    /// write, IME included, goes through it); reset wherever the buffer
    /// is cleared.
    pub interval_edited: bool,
    pub proxy_edited: bool,
}

impl PanelInput {
    /// Mutable reference to the active field's buffer; falls back to interval with no active field
    pub(crate) fn active_buf(&mut self) -> &mut String {
        match self.field {
            Some(InputField::Interval) => &mut self.interval,
            Some(InputField::Proxy) => &mut self.proxy,
            Some(InputField::PeakStart) => &mut self.peak_start,
            Some(InputField::PeakEnd) => &mut self.peak_end,
            Some(InputField::Name) => &mut self.name,
            Some(InputField::Key) => &mut self.key,
            Some(InputField::Org) => &mut self.org,
            Some(InputField::Project) => &mut self.project,
            None => &mut self.interval,
        }
    }

    /// Read-only reference to the active field's buffer; falls back to interval with no active field
    pub(crate) fn active_str(&self) -> &str {
        match self.field {
            Some(InputField::Interval) => &self.interval,
            Some(InputField::Proxy) => &self.proxy,
            Some(InputField::PeakStart) => &self.peak_start,
            Some(InputField::PeakEnd) => &self.peak_end,
            Some(InputField::Name) => &self.name,
            Some(InputField::Key) => &self.key,
            Some(InputField::Org) => &self.org,
            Some(InputField::Project) => &self.project,
            None => &self.interval,
        }
    }

    /// Clear one named field's buffer; the edited marks and the edit state
    /// are the callers' semantics and stay untouched here. Exhaustive like
    /// the other InputField dispatches: a new variant is a compile error.
    pub(crate) fn clear_field(&mut self, f: InputField) {
        match f {
            InputField::Interval => self.interval.clear(),
            InputField::Proxy => self.proxy.clear(),
            InputField::PeakStart => self.peak_start.clear(),
            InputField::PeakEnd => self.peak_end.clear(),
            InputField::Name => self.name.clear(),
            InputField::Key => self.key.clear(),
            InputField::Org => self.org.clear(),
            InputField::Project => self.project.clear(),
        }
    }
}

/// Input-slot hit → corresponding edit field; non-input variants are listed
/// explicitly, a new Hit variant missing registration is a compile error.
/// The eye (RevealKey) and the "Customize" button are not part of the text
/// zone and are not mapped
pub(crate) fn input_field_of_hit(hit: crate::ui::panel::render::Hit) -> Option<InputField> {
    use crate::ui::panel::render::Hit;
    match hit {
        Hit::InputInterval => Some(InputField::Interval),
        Hit::InputProxy => Some(InputField::Proxy),
        Hit::InputPeakStart => Some(InputField::PeakStart),
        Hit::InputPeakEnd => Some(InputField::PeakEnd),
        Hit::InputName => Some(InputField::Name),
        Hit::InputKey => Some(InputField::Key),
        Hit::InputOrg => Some(InputField::Org),
        Hit::InputProject => Some(InputField::Project),
        // Hits outside the text zones are never mapped to input fields.
        Hit::Refresh
        | Hit::Settings
        | Hit::AccountSwitch
        | Hit::UsageInfo
        | Hit::Back
        | Hit::ClosePanel
        | Hit::IntervalPreset(_)
        | Hit::CustomizeInterval
        | Hit::ApplyInterval
        | Hit::ApplyProxy
        | Hit::Language(_)
        | Hit::Appearance(_)
        | Hit::ToggleAutostart
        | Hit::ToggleThreshold
        | Hit::ToggleReset5h
        | Hit::ToggleResetWeekly
        | Hit::ApplyPeak
        | Hit::AddAccount
        | Hit::RemoveAccount(_)
        | Hit::PickAccount(_)
        | Hit::AccountType(_)
        | Hit::RevealKey
        | Hit::SaveAccount
        | Hit::Platform(_)
        | Hit::ExportConfig
        | Hit::ImportConfig
        | Hit::CheckUpdate
        | Hit::OpenDownload
        | Hit::LinkRepo
        | Hit::LinkIssues
        | Hit::CopyDiagnostics
        | Hit::NewsItem(_)
        | Hit::AboutLogo => None,
    }
}

/// Prefill-clamp and slot pinning: expected values derive from the byte
/// caps and the slot mapping above; changes must update them in sync
#[cfg(test)]
mod tests {
    use super::*;

    /// Per-field cache slots: every field maps to a distinct slot inside the
    /// array — a new InputField variant missing from slot() is a compile
    /// error, and this pin catches a slot-count drift.
    #[test]
    fn input_field_slots_distinct() {
        use crate::ui::panel::caret;
        let fields = [
            InputField::Interval,
            InputField::Proxy,
            InputField::PeakStart,
            InputField::PeakEnd,
            InputField::Name,
            InputField::Key,
            InputField::Org,
            InputField::Project,
        ];
        let slots: Vec<usize> = fields.iter().map(|f| f.slot()).collect();
        for s in &slots {
            assert!(*s < caret::CARET_SLOTS, "槽位越界");
        }
        let mut uniq = slots.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), fields.len(), "槽位互异");
    }

    /// Prefill clamp: within cap verbatim; over cap truncated at the byte
    /// cap on a char boundary — a multi-byte tail loses at most one char,
    /// never a split sequence
    #[test]
    fn clamp_prefill_char_boundary() {
        let max = InputField::Proxy.max_bytes();
        assert_eq!(
            InputField::Proxy.clamp_prefill(&"a".repeat(max)).len(),
            max,
            "贴上限原样保留"
        );
        // 2-byte CJK tail one byte short of fitting: the whole char goes
        let cjk = "a".repeat(max - 1) + "中中";
        assert_eq!(InputField::Proxy.clamp_prefill(&cjk).len(), max - 1);
        // 4-byte emoji tail: same whole-char retreat
        let emoji = "a".repeat(max - 1) + "😀😀";
        assert_eq!(InputField::Proxy.clamp_prefill(&emoji).len(), max - 1);
    }
}
