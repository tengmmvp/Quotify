//! Single source of truth for panel layout.
//! Spacing takes the 4px-grid tiers 2/4/6/8/12/16/20, and heights are always even so DPI
//! scaling lands on whole pixels; same-kind semantics share a tier (8 after rows, 6 after
//! inputs, 12 for section heads, 20
//! for margins), with ≤2px optical corrections exempt.

use crate::api::TOKEN_LEGS;
use crate::ui::panel::theme::PANEL_WIDTH;

/// Poll-interval preset tiers (seconds)
pub const INTERVAL_PRESETS: [u64; 4] = [60, 300, 900, 1800];

/// Top navigation bar height
pub const NAV_H: f32 = 30.0;
/// Section height included in section_label's return value
pub const SECTION_LABEL_H: f32 = 22.0;
/// segmented_raw body height
pub const SEGMENTED_H: f32 = 30.0;
/// Post-section gap included in segmented_raw's return value
pub const SEGMENTED_GAP: f32 = 8.0;
/// Self-drawn input field height
pub const INPUT_H: f32 = 26.0;
/// Left/right padding of the settings/main-view content area
pub const CONTENT_PAD: f32 = 20.0;
/// Content width: panel width minus the left/right padding
pub const CONTENT_W: f32 = PANEL_WIDTH as f32 - 2.0 * CONTENT_PAD;
/// Input field's left x, same as the content-area pad
pub const INPUT_X: f32 = CONTENT_PAD;
/// Gap after an input field to the next sub_label
pub const INPUT_GAP: f32 = 6.0;
/// Text inset inside an input field: the caret x, the IME composition
/// window position, the selection highlight, and every usable-width
/// derivation share this — companion to field_geo, the single source for
/// field text geometry.
pub const FIELD_TEXT_INSET: f32 = 6.0;
/// Input-field text typography (size, weight, mono) shared by the caret
/// width table and the field's text drawing: measured widths and drawn
/// glyphs must come from the same triple or the caret drifts off the text.
pub const INPUT_FONT: (f32, u16, bool) = (12.0, 400, true);
/// Proxy-row apply button width and its left gap: the input field's right edge yields by
/// the sum of the two,
/// shared by both the button drawing and field_geo
pub const APPLY_BTN_W: f32 = 48.0;
pub const APPLY_BTN_GAP: f32 = 12.0;

/// Input-field geometry of the active field (x, width, right-edge yield);
/// shares its source with each input_field
/// call site in settings.rs — 26 for the reveal toggle, the proxy row yielding to the
/// apply button (APPLY_BTN_W plus
/// the gap), and the rest inset 4 at the edge. Arms are exhaustive with no fallback:
/// a new InputField variant is a compile error
pub(crate) fn field_geo(field: super::InputField) -> (f32, f32, f32) {
    let cw = CONTENT_W;
    match field {
        super::InputField::Interval => (INPUT_X, 96.0, 4.0),
        super::InputField::Proxy => (INPUT_X, cw - APPLY_BTN_W - APPLY_BTN_GAP, 4.0),
        super::InputField::PeakStart => (PEAK_START_X, 64.0, 4.0),
        super::InputField::PeakEnd => (PEAK_END_X, 64.0, 4.0),
        super::InputField::Name => (INPUT_X, cw, 4.0),
        super::InputField::Key => (INPUT_X, cw, 26.0),
        super::InputField::Org => (INPUT_X, cw, 4.0),
        super::InputField::Project => (INPUT_X, cw, 4.0),
    }
}

/// Usable text width of a field: the caret's visible window, drag
/// clamping, and the draw-side visible slices all derive here — the
/// single source for "what fits", so the width table and the drawn slice
/// can never disagree.
pub(crate) fn field_avail(field: super::InputField) -> f32 {
    let (_x, w, tail) = field_geo(field);
    (w - FIELD_TEXT_INSET - tail).max(1.0)
}

