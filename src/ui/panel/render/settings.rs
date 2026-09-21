//! Settings view

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::widgets::BADGE_H;
use super::{AppearanceChoice, Hit, InputStyle, LanguageChoice, Renderer};
use crate::api::Platform;
use crate::ui::panel::caret::FieldContent;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::{CARD_RADIUS, RADIUS};
use crate::ui::panel::{InputField, Panel, layout};

// ── Settings frame cache ──
// Mirrors main.rs's MainTexts/MainLabels discipline: text whose content
// changes only with (language, config, data, width) is prebuilt as
// TextBlocks, so caret-blink, hover, and height-animation frames create
// zero layouts and zero intermediate strings.

/// Rebuild key: the full-value inputs of the cached layouts. Width enters
/// by bits (f32 semantic equality is bitwise equality). The per-frame
/// check compares against borrowed model data — owned clones live only
/// here, taken on the rebuild path.
struct SettingsKey {
    lang: crate::ui::i18n::Lang,
    threshold: u8,
    cw: u32,
    update: UpdateKey,
    account: Option<AccountKey>,
}

/// Update-check state feeding the about section: picks the button label
/// ("check" / "up to date" / "error" / "go download"); New carries the
/// release tag both the button label branch and the version line read
enum UpdateKey {
    Idle,
    OkNoUpdate,
    Err,
    New(String),
}

/// Borrowed mirror of UpdateKey straight off the frame data: the single
/// model.update → state mapping — the key's hit-path comparison and the
/// build-side key construction both read it, so the two cannot drift
#[derive(Clone, Copy)]
enum UpdateState<'a> {
    Idle,
    OkNoUpdate,
    Err,
    New(&'a str),
}

fn update_state_of<'m>(model: &'m PanelModel<'_>) -> UpdateState<'m> {
    match model.update {
        Some(Ok(info)) if model.update_available => UpdateState::New(&info.tag),
        Some(Ok(_)) => UpdateState::OkNoUpdate,
        Some(Err(_)) => UpdateState::Err,
        None => UpdateState::Idle,
    }
}

impl PartialEq<UpdateState<'_>> for UpdateKey {
    fn eq(&self, s: &UpdateState<'_>) -> bool {
        match (self, s) {
            (Self::Idle, UpdateState::Idle)
            | (Self::OkNoUpdate, UpdateState::OkNoUpdate)
            | (Self::Err, UpdateState::Err) => true,
            (Self::New(t), UpdateState::New(r)) => t == r,
            _ => false,
        }
    }
}

/// Account-card identity: name, platform + team annotation, and the
/// version/tier badge labels off the snapshot
struct AccountKey {
    name: String,
    platform: Platform,
    team: bool,
    version: Option<&'static str>,
    tier: Option<String>,
}

/// One badge on the account card: label block plus the badge box width
/// (label width + 14 padding) — measure and draw read the same layout
/// metrics, so box and text cannot drift
struct CardBadge {
    block: super::TextBlock,
    w: f32,
}

/// Account-card artifacts with all width-derived values baked in: badges
/// in right-to-left draw order plus the ellipsized name. Colors are NOT
/// cached — only the tier class is; the theme lookup stays per-frame so a
/// light/dark switch recolors without a rebuild.
struct AccountCardTexts {
    tier: Option<(CardBadge, crate::api::PlanTier)>,
    version: Option<CardBadge>,
    platform: CardBadge,
    name: super::TextBlock,
}

/// Gap between the version line's right edge and the update button; the
/// draw rect's right boundary and the ellipsis budget share it
const VER_BTN_GAP: f32 = 12.0;

/// About-section artifacts: the action button's label (whichever of the
/// four update states it holds) with its measured width, plus the
/// ellipsized version line
struct AboutTexts {
    btn: super::TextBlock,
    btn_w: f32,
    ver_line: super::TextBlock,
}

/// Settings frame cache: the threshold description, about-section
/// artifacts, and the account card — rebuilt as one unit whenever the key
/// changes. The constant labels live in their own language-keyed cache
/// (Renderer::settings_labels) and survive these rebuilds.
pub(super) struct SettingsTexts {
    key: SettingsKey,
    threshold_desc: super::TextBlock,
    about: AboutTexts,
    card: Option<AccountCardTexts>,
}

/// Constant-label layout set: every settings/add-form label whose content
/// changes only with the language. Field order = build order = draw order
/// (navigation → accounts/add form → interval → notifications → peak →
/// general → proxy → backup → about).
pub(super) struct SettingsLabels {
    // Navigation titles (16/400, centered in the nav bar)
    nav_settings: super::TextBlock,
    nav_add: super::TextBlock,
    // Accounts section: accent title 13/600, key-invalid line 12/400,
    // add pill 13/400, add-form's own section title
    sec_accounts: super::TextBlock,
    key_invalid: super::TextBlock,
    pill_add: super::TextBlock,
    sec_platform: super::TextBlock,
    // Add form: sub labels 13/400, segmented items 13/400, pills 13/400
    // (pub(super): the add-form view in add_form.rs reads this group)
    pub(super) sub_platform: super::TextBlock,
    pub(super) seg_platform_cn: super::TextBlock,
    pub(super) seg_platform_intl: super::TextBlock,
    pub(super) sub_type: super::TextBlock,
    pub(super) seg_personal: super::TextBlock,
    pub(super) seg_team: super::TextBlock,
    pub(super) sub_name: super::TextBlock,
    pub(super) sub_key: super::TextBlock,
    pub(super) sub_org: super::TextBlock,
    pub(super) sub_project: super::TextBlock,
    pub(super) pill_save: super::TextBlock,
    pub(super) pill_cancel: super::TextBlock,
    // Poll interval: title 13/600, segments 13/400, unit 12/400, apply 12/400
    sec_interval: super::TextBlock,
    iv_1m: super::TextBlock,
    iv_5m: super::TextBlock,
    iv_15m: super::TextBlock,
    iv_30m: super::TextBlock,
    iv_custom: super::TextBlock,
    iv_unit: super::TextBlock,
    btn_apply: super::TextBlock,
    // Notifications: titles 13/400, descs 12/400
    sec_notifications: super::TextBlock,
    t_threshold: super::TextBlock,
    t_reset_5h: super::TextBlock,
    d_reset_5h: super::TextBlock,
    t_reset_weekly: super::TextBlock,
    d_reset_weekly: super::TextBlock,
    // Peak hours: labels 12/400
    sec_peak: super::TextBlock,
    peak_start: super::TextBlock,
    peak_end: super::TextBlock,
    // General: title 13/600, sub labels + segments 13/400
    sec_general: super::TextBlock,
    sub_language: super::TextBlock,
    seg_follow_system: super::TextBlock,
    seg_lang_zh: super::TextBlock,
    seg_lang_en: super::TextBlock,
    sub_appearance: super::TextBlock,
    seg_light: super::TextBlock,
    seg_dark: super::TextBlock,
    t_autostart: super::TextBlock,
    // Network proxy
    sec_network: super::TextBlock,
    sub_proxy: super::TextBlock,
    // Config management: buttons 12/400
    sec_backup: super::TextBlock,
    btn_export: super::TextBlock,
    btn_import: super::TextBlock,
}

