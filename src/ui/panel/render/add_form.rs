//! Add-account form (drawn inside the settings frame; labels come from the
//! settings label cache)

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::ID2D1HwndRenderTarget;

use super::settings::SettingsLabels;
use super::{Hit, Renderer, ScopeChoice, TextBlock};
use crate::api::Platform;
use crate::ui::panel::caret::FieldContent;
use crate::ui::panel::{InputField, Panel, layout};

impl Renderer {
    /// Add-account form: platform → type → name/key, team appends
    /// org/project IDs. Every input row (and its label, one
    /// SECTION_LABEL_H above) is pinned to a layout::ADD_*_Y constant —
    /// drawing, the caret, and the height formula share one source; only
    /// the head pickers advance an accumulation chain, stitched below.
    /// Labels come from the caller's frame caches (passed by reference —
    /// both were taken out of self).
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn draw_add_form(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        panel: &Panel,
        labels: &SettingsLabels,
        pad: f32,
        cw: f32,
        dy: f32,
        y: f32,
        alpha: f32,
    ) {
        let y = self.sub_label(target, &labels.sub_platform, pad, y, cw, alpha);
        let plats: [(Hit, &TextBlock); 2] = [
            (Hit::Platform(Platform::Cn), &labels.seg_platform_cn),
            (Hit::Platform(Platform::Intl), &labels.seg_platform_intl),
        ];
        let cur_plat = panel.pending_platform;
        let y = self.segmented_raw(
            target,
            &plats,
            |h| matches!(h, Hit::Platform(v) if cur_plat == *v),
            pad,
            y,
            cw,
            alpha,
        );
        let y = self.sub_label(target, &labels.sub_type, pad, y, cw, alpha);
        // The row set and y chain follow the effective layout.
        let team = panel.team.effective;
        let team_sel = panel.team.pending;
        let types: [(Hit, &TextBlock); 2] = [
            (
                Hit::AccountType(ScopeChoice::Personal),
                &labels.seg_personal,
            ),
            (Hit::AccountType(ScopeChoice::Team), &labels.seg_team),
        ];
        let y = self.segmented_raw(
            target,
            &types,
            |h| matches!(h, Hit::AccountType(v) if team_sel == matches!(*v, ScopeChoice::Team)),
            pad,
            y,
            cw,
            alpha,
        );
        // Stitch the accumulation chain to the constant chain: a drift
        // here paints every later row off its caret/height source (debug
        // only).
        debug_assert!(
            (dy + layout::ADD_NAME_Y - layout::SECTION_LABEL_H - y).abs() < 0.5,
            "add 表单 name 行 y 双链漂移"
        );
        let input = &panel.input;
        let lab_y = |add_y: f32| dy + add_y - layout::SECTION_LABEL_H;
        self.sub_label(
            target,
            &labels.sub_name,
            pad,
            lab_y(layout::ADD_NAME_Y),
            cw,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputName,
            dy + layout::ADD_NAME_Y,
            None,
            FieldContent::Plain(&input.name),
            "",
            input.field == Some(InputField::Name),
            alpha,
        );
        // The key never lands on screen in full: focused shows monospace dots,
        // unfocused with 12+ chars reveals only the first and last 4, and
        // the in-field eye toggles plain text. Both focus states carry the
        // masked parts when unrevealed — the display string materializes
        // only on a cache miss, and the plaintext never enters the
        // generic content parameter, so any future branch drawing
        // `content` verbatim can only ever leak dots.
        let key_active = input.field == Some(InputField::Key);
        let key_content = if panel.key_revealed {
            FieldContent::Plain(&input.key)
        } else {
            FieldContent::Masked {
                key: &input.key,
                active: key_active,
            }
        };
        self.sub_label(
            target,
            &labels.sub_key,
            pad,
            lab_y(layout::ADD_KEY_Y),
            cw,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputKey,
            dy + layout::ADD_KEY_Y,
            Some((Hit::RevealKey, panel.key_revealed)),
            key_content,
            "",
            key_active,
            alpha,
        );
        // Tail constants are shared with add_page_height: the button row
        // sits exactly where the height formula stops.
        let by = if team {
            // Team: org / project IDs (request headers Bigmodel-Organization / Bigmodel-Project)
            self.sub_label(
                target,
                &labels.sub_org,
                pad,
                lab_y(layout::ADD_ORG_Y),
                cw,
                alpha,
            );
            self.input_field(
                panel,
                target,
                Hit::InputOrg,
                dy + layout::ADD_ORG_Y,
                None,
                FieldContent::Plain(&input.org),
                "",
                input.field == Some(InputField::Org),
                alpha,
            );
            self.sub_label(
                target,
                &labels.sub_project,
                pad,
                lab_y(layout::ADD_PROJECT_Y),
                cw,
                alpha,
            );
            self.input_field(
                panel,
                target,
                Hit::InputProject,
                dy + layout::ADD_PROJECT_Y,
                None,
                FieldContent::Plain(&input.project),
                "",
                input.field == Some(InputField::Project),
                alpha,
            );
            dy + layout::ADD_PROJECT_Y + layout::INPUT_H + layout::ADD_TEAM_TAIL
        } else {
            dy + layout::ADD_KEY_Y + layout::INPUT_H + layout::INPUT_GAP + layout::ADD_PERSONAL_TAIL
        };
        let pair_w = 88.0 * 2.0 + 12.0;
        let bx = pad + (cw - pair_w) / 2.0;
        self.pill_button(
            target,
            Hit::SaveAccount,
            bx,
            by,
            88.0,
            layout::SAVE_BTN_H,
            &labels.pill_save,
            alpha,
            true,
        );
        self.pill_button(
            target,
            Hit::Back,
            bx + 100.0,
            by,
            88.0,
            layout::SAVE_BTN_H,
            &labels.pill_cancel,
            alpha,
            false,
        );
    }
}