// ── Main-view section heights: draw_main's y-advance chain and main_view_height share the same set of constants ──

/// Main-view top padding
pub(crate) const MAIN_TOP_PAD: f32 = 16.0;
/// Fixed view height of the no-data empty state: shared by the first-open placeholder and
/// the recalculation formula
pub(crate) const EMPTY_VIEW_H: f32 = 300.0;
/// Top bar row height (the row holding the account masthead and the two right-side buttons)
pub(crate) const MAIN_TOPBAR_H: f32 = 52.0;
/// Masthead: top gap from section start to the title [the solid divider hangs 2px below
/// the section start]
pub(crate) const MAIN_MASTHEAD_RULE_GAP: f32 = 12.0;
/// Masthead: height occupied by the title row
pub(crate) const MAIN_MASTHEAD_ROW_H: f32 = 26.0;
/// Full masthead section height: from section start to the first metric row
pub(crate) const MAIN_MASTHEAD_H: f32 = MAIN_MASTHEAD_RULE_GAP + MAIN_MASTHEAD_ROW_H;
/// Metric row height
pub(crate) const MAIN_METRIC_ROW_H: f32 = 52.0;
/// MCP composition area: bottom gap from the MCP metric row's bottom to the box top
pub(crate) const MAIN_MCP_COMP_TOP_GAP: f32 = 8.0;
/// MCP composition box padding (8 left/right, 6 top/bottom)
pub(crate) const MAIN_MCP_COMP_PAD_X: f32 = 8.0;
pub(crate) const MAIN_MCP_COMP_PAD_Y: f32 = 6.0;
/// Energy cell height (cell width 12, corner radius 2, and gap 2 live on the render side)
pub(crate) const MAIN_MCP_CELL_H: f32 = 10.0;
/// Advance from the energy-bar row to the legend row
pub(crate) const MAIN_MCP_LEGEND_ADV: f32 = 4.0;
/// Legend row height (the row holding the 11px badge text)
pub(crate) const MAIN_MCP_LEGEND_H: f32 = 16.0;
/// Full MCP composition area height: bottom gap + box border 2 + top/bottom padding + bar
/// + advance + legend row
pub(crate) const MAIN_MCP_COMP_H: f32 = MAIN_MCP_COMP_TOP_GAP
    + 2.0
    + 2.0 * MAIN_MCP_COMP_PAD_Y
    + MAIN_MCP_CELL_H
    + MAIN_MCP_LEGEND_ADV
    + MAIN_MCP_LEGEND_H;
/// Data section (Token/balance) leading gap: up to the dashed divider
pub(crate) const MAIN_SECTION_GAP: f32 = 6.0;
/// Data section: advance from the dashed divider to the section title
pub(crate) const MAIN_SECTION_HEAD: f32 = 12.0;
/// Token block: advance from the section head's text bottom to the card top
pub(crate) const MAIN_TOKEN_CARD_ADV: f32 = 20.0;
/// Token card geometry: card width derives from the panel width, and with the card gap it
/// keeps all three cards' edges multiples of 4,
/// so the x/y edges stroke whole pixels at every DPI scale. Card height derives from the
/// number band, the label band,
/// and top/bottom padding.
pub(crate) const TOKEN_CARD_NUM_Y: f32 = 8.0;
pub(crate) const TOKEN_CARD_NUM_H: f32 = 24.0;
pub(crate) const TOKEN_CARD_LABEL_Y: f32 = TOKEN_CARD_NUM_Y + TOKEN_CARD_NUM_H + 4.0;
pub(crate) const TOKEN_CARD_LABEL_H: f32 = 16.0;
pub(crate) const TOKEN_CARD_H: f32 = TOKEN_CARD_LABEL_Y + TOKEN_CARD_LABEL_H + 8.0;
pub(crate) const TOKEN_CARD_GAP: f32 = 12.0;
pub(crate) const TOKEN_CARD_W: f32 = (CONTENT_W - 2.0 * TOKEN_CARD_GAP) / TOKEN_LEGS.len() as f32;
/// Full Token usage block height: leading gap + section head + card advance + card height
pub(crate) const MAIN_TOKEN_BLOCK_H: f32 =
    MAIN_SECTION_GAP + MAIN_SECTION_HEAD + MAIN_TOKEN_CARD_ADV + TOKEN_CARD_H;