/// Version/tier badge labels from the snapshot: the shared source of the
/// key check and the card rebuild, so the two sides cannot drift
fn badge_labels<'a>(model: &'a PanelModel) -> (Option<&'static str>, Option<&'a str>) {
    match model.snapshot {
        Some(snap) => {
            let tier = snap.tier_label();
            let v = snap.plan_version.label();
            (
                (!v.is_empty()).then_some(v),
                (!tier.is_empty()).then_some(tier),
            )
        }
        None => (None, None),
    }
}

impl SettingsKey {
    /// Compare against borrowed frame data: the per-frame hit path
    /// allocates nothing (owned clones live in the key, taken only on
    /// rebuild). The update arm compares through update_state_of — the
    /// same mapping build_settings_texts reads.
    fn matches(&self, model: &PanelModel, cw: f32) -> bool {
        if self.lang != model.lang
            || self.threshold != model.threshold_percent
            || self.cw != cw.to_bits()
            || self.update != update_state_of(model)
        {
            return false;
        }
        let (version, tier) = badge_labels(model);
        match (&self.account, model.account) {
            (None, None) => true,
            (Some(k), Some(acc)) => {
                k.name == acc.name
                    && k.platform == acc.platform
                    && k.team == acc.team
                    && k.version == version
                    && k.tier.as_deref() == tier
            }
            _ => false,
        }
    }
}

impl Renderer {
    /// Settings view
    pub(super) unsafe fn draw_settings(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        w: f32,
        dy: f32,
        alpha: f32,
    ) {
        let s = model.strings;
        let pad = layout::CONTENT_PAD;
        let cw = w - pad * 2.0;

        // ── Frame caches (both exits below put them back): the key check
        // allocates zero on hits, so a key change is the only thing that
        // ever rebuilds layouts. The constant labels sit in their own
        // language-keyed cache — account switches and update results
        // rebuild the volatile part only ──
        let on_add_form = panel.view == crate::ui::panel::PanelView::AddForm;
        let mut labels_opt = self.settings_labels.take();
        if labels_opt.as_ref().is_none_or(|(l, _)| *l != model.lang) {
            labels_opt = Some((model.lang, self.build_settings_labels(s)));
        }
        let labels = labels_opt
            .as_ref()
            .map(|(_, ls)| ls)
            .expect("settings_labels 上方必已置");
        let mut texts_opt = self.settings_texts.take();
        // The add form consumes no volatile artifacts: skipping the key
        // check spares the full rebuild a background refresh would
        // otherwise trigger while it is open.
        if !on_add_form && texts_opt.as_ref().is_none_or(|t| !t.key.matches(model, cw)) {
            texts_opt = Some(self.build_settings_texts(s, model, cw));
        }

        let y = self.settings_nav(
            target,
            labels,
            on_add_form,
            pad,
            w,
            dy + layout::SETTINGS_EDGE_PAD,
            alpha,
        );
        // ── Accounts: the settings page's first section is the data source; on the add page this heading reads "Account info" ──
        let section = if on_add_form {
            &labels.sec_platform
        } else {
            &labels.sec_accounts
        };
        let y = self.section_label(target, Some(section), pad, y, w, alpha, false);
        if on_add_form {
            self.draw_add_form(target, panel, labels, pad, cw, dy, y, alpha);
            // Both exits put the taken caches back: losing them here would
            // force a full rebuild on the next paint.
            self.settings_texts = texts_opt;
            self.settings_labels = labels_opt;
            return;
        }
        let texts = texts_opt.expect("settings_texts 上方必已置");

        let y = self.accounts_section(target, panel, model, &texts, labels, pad, cw, y, alpha);

        let y = self.interval_section(target, panel, model, labels, dy, pad, cw, w, y, alpha);

        let y = self.notifications_section(target, model, &texts, labels, pad, cw, w, y, alpha);

        let y = self.peak_section(target, panel, model, labels, dy, pad, w, y, alpha);

        let y = self.general_section(target, model, labels, pad, cw, w, y, alpha);

        let y = self.proxy_section(target, panel, model, labels, dy, pad, cw, w, y, alpha);

        let y = self.backup_section(target, labels, pad, cw, w, y, alpha);

        self.about_section(target, model, &texts, pad, w, y, alpha);
        self.settings_texts = Some(texts);
        self.settings_labels = labels_opt;
    }

    // ── Settings sections, in draw order: each advances the shared y
    // chain and returns the next y ──

    /// Navigation bar: back arrow (settings page only), close button,
    /// and the page title
    #[allow(clippy::too_many_arguments)]
    unsafe fn settings_nav(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        labels: &SettingsLabels,
        on_add_form: bool,
        pad: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        if !on_add_form {
            self.back_arrow(target, Hit::Back, pad, y + 6.0);
        }
        self.x_button(target, Hit::ClosePanel, w - pad - 10.0, y + 6.0);
        let nav_title = if on_add_form {
            &labels.nav_add
        } else {
            &labels.nav_settings
        };
        let title_rect = D2D_RECT_F {
            left: pad,
            top: y,
            right: w - pad,
            bottom: y + 26.0,
        };
        self.block_c(
            target,
            nav_title,
            &title_rect,
            self.theme.text_primary,
            alpha,
        );
        y + layout::NAV_H
    }

    /// Accounts section: the active account card (plus the auth-error
    /// row) when present, then the always-present add entry — it takes
    /// its own full row spanning the content area for visual symmetry.
    /// All card text comes from the frame cache — badges, widths, and the
    /// ellipsized name are width-derived and were built together.
    #[allow(clippy::too_many_arguments)]
    unsafe fn accounts_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        texts: &SettingsTexts,
        labels: &SettingsLabels,
        pad: f32,
        cw: f32,
        mut y: f32,
        alpha: f32,
    ) -> f32 {
        if let (Some(acc), Some(card)) = (model.account, texts.card.as_ref()) {
            self.account_card(
                target,
                Hit::RemoveAccount(acc.index),
                card,
                pad,
                y,
                cw,
                alpha,
            );
            y += layout::ACCOUNT_CARD_H;
            // The predicate and the y chain share panel.auth_error: height and
            // the drawn row set come from the same source.
            if panel.auth_error {
                let r = D2D_RECT_F {
                    left: pad + 2.0,
                    top: y - 4.0,
                    right: pad + 2.0 + cw,
                    bottom: y + 12.0,
                };
                self.block_l(target, &labels.key_invalid, &r, self.theme.danger, alpha);
                y += layout::AUTH_ERROR_H;
            }
        }
        self.pill_button(
            target,
            Hit::AddAccount,
            pad,
            y + 2.0,
            cw,
            layout::ADD_BTN_H,
            &labels.pill_add,
            alpha,
            false,
        );
        y + layout::ADD_BTN_ROW_H
    }

    /// Poll interval: the segmented control's 5th segment is "Custom";
    /// selecting it expands an input row below
    #[allow(clippy::too_many_arguments)]
    unsafe fn interval_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        labels: &SettingsLabels,
        dy: f32,
        pad: f32,
        cw: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let mut y = self.section_label(target, Some(&labels.sec_interval), pad, y, w, alpha, true);
        // Fixed-length stack array with zero allocation; the sync between preset count and label slots is
        // pinned by the interval_preset_count test.
        let iv = [&labels.iv_1m, &labels.iv_5m, &labels.iv_15m, &labels.iv_30m];
        let presets: [(Hit, &super::TextBlock); layout::INTERVAL_PRESETS.len() + 1] =
            std::array::from_fn(|i| {
                if i < iv.len() {
                    (Hit::IntervalPreset(layout::INTERVAL_PRESETS[i]), iv[i])
                } else {
                    (Hit::CustomizeInterval, &labels.iv_custom)
                }
            });
        let cur = model.poll_interval_secs;
        let is_preset = layout::INTERVAL_PRESETS.contains(&cur);
        y = self.segmented_raw(
            target,
            &presets,
            |h| match h {
                Hit::IntervalPreset(v) => *v == cur,
                Hit::CustomizeInterval => !is_preset,
                _ => false,
            },
            pad,
            y,
            cw,
            alpha,
        );
        if panel.customizing.effective {
            // y is pinned to layout::interval_input_y, sharing the source with the caret and the height formula.
            let iy = dy + layout::interval_input_y(model.accounts_count > 0, panel.auth_error);
            // Stitch the accumulation chain to the formula chain: a drift
            // here paints the input off its caret/height source (debug only).
            debug_assert!((iy - y).abs() < 0.5, "interval 输入框 y 双链漂移");
            let input = &panel.input;
            self.input_field(
                panel,
                target,
                Hit::InputInterval,
                iy,
                None,
                FieldContent::Plain(&input.interval),
                "",
                input.field == Some(InputField::Interval),
                alpha,
            );
            let unit_rect = D2D_RECT_F {
                left: pad + 104.0,
                top: iy + 6.0,
                right: pad + 144.0,
                bottom: iy + 22.0,
            };
            self.block_l(
                target,
                &labels.iv_unit,
                &unit_rect,
                self.theme.text_secondary,
                alpha,
            );
            self.outline_button_blk(
                target,
                Hit::ApplyInterval,
                w - pad - 56.0,
                iy - 1.0,
                56.0,
                layout::BTN_H,
                &labels.btn_apply,
                alpha,
            );
            y = iy + layout::CUSTOMIZE_EXTRA_H;
        }
        y
    }

    /// Notifications: threshold + window-reset toggles
    #[allow(clippy::too_many_arguments)]
    unsafe fn notifications_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        texts: &SettingsTexts,
        labels: &SettingsLabels,
        pad: f32,
        cw: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let mut y = self.section_label(
            target,
            Some(&labels.sec_notifications),
            pad,
            y,
            w,
            alpha,
            true,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleThreshold,
            &labels.t_threshold,
            Some(&texts.threshold_desc),
            model.threshold_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleReset5h,
            &labels.t_reset_5h,
            Some(&labels.d_reset_5h),
            model.reset_5h_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        self.toggle_row(
            target,
            Hit::ToggleResetWeekly,
            &labels.t_reset_weekly,
            Some(&labels.d_reset_weekly),
            model.reset_weekly_enabled,
            pad,
            y,
            cw,
            alpha,
        )
    }

    /// Peak hours: effective on weekdays, custom start/end
    #[allow(clippy::too_many_arguments)]
    unsafe fn peak_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        labels: &SettingsLabels,
        dy: f32,
        pad: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        self.section_label(target, Some(&labels.sec_peak), pad, y, w, alpha, true);
        // y is pinned to layout::peak_input_y, sharing the source with the caret and the height formula.
        let pky = dy
            + layout::peak_input_y(
                model.accounts_count > 0,
                panel.auth_error,
                panel.customizing.effective,
            );
        // Stitch the accumulation chain to the formula chain: the dropped
        // section_label return must equal the formula's rule term (debug only).
        debug_assert!(
            (pky - y - layout::SECTION_RULE_H).abs() < 0.5,
            "peak 输入框 y 双链漂移"
        );
        // The current config value serves as a faint placeholder hint;
        // typing overwrites it, no deletion needed.
        let start_buf = panel.input.peak_start.as_str();
        let end_buf = panel.input.peak_end.as_str();
        // An invalid state (format error or both ends equal) gets a red frame as
        // a hint awaiting correction; empty buffers mean unedited and are not judged.
        let bad = |v: &str| !v.is_empty() && crate::ui::peak::parse_hhmm(v).is_none();
        let peak_bad = bad(start_buf)
            || bad(end_buf)
            || matches!(
                (
                    crate::ui::peak::parse_hhmm(start_buf),
                    crate::ui::peak::parse_hhmm(end_buf),
                ),
                (Some(s), Some(e)) if s == e
            );
        let start_rect = D2D_RECT_F {
            left: pad,
            top: pky + 7.0,
            right: pad + 26.0,
            bottom: pky + 21.0,
        };
        self.block_l(
            target,
            &labels.peak_start,
            &start_rect,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputPeakStart,
            pky,
            None,
            FieldContent::Plain(start_buf),
            model.peak_start_raw,
            panel.input.field == Some(InputField::PeakStart),
            alpha,
        );
        let end_rect = D2D_RECT_F {
            left: pad + 100.0,
            top: pky + 7.0,
            right: pad + 126.0,
            bottom: pky + 21.0,
        };
        self.block_l(
            target,
            &labels.peak_end,
            &end_rect,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputPeakEnd,
            pky,
            None,
            FieldContent::Plain(end_buf),
            model.peak_end_raw,
            panel.input.field == Some(InputField::PeakEnd),
            alpha,
        );
        self.outline_button_blk(
            target,
            Hit::ApplyPeak,
            w - pad - 48.0,
            pky - 1.0,
            48.0,
            layout::BTN_H,
            &labels.btn_apply,
            alpha,
        );
        if peak_bad {
            let edge = self.brush(target, self.theme.danger, alpha);
            for bx in [layout::PEAK_START_X, layout::PEAK_END_X] {
                target.DrawRectangle(
                    &D2D_RECT_F {
                        left: bx,
                        top: pky,
                        right: bx + 64.0,
                        bottom: pky + layout::INPUT_H,
                    },
                    &edge,
                    1.4,
                    None,
                );
            }
        }
        pky + layout::INPUT_H + layout::PEAK_TAIL_GAP
    }

    /// General: language / appearance / autostart
    #[allow(clippy::too_many_arguments)]
    unsafe fn general_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        labels: &SettingsLabels,
        pad: f32,
        cw: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let mut y = self.section_label(target, Some(&labels.sec_general), pad, y, w, alpha, true);
        y = self.sub_label(target, &labels.sub_language, pad, y, cw, alpha);
        let langs: [(Hit, &super::TextBlock); 3] = [
            (
                Hit::Language(LanguageChoice::System),
                &labels.seg_follow_system,
            ),
            (Hit::Language(LanguageChoice::Zh), &labels.seg_lang_zh),
            (Hit::Language(LanguageChoice::En), &labels.seg_lang_en),
        ];
        // Config string → choice enum: unset / unknown values all map to "follow the system".
        let cur_lang = LanguageChoice::from_setting(model.language);
        y = self.segmented_raw(
            target,
            &langs,
            |h| matches!(h, Hit::Language(v) if *v == cur_lang),
            pad,
            y,
            cw,
            alpha,
        );
        y += layout::CHOICE_ROW_TAIL;
        y = self.sub_label(target, &labels.sub_appearance, pad, y, cw, alpha);
        let themes: [(Hit, &super::TextBlock); 3] = [
            (
                Hit::Appearance(AppearanceChoice::System),
                &labels.seg_follow_system,
            ),
            (Hit::Appearance(AppearanceChoice::Light), &labels.seg_light),
            (Hit::Appearance(AppearanceChoice::Dark), &labels.seg_dark),
        ];
        let cur_theme = AppearanceChoice::from_setting(model.appearance);
        y = self.segmented_raw(
            target,
            &themes,
            |h| matches!(h, Hit::Appearance(v) if *v == cur_theme),
            pad,
            y,
            cw,
            alpha,
        );
        y += layout::CHOICE_ROW_TAIL;
        self.toggle_row(
            target,
            Hit::ToggleAutostart,
            &labels.t_autostart,
            None,
            model.autostart,
            pad,
            y,
            cw,
            alpha,
        )
    }

    /// Network proxy: empty address means direct connection; when
    /// unconfigured the field shows only the placeholder hint
    #[allow(clippy::too_many_arguments)]
    unsafe fn proxy_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        model: &PanelModel,
        labels: &SettingsLabels,
        dy: f32,
        pad: f32,
        cw: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let y = self.section_label(target, Some(&labels.sec_network), pad, y, w, alpha, true);
        self.sub_label(target, &labels.sub_proxy, pad, y, cw, alpha);
        // y is pinned to layout::proxy_input_y, sharing the source with the caret and the height formula.
        let py = dy
            + layout::proxy_input_y(
                model.accounts_count > 0,
                panel.auth_error,
                panel.customizing.effective,
            );
        // Stitch the accumulation chain to the formula chain: the dropped
        // sub_label return must equal the formula's label term (debug only).
        debug_assert!(
            (py - y - layout::SECTION_LABEL_H).abs() < 0.5,
            "proxy 输入框 y 双链漂移"
        );
        self.input_field(
            panel,
            target,
            Hit::InputProxy,
            py,
            None,
            FieldContent::Plain(&panel.input.proxy),
            model.strings.proxy_hint,
            panel.input.field == Some(InputField::Proxy),
            alpha,
        );
        self.outline_button_blk(
            target,
            Hit::ApplyProxy,
            w - pad - layout::APPLY_BTN_W,
            py - 1.0,
            layout::APPLY_BTN_W,
            layout::BTN_H,
            &labels.btn_apply,
            alpha,
        );
        py + layout::INPUT_H + layout::PROXY_TAIL_GAP
    }

    /// Config management: export / import, placed before the about section
    #[allow(clippy::too_many_arguments)]
    unsafe fn backup_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        labels: &SettingsLabels,
        pad: f32,
        cw: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) -> f32 {
        let y = self.section_label(target, Some(&labels.sec_backup), pad, y, w, alpha, true);
        let pair_w = 104.0 * 2.0 + 12.0;
        let bx = pad + (cw - pair_w) / 2.0;
        self.outline_button_blk(
            target,
            Hit::ExportConfig,
            bx,
            y,
            104.0,
            layout::BTN_H,
            &labels.btn_export,
            alpha,
        );
        self.outline_button_blk(
            target,
            Hit::ImportConfig,
            bx + 116.0,
            y,
            104.0,
            layout::BTN_H,
            &labels.btn_import,
            alpha,
        );
        y + layout::BACKUP_ROW_H
    }

    /// About: check update + version, at the bottom
    #[allow(clippy::too_many_arguments)]
    unsafe fn about_section(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        model: &PanelModel,
        texts: &SettingsTexts,
        pad: f32,
        w: f32,
        y: f32,
        alpha: f32,
    ) {
        // Version line closing: with a new release the text reads "current → latest" and
        // the button switches to "Go download". The label/width/version
        // line all come from the cache — the update state is part of the
        // key, so the button never shows stale copy; btn_w feeds the
        // version line's right boundary and its build-time ellipsis
        // budget alike.
        let y = self.section_label(target, None, pad, y, w, alpha, true);
        let btn_hit = if model.update_available {
            Hit::OpenDownload
        } else {
            Hit::CheckUpdate
        };
        let btn_w = texts.about.btn_w;
        let ver_rect = D2D_RECT_F {
            left: pad,
            top: y + 7.0,
            right: w - pad - btn_w - VER_BTN_GAP,
            bottom: y + 23.0,
        };
        self.block_l(
            target,
            &texts.about.ver_line,
            &ver_rect,
            self.theme.text_tertiary,
            alpha,
        );
        self.outline_button_blk(
            target,
            btn_hit,
            w - pad - btn_w,
            y + 1.0,
            btn_w,
            layout::BTN_H,
            &texts.about.btn,
            alpha,
        );
    }

    // ── Settings-page-specific widgets ──

    /// Rebuild the constant-label set: keyed by the language alone, so it
    /// lives in its own cache and survives key changes of the volatile
    /// settings cache (account switches, update results). Each block's
    /// size/weight is written here next to its field.
    unsafe fn build_settings_labels(&mut self, s: &crate::ui::i18n::Strings) -> SettingsLabels {
        SettingsLabels {
            nav_settings: self.make_text_block(s.settings, 16.0, 400),
            nav_add: self.make_text_block(s.add_account, 16.0, 400),
            sec_accounts: self.make_text_block(s.accounts_section, 13.0, 600),
            key_invalid: self.make_text_block(s.key_invalid, 12.0, 400),
            pill_add: self.make_text_block(s.add_account, 13.0, 400),
            sec_platform: self.make_text_block(s.platform_section, 13.0, 600),
            sub_platform: self.make_text_block(s.account_platform, 13.0, 400),
            seg_platform_cn: self.make_text_block(s.platform_cn, 13.0, 400),
            seg_platform_intl: self.make_text_block(s.platform_intl, 13.0, 400),
            sub_type: self.make_text_block(s.account_type_label, 13.0, 400),
            seg_personal: self.make_text_block(s.type_personal, 13.0, 400),
            seg_team: self.make_text_block(s.type_team, 13.0, 400),
            sub_name: self.make_text_block(s.account_name, 13.0, 400),
            sub_key: self.make_text_block(s.api_key_label, 13.0, 400),
            sub_org: self.make_text_block(s.org_id_label, 13.0, 400),
            sub_project: self.make_text_block(s.project_id_label, 13.0, 400),
            pill_save: self.make_text_block(s.save, 13.0, 400),
            pill_cancel: self.make_text_block(s.cancel, 13.0, 400),
            sec_interval: self.make_text_block(s.poll_interval, 13.0, 600),
            iv_1m: self.make_text_block(s.interval_1m, 13.0, 400),
            iv_5m: self.make_text_block(s.interval_5m, 13.0, 400),
            iv_15m: self.make_text_block(s.interval_15m, 13.0, 400),
            iv_30m: self.make_text_block(s.interval_30m, 13.0, 400),
            iv_custom: self.make_text_block(s.interval_custom, 13.0, 400),
            iv_unit: self.make_text_block(s.interval_custom_unit, 12.0, 400),
            btn_apply: self.make_text_block(s.apply, 12.0, 400),
            sec_notifications: self.make_text_block(s.notifications, 13.0, 600),
            t_threshold: self.make_text_block(s.notify_threshold, 13.0, 400),
            t_reset_5h: self.make_text_block(s.notify_reset_5h_opt, 13.0, 400),
            d_reset_5h: self.make_text_block(s.notify_reset_5h_desc, 12.0, 400),
            t_reset_weekly: self.make_text_block(s.notify_reset_weekly_opt, 13.0, 400),
            d_reset_weekly: self.make_text_block(s.notify_reset_weekly_desc, 12.0, 400),
            sec_peak: self.make_text_block(s.peak_section, 13.0, 600),
            peak_start: self.make_text_block(s.peak_start_label, 12.0, 400),
            peak_end: self.make_text_block(s.peak_end_label, 12.0, 400),
            sec_general: self.make_text_block(s.settings_general, 13.0, 600),
            sub_language: self.make_text_block(s.language, 13.0, 400),
            seg_follow_system: self.make_text_block(s.follow_system, 13.0, 400),
            seg_lang_zh: self.make_text_block(s.lang_name_zh, 13.0, 400),
            seg_lang_en: self.make_text_block(s.lang_name_en, 13.0, 400),
            sub_appearance: self.make_text_block(s.appearance_section, 13.0, 400),
            seg_light: self.make_text_block(s.theme_light, 13.0, 400),
            seg_dark: self.make_text_block(s.theme_dark, 13.0, 400),
            t_autostart: self.make_text_block(s.autostart, 13.0, 400),
            sec_network: self.make_text_block(s.network_section, 13.0, 600),
            sub_proxy: self.make_text_block(s.proxy_label, 13.0, 400),
            sec_backup: self.make_text_block(s.backup_section, 13.0, 600),
            btn_export: self.make_text_block(s.export_config, 12.0, 400),
            btn_import: self.make_text_block(s.import_config, 12.0, 400),
        }
    }

    /// Rebuild the volatile settings cache in one pass: threshold
    /// description, about artifacts, and the account card. Each block's
    /// size/weight is written here next to its field — the draw side
    /// reads geometry only, so a typography change has one home.
    unsafe fn build_settings_texts(
        &mut self,
        s: &crate::ui::i18n::Strings,
        model: &PanelModel,
        cw: f32,
    ) -> SettingsTexts {
        let threshold_desc = self.make_text_block(
            &s.notify_threshold_desc
                .replace("{p}", &model.threshold_percent.to_string()),
            12.0,
            400,
        );
        // About artifacts follow the live update state: New swaps both the
        // button copy ("go download") and the version line ("cur → new").
        let update = match update_state_of(model) {
            UpdateState::Idle => UpdateKey::Idle,
            UpdateState::OkNoUpdate => UpdateKey::OkNoUpdate,
            UpdateState::Err => UpdateKey::Err,
            UpdateState::New(tag) => UpdateKey::New(tag.to_string()),
        };
        let cur = env!("CARGO_PKG_VERSION");
        let plain_ver = || s.version_label.replace("{v}", cur);
        let (btn_text, ver_text) = match &update {
            UpdateKey::New(tag) => (
                s.go_download.to_string(),
                s.version_new
                    .replace("{cur}", cur)
                    .replace("{new}", tag.trim_start_matches('v')),
            ),
            UpdateKey::OkNoUpdate => (s.up_to_date.to_string(), plain_ver()),
            UpdateKey::Err => (s.err_update.to_string(), plain_ver()),
            UpdateKey::Idle => (s.check_update.to_string(), plain_ver()),
        };
        let btn = self.make_text_block(&btn_text, 12.0, 400);
        // Width from the same layout metrics the old per-frame measure
        // read — the version line's right boundary is unchanged.
        let btn_w = (btn.w + 28.0).max(104.0);
        // A long release tag ellipsizes against the post-button remainder
        // instead of the draw rect's hard clip — same discipline as the
        // card name below.
        let (ver_disp, _) = self.ellipsize(
            &ver_text,
            12.0,
            (cw - btn_w - VER_BTN_GAP).max(0.0),
            400,
            false,
        );
        let about = AboutTexts {
            btn,
            btn_w,
            ver_line: self.make_text_block(&ver_disp, 12.0, 400),
        };
        // Account card: badges (widths baked in) plus the name ellipsized
        // against the badge-zone remainder. The pad cancels out of the
        // walk, so cw alone pins the budget.
        let (version, tier) = badge_labels(model);
        let card = model.account.map(|acc| {
            let platform = super::platform_with_team(s, acc.platform, acc.team, " | ");
            let mk = |r: &Self, t: &str, mono: bool| {
                let b = super::TextBlock::new(r.make_layout(t, 10.5, 400, mono));
                CardBadge {
                    w: b.w + super::BADGE_PAD_X,
                    block: b,
                }
            };
            let tier_badge =
                tier.map(|t| (mk(self, t, false), crate::api::PlanTier::from_label(t)));
            let version_badge = version.map(|v| mk(self, v, true));
            let platform_badge = mk(self, &platform, false);
            let (_, _, _, name_max) = Self::account_badge_walk(
                cw,
                tier_badge.as_ref().map(|(t, _)| t.w),
                version_badge.as_ref().map(|v| v.w),
                platform_badge.w,
            );
            let (name_disp, _) = self.ellipsize(acc.name, 15.0, name_max, 500, false);
            AccountCardTexts {
                tier: tier_badge,
                version: version_badge,
                platform: platform_badge,
                name: super::TextBlock::new(self.make_layout(&name_disp, 15.0, 500, false)),
            }
        });
        SettingsTexts {
            key: SettingsKey {
                lang: model.lang,
                threshold: model.threshold_percent,
                cw: cw.to_bits(),
                update,
                account: model.account.map(|acc| AccountKey {
                    name: acc.name.to_string(),
                    platform: acc.platform,
                    team: acc.team,
                    version,
                    tier: tier.map(|t| t.to_string()),
                }),
            },
            threshold_desc,
            about,
            card,
        }
    }

    /// Outlined button drawing a cached label block: thin rounded
    /// outline, hover tint + primary-color label; the hit rect grows
    /// 4px past the visual box for an easier target
    #[allow(clippy::too_many_arguments)]
    unsafe fn outline_button_blk(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &super::TextBlock,
        alpha: f32,
    ) {
        let hovered = self.hover == Some(hit);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        if hovered {
            let b = self.brush(target, self.theme.track, alpha);
            target.FillRoundedRectangle(&r, &b);
        }
        let edge = self.brush(target, self.theme.border, alpha * 0.9);
        target.DrawRoundedRectangle(&r, &edge, 1.0, None);
        let fg = if hovered {
            self.theme.text_primary
        } else {
            self.theme.text_secondary
        };
        let rect = D2D_RECT_F {
            left: x,
            top: y + 5.0,
            right: x + w,
            bottom: y + h - 4.0,
        };
        self.block_c(target, label, &rect, fg, alpha);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                bottom: y + h + 4.0,
            },
        ));
    }

    /// Section main title: text_primary-colored accent bar + title;
    /// sub-labels carry no bar to distinguish hierarchy.
    /// None draws the rule with no title (the about section's bare divider)
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: Option<&super::TextBlock>,
        x: f32,
        y: f32,
        _w: f32,
        alpha: f32,
        rule: bool,
    ) -> f32 {
        let mut ny = y;
        if rule {
            self.divider(target, x, ny + 2.0, _w - x * 2.0, alpha);
            ny += layout::SECTION_RULE_GAP;
        }
        if let Some(label) = label {
            // The title is vcentered in the ACCENT_ROW_H band right of the
            // accent bar, its right boundary at the content edge _w - x —
            // mirroring the left pad, same as the divider's span; main.rs's
            // accent_label shares the same band geometry.
            self.fill_accent_bar(target, x, ny, alpha);
            let rect = D2D_RECT_F {
                left: x + super::widgets::ACCENT_INSET,
                top: ny,
                right: _w - x,
                bottom: ny + super::widgets::ACCENT_ROW_H,
            };
            self.block_lv(target, label, &rect, self.theme.text_tertiary, alpha);
            ny + layout::SECTION_LABEL_H
        } else {
            // Pure divider without a title, leaving only a small gap for the content that follows.
            ny + 6.0
        }
    }

    /// Sub-item label within a section: 13/400 secondary color, no accent bar
    /// Type scale: 16 navigation / 13·600 section titles / 13·400 control labels / 12·400 descriptions
    pub(super) unsafe fn sub_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &super::TextBlock,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let rect = D2D_RECT_F {
            left: x,
            top: y + 1.0,
            right: x + w,
            bottom: y + 18.0,
        };
        self.block_l(target, label, &rect, self.theme.text_secondary, alpha);
        y + layout::SECTION_LABEL_H
    }

    /// Self-drawn caret: drawn only with keyboard focus and the blink phase in the on
    /// segment. cx is the in-field caret x;
    /// the width takes whole physical pixels to avoid faint anti-aliased edges at
    /// fractional scale factors.
    unsafe fn draw_caret(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        x: f32,
        cx: f32,
        y: f32,
        alpha: f32,
    ) {
        let focused = panel
            .hwnd
            .is_some_and(|h| windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == h);
        if !focused || !panel.caret_blink().0 {
            return;
        }
        let left = x + layout::FIELD_TEXT_INSET + cx;
        let cw = panel.px(1).max(1) as f32 / panel.dpi;
        let caret = self.brush(target, self.theme.action, alpha);
        target.FillRectangle(
            &D2D_RECT_F {
                left,
                top: y + layout::CARET_Y_OFFSET,
                right: left + cw,
                bottom: y + layout::CARET_Y_OFFSET + 16.0,
            },
            &caret,
        );
    }

    /// Self-drawn input field: the caret is drawn directly, the IME composition window is
    /// positioned at the caret.
    /// The active state draws by the caret's visible window and highlights the selection;
    /// the unfocused state draws a tail slice.
    /// Geometry x, width, and right-edge clearance come from field_geo as the single source.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn input_field(
        &mut self,
        panel: &Panel,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        y: f32,
        eye: Option<(Hit, bool)>,
        content: FieldContent<'_>,
        placeholder: &str,
        active: bool,
        alpha: f32,
    ) {
        let Some(field) = crate::ui::panel::input_field_of_hit(hit) else {
            return;
        };
        // The text typography shares INPUT_FONT with the caret width table;
        // the placeholder stays non-mono (it never feeds the width table).
        let (text_sz, text_wt, text_mono) = layout::INPUT_FONT;
        let (x, w, tail) = crate::ui::panel::layout::field_geo(field);
        let rect = D2D_RECT_F {
            left: x,
            top: y,
            right: x + w,
            bottom: y + layout::INPUT_H,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRectangle(&rect, &fill);
        let edge_color = if active {
            self.theme.action
        } else {
            self.theme.border
        };
        let edge = self.brush(target, edge_color, alpha);
        target.DrawRectangle(&rect, &edge, 1.2, None);
        if content.is_empty() && !placeholder.is_empty() {
            let ph = self.input_block(
                InputStyle::Placeholder,
                field,
                |s| s == placeholder,
                || placeholder.to_string(),
                12.0,
                400,
                false,
            );
            self.draw_block_at(
                target,
                &ph,
                x + layout::FIELD_TEXT_INSET,
                y + 6.0,
                self.theme.text_tertiary,
                alpha,
                false,
            );
            // In the focused state with an empty buffer, the caret is drawn at the start of the field.
            if active {
                self.draw_caret(target, panel, x, 0.0, y, alpha);
            }
        } else if active {
            let avail = layout::field_avail(field);
            let cl = panel.caret_layout(self);
            let chars = &cl.widths.chars;
            let start = cl.vis_start.min(chars.len());
            let mut end = start;
            while end < chars.len() && cl.seg(start, end + 1) <= avail {
                end += 1;
            }
            if let Some((a, b)) = panel.input.edit.selection() {
                let a = a.min(chars.len());
                let b = b.min(chars.len());
                let sx = |n: usize| cl.seg(start, n);
                let inset = x + layout::FIELD_TEXT_INSET;
                let sel_left = inset + sx(a);
                let sel_right = inset + sx(b);
                if sel_right > inset && sel_left < x + w - tail {
                    let hl = self.brush(target, self.theme.action, alpha * 0.16);
                    target.FillRectangle(
                        &D2D_RECT_F {
                            left: sel_left.max(inset),
                            top: y + 5.0,
                            right: sel_right.min(x + w - tail),
                            bottom: y + 21.0,
                        },
                        &hl,
                    );
                }
            }
            // Cache hits compare the visible slice char-by-char and never
            // materialize the string; only a real miss collects it.
            let vis_blk = self.input_block(
                InputStyle::Body,
                field,
                |s| s.chars().eq(chars[start..end].iter().copied()),
                || chars[start..end].iter().collect(),
                text_sz,
                text_wt,
                text_mono,
            );
            self.draw_block_at(
                target,
                &vis_blk,
                x + layout::FIELD_TEXT_INSET,
                y + 6.0,
                self.theme.text_primary,
                alpha,
                false,
            );
            // The caret x and IME positioning take the same layout product.
            self.draw_caret(target, panel, x, cl.cx, y, alpha);
        } else {
            // Unfocused state: the tail visual slice takes the longest
            // suffix that fits; the char list and width table come from
            // the shared (field, display) artifacts — repaint frames
            // (minute heartbeat, hover, height transitions) build zero
            // layouts and materialize zero strings; suffix width = full
            // width - prefix width, zero per-candidate measuring.
            let avail = layout::field_avail(field);
            let art = panel.widths_of(self, field, &content);
            // Slice range: the whole display when it fits, else the
            // longest suffix that does [suffix width grows monotonically
            // as the start decreases, so the position before the first
            // overflow is the longest suffix that fits; keep at least the
            // last 1 character].
            let n = art.chars.len();
            let start = if n == 0 || art.widths[n] <= avail {
                0
            } else {
                let mut k = 0usize;
                while k + 1 < n && art.widths[n] - art.widths[k] > avail {
                    k += 1;
                }
                k
            };
            let vis_blk = self.input_block(
                InputStyle::Body,
                field,
                |s| s.chars().eq(art.chars[start..].iter().copied()),
                || art.chars[start..].iter().collect(),
                text_sz,
                text_wt,
                text_mono,
            );
            self.draw_block_at(
                target,
                &vis_blk,
                x + layout::FIELD_TEXT_INSET,
                y + 6.0,
                self.theme.text_primary,
                alpha,
                false,
            );
        }
        if let Some((eye_hit, revealed)) = eye {
            let (ecx, ecy) = (x + w - 15.0, y + layout::INPUT_H / 2.0);
            self.eye(
                target,
                ecx,
                ecy,
                13.0,
                revealed,
                self.theme.text_secondary,
                alpha,
            );
            // The eye hit zone is registered before the whole field:
            // hit_at takes the first hit, and a later registration would be swallowed by
            // the whole-field rect.
            self.hits.push((
                eye_hit,
                D2D_RECT_F {
                    left: ecx - 12.0,
                    top: y + 2.0,
                    right: ecx + 12.0,
                    bottom: y + layout::INPUT_H - 2.0,
                },
            ));
        }
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                // The field hit zone spans the drawn box plus the shared
                // 4px margin, derived from INPUT_H — not a hand-computed
                // constant that drifts when the box height changes.
                bottom: y + layout::INPUT_H + 4.0,
            },
        ));
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn toggle_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        title: &super::TextBlock,
        desc: Option<&super::TextBlock>,
        on: bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let title_rect = D2D_RECT_F {
            left: x,
            top: y + 1.0,
            right: x + w - 56.0,
            bottom: y + 1.0 + layout::TOGGLE_TITLE_DRAW_H,
        };
        self.block_l(target, title, &title_rect, self.theme.text_primary, alpha);
        let mut ty = y + layout::TOGGLE_TITLE_H;
        if let Some(desc) = desc {
            let desc_rect = D2D_RECT_F {
                left: x,
                top: ty,
                right: x + w - 56.0,
                bottom: ty + layout::TOGGLE_DESC_DRAW_H,
            };
            self.block_l(target, desc, &desc_rect, self.theme.text_tertiary, alpha);
            ty += layout::TOGGLE_DESC_H;
        }
        let (tw, th) = (38.0, 22.0);
        let tx = x + w - tw;
        let cy = (y + (ty - y - th) / 2.0).max(y);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: tx,
                top: cy,
                right: tx + tw,
                bottom: cy + th,
            },
            radiusX: th / 2.0,
            radiusY: th / 2.0,
        };
        let color = if on { self.theme.ok } else { self.theme.border };
        let b = self.brush(target, color, alpha);
        target.FillRoundedRectangle(&r, &b);
        let knob = th - 4.0;
        let kx = if on { tx + tw - knob - 2.0 } else { tx + 2.0 };
        let kb = self.brush(target, [1.0, 1.0, 1.0, 1.0], alpha);
        let ellipse = windows::Win32::Graphics::Direct2D::D2D1_ELLIPSE {
            point: windows_numerics::Vector2 {
                X: kx + knob / 2.0,
                Y: cy + th / 2.0,
            },
            radiusX: knob / 2.0,
            radiusY: knob / 2.0,
        };
        target.FillEllipse(&ellipse, &kb);
        // The hit zone covers only the toggle body plus an 8px tolerance;
        // clicking the row's text/blank space does not flip it.
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: tx - 8.0,
                top: cy - 6.0,
                right: tx + tw + 8.0,
                bottom: cy + th + 6.0,
            },
        ));
        ty + layout::TOGGLE_TAIL
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn segmented_raw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        items: &[(Hit, &super::TextBlock)],
        selected: impl Fn(&Hit) -> bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        let h = layout::SEGMENTED_H;
        let n = items.len().max(1) as f32;
        let seg_w = w / n;
        let track = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let tb = self.brush(target, self.theme.border, alpha * 0.9);
        target.DrawRoundedRectangle(&track, &tb, 1.0, None);
        for (i, (hit, label)) in items.iter().enumerate() {
            let sel = selected(hit);
            if sel {
                let seg = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left: x + i as f32 * seg_w + 2.0,
                        top: y + 2.0,
                        right: x + (i as f32 + 1.0) * seg_w - 2.0,
                        bottom: y + h - 2.0,
                    },
                    radiusX: 2.5,
                    radiusY: 2.5,
                };
                let sb = self.brush(target, self.theme.action, alpha);
                target.FillRoundedRectangle(&seg, &sb);
            }
            let color = if sel {
                self.theme.action_text
            } else {
                self.theme.text_secondary
            };
            let tx = x + i as f32 * seg_w;
            let rect = D2D_RECT_F {
                left: tx,
                top: y,
                right: tx + seg_w,
                bottom: y + h,
            };
            self.block_vc(target, label, &rect, color, alpha);
            self.hits.push((
                *hit,
                D2D_RECT_F {
                    left: tx,
                    top: y,
                    right: tx + seg_w,
                    bottom: y + h,
                },
            ));
        }
        y + h + layout::SEGMENTED_GAP
    }

    /// Badge-zone walk shared by the cache build and the card draw: from
    /// the right boundary (56 for the × button), tier/version/platform
    /// stack leftward with 6 between badges; returns (tier_x, version_x,
    /// platform_x, name_max) in card-local x. One walk keeps the name's
    /// truncation budget and the drawn badge row on the same numbers.
    fn account_badge_walk(
        w: f32,
        tier_w: Option<f32>,
        version_w: Option<f32>,
        platform_w: f32,
    ) -> (Option<f32>, Option<f32>, f32, f32) {
        let mut bx = w - 56.0;
        let tier_x = tier_w.map(|t| {
            bx -= t;
            let x = bx;
            bx -= 6.0;
            x
        });
        let version_x = version_w.map(|v| {
            bx -= v;
            let x = bx;
            bx -= 6.0;
            x
        });
        bx -= platform_w;
        let platform_x = bx;
        let name_max = (bx - 10.0 - 12.0).max(60.0);
        (tier_x, version_x, platform_x, name_max)
    }

    /// Single account card, all text from the frame cache: badges carry
    /// their baked widths, the name was ellipsized against the same badge
    /// walk performed here (see build_settings_texts)
    #[allow(clippy::too_many_arguments)]
    unsafe fn account_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        remove: Hit,
        card: &AccountCardTexts,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) {
        let h = layout::CARD_BODY_H;
        let rect = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: CARD_RADIUS,
            radiusY: CARD_RADIUS,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRoundedRectangle(&rect, &fill);
        let edge = self.brush(target, self.theme.card_border, alpha);
        target.DrawRoundedRectangle(&rect, &edge, 1.0, None);
        // Badges and the close button align right, the name takes the remaining space on
        // the left; tier/version/platform are laid out from the right boundary leftward in order.
        // Colors stay per-frame (theme lookups) — only widths and layouts are cached;
        // the positions come from the same walk the name budget was measured against.
        let by = y + (h - BADGE_H) / 2.0;
        let (tier_x, version_x, platform_x, name_max) = Self::account_badge_walk(
            w,
            card.tier.as_ref().map(|(t, _)| t.w),
            card.version.as_ref().map(|v| v.w),
            card.platform.w,
        );
        if let (Some((tier, class)), Some(tx)) = (card.tier.as_ref(), tier_x) {
            let (edge, fg) = self.tier_badge_colors(*class);
            self.badge_blk(target, &tier.block, x + tx, by, tier.w, edge, fg, alpha);
        }
        if let (Some(version), Some(vx)) = (card.version.as_ref(), version_x) {
            self.badge_blk(
                target,
                &version.block,
                x + vx,
                by,
                version.w,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
            );
        }
        self.badge_blk(
            target,
            &card.platform.block,
            x + platform_x,
            by,
            card.platform.w,
            self.theme.border,
            self.theme.text_secondary,
            alpha,
        );
        let name_rect = D2D_RECT_F {
            left: x + 12.0,
            top: y,
            right: x + 12.0 + name_max,
            bottom: y + h,
        };
        self.block_lv(
            target,
            &card.name,
            &name_rect,
            self.theme.text_primary,
            alpha,
        );
        self.x_button(target, remove, x + w - 24.0, y + 15.0);
    }

    /// Tier color grayscale gradient keyed off the PlanTier classification — the
    /// same ladder the data side derives; a tier-rule change propagates to the
    /// badge automatically. Max darkest, Pro next, Lite and unknown lightest.
    /// Takes the class (cached at build) instead of the label string, so no
    /// per-frame to_ascii_lowercase.
    fn tier_badge_colors(&self, tier: crate::api::PlanTier) -> ([f32; 4], [f32; 4]) {
        match tier {
            crate::api::PlanTier::Max => (self.theme.text_primary, self.theme.text_primary),
            crate::api::PlanTier::Pro => (self.theme.text_secondary, self.theme.text_secondary),
            _ => (self.theme.border, self.theme.text_tertiary),
        }
    }

    /// Small rounded filled button drawing a cached label block
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn pill_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &super::TextBlock,
        alpha: f32,
        primary: bool,
    ) {
        let hovered = self.hover == Some(hit);
        let r = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let (base, fill_alpha, fg) = if primary {
            (
                self.theme.action,
                if hovered { alpha * 0.86 } else { alpha },
                self.theme.action_text,
            )
        } else {
            (
                if hovered {
                    self.theme.border
                } else {
                    self.theme.track
                },
                alpha,
                self.theme.text_primary,
            )
        };
        let b = self.brush(target, base, fill_alpha);
        target.FillRoundedRectangle(&r, &b);
        let rect = D2D_RECT_F {
            left: x,
            top: y + 2.0,
            right: x + w,
            bottom: y + h - 2.0,
        };
        self.block_vc(target, label, &rect, fg, alpha);
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: x - 4.0,
                top: y - 4.0,
                right: x + w + 4.0,
                bottom: y + h + 4.0,
            },
        ));
    }
}