/// Height occupied by the balance row text
pub(crate) const MAIN_BALANCE_ROW_H: f32 = 18.0;
/// Full balance block height: effectively leading gap + section head + text row,
/// with an extra 2px breathing gap at the bottom folded into the height
pub(crate) const MAIN_BALANCE_BLOCK_H: f32 =
    MAIN_SECTION_GAP + MAIN_SECTION_HEAD + MAIN_BALANCE_ROW_H + 2.0;
/// Main-view content bottom gap: from the last data block's bottom to the footer top
pub(crate) const MAIN_CONTENT_BOTTOM_GAP: f32 = 2.0;
/// Footer area height (pinned to the bottom)
pub(crate) const MAIN_FOOTER_H: f32 = 36.0;
/// Full main-view tail height: content bottom gap + footer
pub(crate) const MAIN_TAIL_H: f32 = MAIN_CONTENT_BOTTOM_GAP + MAIN_FOOTER_H;
/// Pacman footer-swap animation's vertical overshoot headroom: the
/// glyph body (radius PACMAN_R, centered mid-strip) and the falling
/// crumbs push drawn pixels past the footer's text row toward both
/// strip edges; this headroom on top of MAIN_FOOTER_H keeps the repaint
/// band from ever clipping the animation itself.
pub(crate) const FOOTER_BAND_OVERSHOOT: f32 = 24.0;
/// Footer narrow-band height (anchored to the panel bottom): the footer
/// strip plus the swap animation's overshoot headroom. The animation
/// tick's footer band derives here, so band geometry follows section
/// heights at their single source instead of a hard-coded depth.
pub(crate) const FOOTER_BAND_H: f32 = MAIN_FOOTER_H + FOOTER_BAND_OVERSHOOT;

/// Compile-time band invariant: the band always covers the full footer
/// strip plus the overshoot headroom — a composition change that breaks
/// coverage is a build error, not a clipped animation at runtime.
const _: () = assert!(FOOTER_BAND_H >= MAIN_FOOTER_H + FOOTER_BAND_OVERSHOOT);

// ── Settings page section heights: shared by draw_settings's y advance, the *_input_y functions below, and settings_view_height ──

/// Settings page top/bottom margins: top padding before the nav and
/// bottom allowance after the version row
pub const SETTINGS_EDGE_PAD: f32 = 12.0;
/// With section_label's rule: top gap from the divider to the title
pub const SECTION_RULE_GAP: f32 = 12.0;
/// Full section height of a rule-bearing section head [shared by the
/// poll/notification/peak/general/network/config-management sections]
pub const SECTION_RULE_H: f32 = SECTION_RULE_GAP + SECTION_LABEL_H;
/// Account card body height: the drawn card box, shared by the card
/// drawing and the row height below
pub const CARD_BODY_H: f32 = 40.0;
/// Full account card row height: card body + post-card gap 8
pub const ACCOUNT_CARD_H: f32 = CARD_BODY_H + 8.0;
/// Auth-failure hint row height
pub const AUTH_ERROR_H: f32 = 18.0;
/// Always-visible add-account button height
pub const ADD_BTN_H: f32 = 28.0;
/// Add-account button row height: the button plus allowance above and below
pub const ADD_BTN_ROW_H: f32 = ADD_BTN_H + 6.0;
/// Custom-interval expansion delta: input field 26 + trailing gap 12 [same value as
/// the rule's top gap, but semantically independent]
pub const CUSTOMIZE_EXTRA_H: f32 = INPUT_H + 12.0;
/// Settings page persistent button height (apply/export/import/check for updates)
pub const BTN_H: f32 = 28.0;
/// Add-page save/cancel button height
pub const SAVE_BTN_H: f32 = 30.0;
/// Toggle row's three inner segments: title, description, trailing gap;
/// row height derives from the sum
pub const TOGGLE_TITLE_H: f32 = 20.0;
/// Toggle row title drawing-box height: inset 2px from the advance; stored paired with the advance
pub const TOGGLE_TITLE_DRAW_H: f32 = TOGGLE_TITLE_H - 2.0;
pub const TOGGLE_DESC_H: f32 = 12.0;
/// Toggle row description drawing-box height: a 12px font's descender needs 2px more than
/// the advance; stored paired with the advance.
pub const TOGGLE_DESC_DRAW_H: f32 = TOGGLE_DESC_H + 2.0;
pub const TOGGLE_TAIL: f32 = 8.0;
pub const TOGGLE_ROW_H: f32 = TOGGLE_TITLE_H + TOGGLE_DESC_H + TOGGLE_TAIL;
pub const TOGGLE_ROW_PLAIN_H: f32 = TOGGLE_TITLE_H + TOGGLE_TAIL;
/// Bottom gap after the peak input row
pub const PEAK_TAIL_GAP: f32 = 8.0;
/// Choice rows (language/appearance) trailing allowance
pub const CHOICE_ROW_TAIL: f32 = 2.0;
/// Full choice row height (language/appearance): sub-label + segmented control + trailing allowance
pub const CHOICE_ROW_H: f32 = SECTION_LABEL_H + SEGMENTED_H + SEGMENTED_GAP + CHOICE_ROW_TAIL;
/// Bottom gap after the proxy input field [the hint text is an in-field placeholder and
/// takes no extra row]
pub const PROXY_TAIL_GAP: f32 = 6.0;
/// Config-management button row height: button 28 + 8 after the row
pub const BACKUP_ROW_H: f32 = BTN_H + 8.0;
/// About section's bare divider (empty title): rule top gap + trailing gap 6
pub const ABOUT_DIVIDER_H: f32 = SECTION_RULE_GAP + 6.0;
/// Version row height: button top offset 1 + button 28 + bottom allowance 1
pub const VERSION_ROW_H: f32 = 1.0 + BTN_H + 1.0;

/// Add-page input row step: label + input field + post-field gap
const ADD_ROW_STEP: f32 = SECTION_LABEL_H + INPUT_H + INPUT_GAP;
/// Add page: top y of the name input field — the sum of the derivation chain of the
/// "Account details" title + the platform/type segmented
/// controls + the name sub-label
pub const ADD_NAME_Y: f32 =
    SETTINGS_EDGE_PAD + NAV_H + 4.0 * SECTION_LABEL_H + 2.0 * (SEGMENTED_H + SEGMENTED_GAP);
/// Add page: top y of the API Key input field
pub const ADD_KEY_Y: f32 = ADD_NAME_Y + ADD_ROW_STEP;
/// Add page, team edition: top y of the Organization ID input field
pub const ADD_ORG_Y: f32 = ADD_KEY_Y + ADD_ROW_STEP;
/// Add page, team edition: top y of the Project ID input field
pub const ADD_PROJECT_Y: f32 = ADD_ORG_Y + ADD_ROW_STEP;

/// Vertical centering offset of the caret within the input field
pub const CARET_Y_OFFSET: f32 = 5.0;

/// Peak start/end input field x, following the right side of their own text labels.
pub const PEAK_START_X: f32 = 48.0;
pub const PEAK_END_X: f32 = 148.0;