/// Masking rule regression: both sides of the 12-char threshold,
/// focused forcing all dots, short strings and middles leaking no plain text.
/// Also pins the poll preset values and count: value drift or adding a preset without
/// updating the label slots alarms here.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::i18n::Lang;
    use crate::ui::panel::layout::INTERVAL_PRESETS;
    use crate::ui::panel::mask_key;
    use crate::ui::panel::model::AccountView;

    #[test]
    fn interval_presets_pinned() {
        assert_eq!(INTERVAL_PRESETS, [60, 300, 900, 1800]);
    }

    /// Cache-key equivalence pins: identical inputs hit; a change in
    /// language, width, or account presence misses and forces a rebuild
    #[test]
    fn settings_key_equivalence() {
        let base = || PanelModel {
            strings: Lang::Zh.strings(),
            lang: Lang::Zh,
            snapshot: None,
            error: None,
            account: Some(AccountView {
                index: 0,
                name: "acc",
                platform: Platform::Cn,
                team: false,
            }),
            accounts_count: 1,
            popup_open: false,
            accounts: &[],
            poll_interval_secs: 300,
            language: None,
            appearance: None,
            autostart: false,
            threshold_enabled: true,
            threshold_percent: 80,
            reset_5h_enabled: true,
            reset_weekly_enabled: false,
            update_available: false,
            peak_range: (0, 0),
            peak_start_raw: "",
            peak_end_raw: "",
            update: None,
            news: None,
            last_news_read: None,
        };
        let key = SettingsKey {
            lang: Lang::Zh,
            threshold: 80,
            cw: 200.0f32.to_bits(),
            update: UpdateKey::Idle,
            account: Some(AccountKey {
                name: "acc".to_string(),
                platform: Platform::Cn,
                team: false,
                version: None,
                tier: None,
            }),
        };
        // Identical inputs → cache hit
        assert!(key.matches(&base(), 200.0));
        // Language switch → miss
        let mut m = base();
        m.lang = Lang::En;
        assert!(!key.matches(&m, 200.0));
        // Width change → miss
        assert!(!key.matches(&base(), 240.0));
        // Account removed (count 1 → 0) → miss; the reverse direction is
        // symmetric via the (Some, None) arm
        let mut m = base();
        m.account = None;
        m.accounts_count = 0;
        assert!(!key.matches(&m, 200.0));
    }

    /// Update-mapping single-source pin: key/model equivalence per reachable
    /// state; update_available drift or tag change must mismatch and rebuild
    #[test]
    fn update_key_state_equivalence() {
        let info = |tag: &str| crate::service::update::ReleaseInfo {
            tag: tag.to_string(),
            url: String::new(),
        };
        fn upd_model<'a>(
            update: Option<&'a Result<crate::service::update::ReleaseInfo, String>>,
            avail: bool,
        ) -> PanelModel<'a> {
            PanelModel {
                strings: Lang::Zh.strings(),
                lang: Lang::Zh,
                snapshot: None,
                error: None,
                account: Some(AccountView {
                    index: 0,
                    name: "acc",
                    platform: Platform::Cn,
                    team: false,
                }),
                accounts_count: 1,
                popup_open: false,
                accounts: &[],
                poll_interval_secs: 300,
                language: None,
                appearance: None,
                autostart: false,
                threshold_enabled: true,
                threshold_percent: 80,
                reset_5h_enabled: true,
                reset_weekly_enabled: false,
                update_available: avail,
                peak_range: (0, 0),
                peak_start_raw: "",
                peak_end_raw: "",
                update,
                news: None,
                last_news_read: None,
            }
        }
        let key = |update: UpdateKey| SettingsKey {
            lang: Lang::Zh,
            threshold: 80,
            cw: 200.0f32.to_bits(),
            update,
            account: Some(AccountKey {
                name: "acc".to_string(),
                platform: Platform::Cn,
                team: false,
                version: None,
                tier: None,
            }),
        };
        // Idle ↔ no result yet; a arrived result mismatches
        assert!(key(UpdateKey::Idle).matches(&upd_model(None, false), 200.0));
        assert!(!key(UpdateKey::Idle).matches(&upd_model(Some(&Ok(info("v1"))), false), 200.0));
        // Ok without availability ↔ up-to-date; availability on flips to New
        assert!(
            key(UpdateKey::OkNoUpdate).matches(&upd_model(Some(&Ok(info("v1"))), false), 200.0)
        );
        assert!(
            !key(UpdateKey::OkNoUpdate).matches(&upd_model(Some(&Ok(info("v1"))), true), 200.0)
        );
        // Err ↔ failed check
        assert!(key(UpdateKey::Err).matches(&upd_model(Some(&Err("net".into())), false), 200.0));
        assert!(!key(UpdateKey::Err).matches(&upd_model(Some(&Ok(info("v1"))), false), 200.0));
        // New ↔ same available tag; a different tag misses
        assert!(
            key(UpdateKey::New("v1".into()))
                .matches(&upd_model(Some(&Ok(info("v1"))), true), 200.0)
        );
        assert!(
            !key(UpdateKey::New("v1".into()))
                .matches(&upd_model(Some(&Ok(info("v9"))), true), 200.0)
        );
    }

    /// Preset count equals the render-side label slot count: adding a preset must sync
    /// both places; zip truncation alarms here
    #[test]
    fn interval_preset_count() {
        assert_eq!(INTERVAL_PRESETS.len(), 4);
    }

    #[test]
    fn mask_key_threshold() {
        // Under 12 chars: all dots, no head/tail trimming
        assert_eq!(mask_key("abc123", false), "••••••");
        // Exactly 12 chars: reveal the first and last 4
        assert_eq!(mask_key("abcd1234wxyz", false), "abcd…wxyz");
        // Over 12 chars: likewise reveal only the first and last 4
        assert_eq!(mask_key("abcd1234wxyz9999", false), "abcd…9999");
    }

    #[test]
    fn mask_key_active_full_dots() {
        // Focused forces per-character dots matching the original length.
        assert_eq!(mask_key("abcd1234wxyz9999", true), "•".repeat(16));
        assert_eq!(mask_key("abc", true), "•••");
    }

    #[test]
    fn mask_key_no_middle_plaintext() {
        // With 12+ chars the middle never lands on screen.
        let m = mask_key("abcdefghijkl", false);
        assert_eq!(m, "abcd…ijkl");
        assert!(!m.contains('e') && !m.contains('h'));
    }
}