/// Settings page top y of the custom-interval input field; stretches with the account
/// block and the error row
pub fn interval_input_y(has_account: bool, auth_error: bool) -> f32 {
    let mut y = SETTINGS_EDGE_PAD + NAV_H + SECTION_LABEL_H;
    // With an account: card + optional error row; then the always-visible add-account button row.
    y += if has_account {
        ACCOUNT_CARD_H + if auth_error { AUTH_ERROR_H } else { 0.0 } + ADD_BTN_ROW_H
    } else {
        ADD_BTN_ROW_H
    };
    // Tail: poll section title + segmented body + post-section gap.
    y + SECTION_RULE_H + SEGMENTED_H + SEGMENTED_GAP
}

/// Settings page top y of the peak-hours input row, after the notification section
pub fn peak_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    // With the custom interval expanded, the render chain continues from interval_input_y
    // + CUSTOMIZE_EXTRA_H;
    // when collapsed, the segmented control's return value is interval_input_y itself —
    // the two states' y flows share the same point.
    let after_interval = if customizing { CUSTOMIZE_EXTRA_H } else { 0.0 };
    interval_input_y(has_account, auth_error)
        + after_interval
        + SECTION_RULE_H // notification section title
        + 3.0 * TOGGLE_ROW_H // three notification toggle rows
        + SECTION_RULE_H // peak-hours section title
}

/// Settings page top y of the proxy input field: after the peak section come the general
/// and network sections, continuing peak_input_y's chain
pub fn proxy_input_y(has_account: bool, auth_error: bool, customizing: bool) -> f32 {
    peak_input_y(has_account, auth_error, customizing)
        + INPUT_H + PEAK_TAIL_GAP // peak input row
        + SECTION_RULE_H // general section title
        + CHOICE_ROW_H // language row
        + CHOICE_ROW_H // appearance row
        + TOGGLE_ROW_PLAIN_H // autostart row
        + SECTION_RULE_H // network proxy section title
        + SECTION_LABEL_H // proxy sub-label
}

/// Settings page total height (logical pixels): continues from the proxy input field
/// through the config-management, About, and version rows to finish,
/// eliminating a separate whole-page sum inside view_height.
pub fn settings_view_height(has_account: bool, auth_error: bool, customizing: bool) -> i32 {
    (proxy_input_y(has_account, auth_error, customizing)
        + INPUT_H + PROXY_TAIL_GAP // proxy input field
        + SECTION_RULE_H + BACKUP_ROW_H // config management section
        + ABOUT_DIVIDER_H // About section bare divider
        + VERSION_ROW_H // version row
        + SETTINGS_EDGE_PAD) as i32 // bottom allowance
}

/// Add-page tail gaps after the last input row: team (ends at the
/// project field) vs personal (ends at the key field); shared by the
/// form's y advance and add_page_height
pub const ADD_TEAM_TAIL: f32 = 12.0;
pub const ADD_PERSONAL_TAIL: f32 = 6.0;

/// Add page total height (logical pixels).
///
/// Tail chain = last input row + save button + bottom allowance; for the team edition the
/// last row is the Project
/// ID field, otherwise the KEY field.
pub fn add_page_height(team: bool) -> i32 {
    let y = if team {
        ADD_PROJECT_Y + INPUT_H + ADD_TEAM_TAIL
    } else {
        ADD_KEY_Y + INPUT_H + INPUT_GAP + ADD_PERSONAL_TAIL
    };
    (y + SAVE_BTN_H + 10.0) as i32
}

/// Main view total height (logical pixels): fixed height for the loading/failure states;
/// the data state sums the main-view
/// section-constant chain above, sharing its source with draw_main's y advance —
/// both sides reference the same set of constants, so geometry changes
/// can no longer drift by updating only one half of the drawing side and the height formula.
pub fn main_view_height(
    has_data: bool,
    rows: usize,
    has_mcp_comp: bool,
    has_stats: bool,
    has_balance: bool,
) -> i32 {
    if !has_data {
        return EMPTY_VIEW_H as i32;
    }
    (MAIN_TOP_PAD
        + MAIN_TOPBAR_H
        + MAIN_MASTHEAD_H
        + rows as f32 * MAIN_METRIC_ROW_H
        + has_mcp_comp as i32 as f32 * MAIN_MCP_COMP_H
        + has_stats as i32 as f32 * MAIN_TOKEN_BLOCK_H
        + has_balance as i32 as f32 * MAIN_BALANCE_BLOCK_H
        + MAIN_TAIL_H) as i32
}

/// Single derivation source for the main-view height from a snapshot:
/// Panel::sync_derived's height computation shares it, composed here beside
/// main_view_height so the height chain lives in one layer.
pub fn main_height_of(snap: Option<&crate::api::UsageSnapshot>) -> i32 {
    let (rows, comp, stats, bal) = crate::api::main_features(snap);
    main_view_height(snap.is_some(), rows, comp, stats, bal)
}

/// Pinned regressions: expected values derive from the render y chain;
/// layout changes must update them in sync
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_input_y_pinned() {
        assert_eq!(interval_input_y(false, false), 170.0);
        assert_eq!(interval_input_y(false, true), 170.0);
        assert_eq!(interval_input_y(true, false), 218.0);
        assert_eq!(interval_input_y(true, true), 236.0);
    }

    #[test]
    fn peak_input_y_pinned() {
        assert_eq!(peak_input_y(false, false, false), 358.0);
        assert_eq!(peak_input_y(true, false, false), 406.0);
        assert_eq!(peak_input_y(true, true, false), 424.0);
        assert_eq!(peak_input_y(true, true, true), 462.0);
        assert_eq!(peak_input_y(false, false, true), 396.0);
    }

    #[test]
    fn proxy_input_y_pinned() {
        assert_eq!(proxy_input_y(false, false, false), 634.0);
        assert_eq!(proxy_input_y(true, false, false), 682.0);
        assert_eq!(proxy_input_y(true, true, false), 700.0);
        assert_eq!(proxy_input_y(true, true, true), 738.0);
        assert_eq!(proxy_input_y(false, false, true), 672.0);
    }

    #[test]
    fn add_page_height_pinned() {
        assert_eq!(add_page_height(false), 338);
        assert_eq!(add_page_height(true), 446);
    }

    #[test]
    fn token_card_geometry_pinned() {
        // Expected values derive from the 340 panel width and the render bands;
        // geometry changes must update them in sync
        assert_eq!(TOKEN_CARD_W, 92.0);
        assert_eq!(TOKEN_CARD_H, 60.0);
        assert_eq!(TOKEN_CARD_LABEL_Y, 36.0);
    }

    #[test]
    fn footer_band_covers_footer_strip() {
        // The composition is pinned end to end: a section-height tweak
        // that leaves the band behind changes the total and fails here —
        // update in sync (the ≥ coverage rule itself is the const
        // assert beside the definition).
        assert_eq!(MAIN_FOOTER_H, 36.0);
        assert_eq!(FOOTER_BAND_OVERSHOOT, 24.0);
        assert_eq!(FOOTER_BAND_H, 60.0);
    }

    #[test]
    fn main_view_height_pinned() {
        assert_eq!(main_view_height(false, 0, false, false, false), 300);
        assert_eq!(main_view_height(true, 0, false, false, false), 144);
        assert_eq!(main_view_height(true, 1, false, false, false), 196);
        assert_eq!(main_view_height(true, 3, false, false, true), 338);
        assert_eq!(main_view_height(true, 3, false, true, true), 436);
        assert_eq!(main_view_height(true, 2, false, true, false), 346);
        // MCP composition area: zero delta without data details, +52 with them
        assert_eq!(main_view_height(true, 3, true, false, false), 352);
        assert_eq!(main_view_height(true, 1, true, false, false), 248);
    }
}
