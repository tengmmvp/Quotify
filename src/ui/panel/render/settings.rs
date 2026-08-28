//! 设置视图与添加账号表单

#![allow(unsafe_op_in_unsafe_fn)]

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{D2D1_ROUNDED_RECT, ID2D1HwndRenderTarget};

use super::{Align, AppearanceChoice, Hit, LanguageChoice, Renderer, ScopeChoice};
use crate::api::Platform;
use crate::ui::panel::model::PanelModel;
use crate::ui::panel::theme::RADIUS;
use crate::ui::panel::{InputField, Panel, layout};

impl Renderer {
    /// 设置视图
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
        let mut y = dy + layout::SETTINGS_EDGE_PAD;

        // ── 导航栏 ──
        if !panel.adding_account {
            self.back_arrow(target, Hit::Back, pad, y + 6.0);
        }
        self.x_button(target, Hit::ClosePanel, w - pad - 10.0, y + 6.0);
        let nav_title = if panel.adding_account {
            s.add_account
        } else {
            s.settings
        };
        let title_rect = D2D_RECT_F {
            left: pad,
            top: y,
            right: w - pad,
            bottom: y + 26.0,
        };
        self.text_aligned(
            target,
            nav_title,
            &title_rect,
            16.0,
            400,
            self.theme.text_primary,
            alpha,
            Align::Center,
            false,
        );
        y += layout::NAV_H;

        // ── 账号：设置页首项即数据来源；添加页此处标题用「账号信息」──
        let section = if panel.adding_account {
            s.platform_section
        } else {
            s.accounts_section
        };
        y = self.section_label(target, section, pad, y, w, alpha, false);
        if panel.adding_account {
            // 添加流程：平台 → 类型 → 名称/key，团队版追加组织/项目 ID
            y = self.sub_label(target, s.account_platform, pad, y, cw, alpha);
            let plats: [(Hit, &str); 2] = [
                (Hit::Platform(Platform::Cn), s.platform_cn),
                (Hit::Platform(Platform::Intl), s.platform_intl),
            ];
            let cur_plat = panel.pending_platform;
            y = self.segmented_raw(
                target,
                &plats,
                |h| matches!(h, Hit::Platform(v) if cur_plat == *v),
                pad,
                y,
                cw,
                alpha,
            );
            y = self.sub_label(target, s.account_type_label, pad, y, cw, alpha);
            let team = panel.pending_team;
            let types: [(Hit, &str); 2] = [
                (Hit::AccountType(ScopeChoice::Personal), s.type_personal),
                (Hit::AccountType(ScopeChoice::Team), s.type_team),
            ];
            y = self.segmented_raw(
                target,
                &types,
                |h| matches!(h, Hit::AccountType(v) if team == matches!(*v, ScopeChoice::Team)),
                pad,
                y,
                cw,
                alpha,
            );
            // y 钉在 layout 常量：绘制、光标、高度公式三方同源
            let input = &panel.input;
            self.sub_label(target, s.account_name, pad, y, cw, alpha);
            let name_y = dy + layout::ADD_NAME_Y;
            self.input_field(
                panel,
                target,
                Hit::InputName,
                layout::INPUT_X,
                name_y,
                cw,
                None,
                &input.name,
                "",
                input.field == Some(InputField::Name),
                alpha,
            );
            y = name_y + layout::INPUT_H + layout::INPUT_GAP;
            self.sub_label(target, s.api_key_label, pad, y, cw, alpha);
            let key_y = dy + layout::ADD_KEY_Y;
            // key 不整串落屏：聚焦显等宽圆点，未聚焦且足 12 位只露首尾各 4 位；框内眼睛可切明文
            let key_active = input.field == Some(InputField::Key);
            let key_disp = if panel.key_revealed {
                input.key.clone()
            } else {
                mask_key(&input.key, key_active)
            };
            self.input_field(
                panel,
                target,
                Hit::InputKey,
                layout::INPUT_X,
                key_y,
                cw,
                Some((Hit::RevealKey, panel.key_revealed)),
                &key_disp,
                "",
                key_active,
                alpha,
            );
            y = key_y + layout::INPUT_H + layout::INPUT_GAP;
            if team {
                // 团队版：组织 / 项目 ID（请求头 Bigmodel-Organization / Bigmodel-Project）
                self.sub_label(target, s.org_id_label, pad, y, cw, alpha);
                let org_y = dy + layout::ADD_ORG_Y;
                self.input_field(
                    panel,
                    target,
                    Hit::InputOrg,
                    layout::INPUT_X,
                    org_y,
                    cw,
                    None,
                    &input.org,
                    "",
                    input.field == Some(InputField::Org),
                    alpha,
                );
                y = org_y + layout::INPUT_H + layout::INPUT_GAP;
                self.sub_label(target, s.project_id_label, pad, y, cw, alpha);
                let project_y = dy + layout::ADD_PROJECT_Y;
                self.input_field(
                    panel,
                    target,
                    Hit::InputProject,
                    layout::INPUT_X,
                    project_y,
                    cw,
                    None,
                    &input.project,
                    "",
                    input.field == Some(InputField::Project),
                    alpha,
                );
                y = project_y + layout::INPUT_H + 12.0;
            } else {
                y += 6.0;
            }
            let pair_w = 88.0 * 2.0 + 12.0;
            let bx = pad + (cw - pair_w) / 2.0;
            self.pill_button(
                target,
                Hit::SaveAccount,
                bx,
                y,
                88.0,
                30.0,
                s.save,
                alpha,
                true,
            );
            self.pill_button(
                target,
                Hit::Back,
                bx + 100.0,
                y,
                88.0,
                30.0,
                s.cancel,
                alpha,
                false,
            );
            return;
        }
        // 当前活跃账号卡片（有则显示）；添加入口常驻，支持继续添加账号
        if let Some(acc) = model.account {
            let platform = if acc.platform == Platform::Cn {
                s.platform_cn
            } else {
                s.platform_intl
            };
            // 团队版在平台名牌并列标注——账号卡无编辑入口，保存后仍可辨识
            let platform_owned;
            let platform = if acc.team {
                platform_owned = format!("{platform} | {}", s.team_badge);
                &platform_owned
            } else {
                platform
            };
            // 版本 / 等级来自用量数据，无数据时占位
            let (version, tier) = match model.snapshot {
                Some(snap) => {
                    let t = snap.tier.label();
                    let tier = if t.is_empty() {
                        snap.plan_label.clone().unwrap_or_else(|| "—".into())
                    } else {
                        t.to_string()
                    };
                    (snap.plan_version.label().to_string(), tier)
                }
                None => ("—".to_string(), "—".to_string()),
            };
            self.account_card(
                target,
                Hit::RemoveAccount(acc.index),
                acc.name,
                platform,
                &version,
                &tier,
                pad,
                y,
                cw,
                alpha,
            );
            y += layout::ACCOUNT_CARD_H;
            if matches!(model.error, Some(crate::api::FetchError::Auth)) {
                self.text(
                    target,
                    s.key_invalid,
                    pad + 2.0,
                    y - 4.0,
                    cw,
                    16.0,
                    12.0,
                    400,
                    self.theme.danger,
                    alpha,
                );
                y += layout::AUTH_ERROR_H;
            }
        }
        // 添加账号独占一行：占满内容区，视觉对称
        self.pill_button(
            target,
            Hit::AddAccount,
            pad,
            y + 2.0,
            cw,
            30.0,
            s.add_account,
            alpha,
            false,
        );
        y += layout::ADD_BTN_ROW_H;

        // ── 轮询间隔：分段第 5 段「自定义」，选中时下方展开输入行 ──
        y = self.section_label(target, s.poll_interval, pad, y, w, alpha, true);
        let presets: [(Hit, &str); 5] = [
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[0]),
                s.interval_1m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[1]),
                s.interval_5m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[2]),
                s.interval_15m,
            ),
            (
                Hit::IntervalPreset(layout::INTERVAL_PRESETS[3]),
                s.interval_30m,
            ),
            (Hit::CustomizeInterval, s.interval_custom),
        ];
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
        if panel.customizing_interval {
            // y 钉在 layout::interval_input_y，与光标、高度公式同源
            let iy = dy + layout::interval_input_y(model.accounts_count > 0, panel.account_error);
            let input = &panel.input;
            self.input_field(
                panel,
                target,
                Hit::InputInterval,
                layout::INPUT_X,
                iy,
                96.0,
                None,
                &input.interval,
                "",
                input.field == Some(InputField::Interval),
                alpha,
            );
            self.text(
                target,
                s.interval_custom_unit,
                pad + 104.0,
                iy + 6.0,
                40.0,
                16.0,
                12.0,
                400,
                self.theme.text_secondary,
                alpha,
            );
            self.outline_button(
                target,
                Hit::ApplyInterval,
                w - pad - 56.0,
                iy - 1.0,
                56.0,
                28.0,
                s.apply,
                alpha,
            );
            y = iy + layout::CUSTOMIZE_EXTRA_H;
        }

        // ── 通知 ──
        y = self.section_label(target, s.notifications, pad, y, w, alpha, true);
        let threshold_desc = s
            .notify_threshold_desc
            .replace("{p}", &model.threshold_percent.to_string());
        y = self.toggle_row(
            target,
            Hit::ToggleThreshold,
            s.notify_threshold,
            &threshold_desc,
            model.threshold_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleReset5h,
            s.notify_reset_5h_opt,
            s.notify_reset_5h_desc,
            model.reset_5h_enabled,
            pad,
            y,
            cw,
            alpha,
        );
        y = self.toggle_row(
            target,
            Hit::ToggleResetWeekly,
            s.notify_reset_weekly_opt,
            s.notify_reset_weekly_desc,
            model.reset_weekly_enabled,
            pad,
            y,
            cw,
            alpha,
        );

        // ── 高峰区间：工作日生效，自定义起止 ──
        self.section_label(target, s.peak_section, pad, y, w, alpha, true);
        // y 钉在 layout::peak_input_y，与光标、高度公式同源
        let pky = dy
            + layout::peak_input_y(
                model.accounts_count > 0,
                panel.account_error,
                panel.customizing_interval,
            );
        // 当前配置值作为弱色占位提示，输入即覆盖，无需删除
        let start_buf = panel.input.peak_start.as_str();
        let end_buf = panel.input.peak_end.as_str();
        // 非法状态（格式错误或两端相等）给红框，提示等待修正；空缓冲表示未编辑不判
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
        self.text(
            target,
            s.peak_start_label,
            pad,
            pky + 7.0,
            26.0,
            14.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputPeakStart,
            layout::PEAK_START_X,
            pky,
            64.0,
            None,
            start_buf,
            model.peak_start_raw,
            panel.input.field == Some(InputField::PeakStart),
            alpha,
        );
        self.text(
            target,
            s.peak_end_label,
            pad + 100.0,
            pky + 7.0,
            26.0,
            14.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.input_field(
            panel,
            target,
            Hit::InputPeakEnd,
            layout::PEAK_END_X,
            pky,
            64.0,
            None,
            end_buf,
            model.peak_end_raw,
            panel.input.field == Some(InputField::PeakEnd),
            alpha,
        );
        self.outline_button(
            target,
            Hit::ApplyPeak,
            w - pad - 48.0,
            pky - 1.0,
            48.0,
            28.0,
            s.apply,
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
        y = pky + layout::INPUT_H + layout::PEAK_TAIL_GAP;

        // ── 通用：语言 / 外观 / 开机自启 ──
        y = self.section_label(target, s.settings_general, pad, y, w, alpha, true);
        y = self.sub_label(target, s.language, pad, y, cw, alpha);
        let langs: [(Hit, &str); 3] = [
            (Hit::Language(LanguageChoice::System), s.follow_system),
            (Hit::Language(LanguageChoice::Zh), "中文"),
            (Hit::Language(LanguageChoice::En), "English"),
        ];
        // 配置字符串 → 选项枚举：未配置 / 未知值都归「跟随系统」
        let cur_lang = match model.language {
            Some("zh") => LanguageChoice::Zh,
            Some("en") => LanguageChoice::En,
            _ => LanguageChoice::System,
        };
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
        y = self.sub_label(target, s.appearance_section, pad, y, cw, alpha);
        let themes: [(Hit, &str); 3] = [
            (Hit::Appearance(AppearanceChoice::System), s.follow_system),
            (Hit::Appearance(AppearanceChoice::Light), s.theme_light),
            (Hit::Appearance(AppearanceChoice::Dark), s.theme_dark),
        ];
        let cur_theme = match model.appearance {
            Some("light") => AppearanceChoice::Light,
            Some("dark") => AppearanceChoice::Dark,
            _ => AppearanceChoice::System,
        };
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
        y = self.toggle_row(
            target,
            Hit::ToggleAutostart,
            s.autostart,
            "",
            model.autostart,
            pad,
            y,
            cw,
            alpha,
        );

        // ── 网络代理：地址留空直连，未配置时框内只显占位提示 ──
        y = self.section_label(target, s.network_section, pad, y, w, alpha, true);
        self.sub_label(target, s.proxy_label, pad, y, cw, alpha);
        // y 钉在 layout::proxy_input_y，与光标、高度公式同源
        let py = dy
            + layout::proxy_input_y(
                model.accounts_count > 0,
                panel.account_error,
                panel.customizing_interval,
            );
        self.input_field(
            panel,
            target,
            Hit::InputProxy,
            layout::INPUT_X,
            py,
            cw,
            None,
            &panel.input.proxy,
            s.proxy_hint,
            panel.input.field == Some(InputField::Proxy),
            alpha,
        );
        y = py + layout::INPUT_H + layout::PROXY_TAIL_GAP;

        // ── 配置管理：导出 / 导入，位于关于区之前 ──
        y = self.section_label(target, s.backup_section, pad, y, w, alpha, true);
        let pair_w = 104.0 * 2.0 + 12.0;
        let bx = pad + (cw - pair_w) / 2.0;
        self.outline_button(
            target,
            Hit::ExportConfig,
            bx,
            y,
            104.0,
            28.0,
            s.export_config,
            alpha,
        );
        self.outline_button(
            target,
            Hit::ImportConfig,
            bx + 116.0,
            y,
            104.0,
            28.0,
            s.import_config,
            alpha,
        );
        y += layout::BACKUP_ROW_H;

        // ── 关于：检查更新 + 版本，位于底部 ──
        let update_label = match model.update {
            Some(Ok(info))
                if crate::service::update::is_newer(&info.tag, env!("CARGO_PKG_VERSION")) =>
            {
                format!("{} · {}", s.check_update, info.tag)
            }
            Some(Ok(_)) => s.up_to_date.into(),
            Some(Err(_)) => s.err_update.into(),
            None => s.check_update.into(),
        };
        y = self.section_label(target, "", pad, y, w, alpha, true);
        // 版本行收尾：有新版时文字带「当前 → 最新」、按钮换「前往下载」
        let (btn_hit, btn_label) = if model.update_available {
            (Hit::OpenDownload, s.go_download)
        } else {
            (Hit::CheckUpdate, update_label.as_str())
        };
        let ver_line = if model.update_available
            && let Some(Ok(info)) = model.update
        {
            s.version_new
                .replace("{cur}", env!("CARGO_PKG_VERSION"))
                .replace("{new}", info.tag.trim_start_matches('v'))
        } else {
            s.version_label.replace("{v}", env!("CARGO_PKG_VERSION"))
        };
        let btn_w = (self.measure(btn_label, 12.0, 400, false) + 28.0).max(104.0);
        self.text(
            target,
            &ver_line,
            pad,
            y + 7.0,
            w - pad * 2.0 - btn_w - 12.0,
            16.0,
            12.0,
            400,
            self.theme.text_tertiary,
            alpha,
        );
        self.outline_button(
            target,
            btn_hit,
            w - pad - btn_w,
            y + 1.0,
            btn_w,
            28.0,
            btn_label,
            alpha,
        );
    }

    // ── 设置页专属小件 ──

    /// 区块主标题：text_primary 色强调块 + 标题；子标签不带块以区分层级
    #[allow(clippy::too_many_arguments)]
    unsafe fn section_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
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
        if !label.is_empty() {
            let bar = self.brush(target, self.theme.text_primary, alpha * 0.9);
            target.FillRectangle(
                &D2D_RECT_F {
                    left: x,
                    top: ny + 1.0,
                    right: x + 3.0,
                    bottom: ny + 14.0,
                },
                &bar,
            );
            self.text(
                target,
                label,
                x + 7.0,
                ny,
                _w - 7.0,
                17.0,
                13.0,
                600,
                self.theme.text_tertiary,
                alpha,
            );
            ny + layout::SECTION_LABEL_H
        } else {
            // 纯分隔无标题，只留少量空隙给紧随内容
            ny + 6.0
        }
    }

    /// 区块内子项标签：13/400 次级色、无强调块
    /// 字阶：16 导航 / 13·500 主标题 / 13·400 控件标签 / 12·400 描述
    unsafe fn sub_label(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        label: &str,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        self.text(
            target,
            label,
            x,
            y + 1.0,
            w,
            17.0,
            13.0,
            400,
            self.theme.text_secondary,
            alpha,
        );
        y + layout::SECTION_LABEL_H
    }

    /// 自绘输入框：光标用系统 CreateCaret，IME 组合窗随光标定位。
    /// 激活态按光标可视窗口绘制并高亮选区，失焦态画尾部切片。
    #[allow(clippy::too_many_arguments)]
    unsafe fn input_field(
        &mut self,
        panel: &Panel,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        eye: Option<(Hit, bool)>,
        content: &str,
        placeholder: &str,
        active: bool,
        alpha: f32,
    ) {
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
        let tail = if eye.is_some() { 26.0 } else { 4.0 };
        let text_rect = D2D_RECT_F {
            left: x + 6.0,
            top: y + 6.0,
            right: x + w - tail,
            bottom: y + 22.0,
        };
        if content.is_empty() && !placeholder.is_empty() {
            self.text_rect_opts(
                target,
                placeholder,
                &text_rect,
                12.0,
                400,
                self.theme.text_tertiary,
                alpha,
                Align::Left,
                false,
            );
        } else if active {
            let avail = (w - 6.0 - tail).max(1.0);
            let cl = panel.caret_layout(self);
            let chars: Vec<char> = content.chars().collect();
            let start = cl.vis_start.min(chars.len());
            let mut end = start;
            while end < chars.len() && cl.seg(start, end + 1) <= avail {
                end += 1;
            }
            let vis: String = chars[start..end].iter().collect();
            if let Some((a, b)) = panel.input.edit.selection() {
                let a = a.min(chars.len());
                let b = b.min(chars.len());
                let sx = |n: usize| cl.seg(start, n);
                let sel_left = x + 6.0 + sx(a);
                let sel_right = x + 6.0 + sx(b);
                if sel_right > x + 6.0 && sel_left < x + w - tail {
                    let hl = self.brush(target, self.theme.action, alpha * 0.16);
                    target.FillRectangle(
                        &D2D_RECT_F {
                            left: sel_left.max(x + 6.0),
                            top: y + 5.0,
                            right: sel_right.min(x + w - tail),
                            bottom: y + 21.0,
                        },
                        &hl,
                    );
                }
            }
            self.text_rect_opts(
                target,
                &vis,
                &text_rect,
                12.0,
                400,
                self.theme.text_primary,
                alpha,
                Align::Left,
                true,
            );
        } else {
            // 失焦态：尾部可视切片取最长可放入后缀；前缀宽表一次成型，
            // 后缀宽 = 全宽 - 前缀宽，零逐候选测量
            let avail = (w - 6.0 - tail).max(1.0);
            let chars: Vec<char> = content.chars().collect();
            let mut vis = String::new();
            if !chars.is_empty() {
                let widths = self.prefix_widths(&chars, 12.0, 400, true);
                let n = chars.len();
                if widths[n] <= avail {
                    vis = content.to_string();
                } else {
                    // 至少保尾 1 字符；找最小起点使后缀入宽[后缀宽随
                    // 起点减小单调增，首个溢出前一位即最长可放入后缀]
                    let mut k = 0usize;
                    while k + 1 < n && widths[n] - widths[k] > avail {
                        k += 1;
                    }
                    vis = chars[k..].iter().collect();
                }
            }
            self.text_rect_opts(
                target,
                &vis,
                &text_rect,
                12.0,
                400,
                self.theme.text_primary,
                alpha,
                Align::Left,
                true,
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
            // 眼睛命中区先于整框登记：hit_at 取首个命中，后登记会被整框吞掉
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
                bottom: y + 30.0,
            },
        ));
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn toggle_row(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        title: &str,
        desc: &str,
        on: bool,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) -> f32 {
        self.text(
            target,
            title,
            x,
            y + 1.0,
            w - 56.0,
            18.0,
            13.0,
            400,
            self.theme.text_primary,
            alpha,
        );
        let mut ty = y + 19.0;
        if !desc.is_empty() {
            self.text(
                target,
                desc,
                x,
                ty,
                w - 56.0,
                14.0,
                12.0,
                400,
                self.theme.text_tertiary,
                alpha,
            );
            ty += 14.0;
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
        // 命中区只覆盖开关本体并含 8px 容差——点击行文字/空白不翻转
        self.hits.push((
            hit,
            D2D_RECT_F {
                left: tx - 8.0,
                top: cy - 6.0,
                right: tx + tw + 8.0,
                bottom: cy + th + 6.0,
            },
        ));
        ty + 9.0
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn segmented_raw(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        items: &[(Hit, &str)],
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
            self.text_aligned_vc(
                target,
                label,
                &rect,
                13.0,
                400,
                color,
                alpha,
                Align::Center,
                false,
            );
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

    /// 单账号卡片
    #[allow(clippy::too_many_arguments)]
    unsafe fn account_card(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        remove: Hit,
        name: &str,
        platform: &str,
        version: &str,
        tier: &str,
        x: f32,
        y: f32,
        w: f32,
        alpha: f32,
    ) {
        let h = 40.0;
        let card = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            radiusX: RADIUS,
            radiusY: RADIUS,
        };
        let fill = self.brush(target, self.theme.track, alpha);
        target.FillRoundedRectangle(&card, &fill);
        let edge = self.brush(target, self.theme.border, alpha * 0.8);
        target.DrawRoundedRectangle(&card, &edge, 1.0, None);
        // 徽标与关闭钮居右对齐，名称吃左侧剩余空间；从右界向左依次排 tier/version/platform
        let by = y + (h - 17.0) / 2.0;
        let mut bx = x + w - 56.0; // 徽标区右界，给右侧 × 留位
        if tier != "—" {
            let tw = self.measure(tier, 10.5, 400, false) + 14.0;
            bx -= tw;
            let (edge, fg) = self.tier_badge_colors(tier);
            self.badge(target, tier, bx, by, tw, edge, fg, alpha, false);
            bx -= 6.0;
        }
        if version != "—" {
            let vw = self.measure(version, 10.5, 400, true) + 14.0;
            bx -= vw;
            self.badge(
                target,
                version,
                bx,
                by,
                vw,
                self.theme.border,
                self.theme.text_secondary,
                alpha,
                true,
            );
            bx -= 6.0;
        }
        let pw = self.measure(platform, 10.5, 400, false) + 14.0;
        bx -= pw;
        self.badge(
            target,
            platform,
            bx,
            by,
            pw,
            self.theme.border,
            self.theme.text_secondary,
            alpha,
            false,
        );
        let name_max = (bx - 10.0 - (x + 12.0)).max(60.0);
        let (name_disp, _) = self.ellipsize(name, 15.0, name_max, 500, false);
        self.text_aligned_vc(
            target,
            &name_disp,
            &D2D_RECT_F {
                left: x + 12.0,
                top: y,
                right: x + 12.0 + name_max,
                bottom: y + h,
            },
            15.0,
            500,
            self.theme.text_primary,
            alpha,
            Align::Left,
            false,
        );
        self.x_button(target, remove, x + w - 24.0, y + 15.0);
    }

    /// 等级配色墨阶梯度：Max 最深、Pro 次之、Lite 最浅
    fn tier_badge_colors(&self, tier: &str) -> ([f32; 4], [f32; 4]) {
        let t = tier.trim().to_ascii_lowercase();
        if t.contains("max") {
            (self.theme.text_primary, self.theme.text_primary)
        } else if t.contains("pro") {
            (self.theme.text_secondary, self.theme.text_secondary)
        } else {
            (self.theme.border, self.theme.text_tertiary)
        }
    }

    /// 小圆角填充按钮
    #[allow(clippy::too_many_arguments)]
    unsafe fn pill_button(
        &mut self,
        target: &ID2D1HwndRenderTarget,
        hit: Hit,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: &str,
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
            top: y + 5.0,
            right: x + w,
            bottom: y + h - 4.0,
        };
        self.text_aligned(
            target,
            label,
            &rect,
            13.0,
            400,
            fg,
            alpha,
            Align::Center,
            false,
        );
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

/// key 掩码显示串：聚焦或不足 12 位时逐字符圆点，圆点数与原串一致，
/// 光标步宽仍对且短串不露明文；未聚焦且足 12 位只露首尾各 4 位
fn mask_key(key: &str, active: bool) -> String {
    let n = key.chars().count();
    if active || n < 12 {
        "•".repeat(n)
    } else {
        let head: String = key.chars().take(4).collect();
        let tail: String = key.chars().skip(n - 4).collect();
        format!("{head}…{tail}")
    }
}

/// 掩码规则回归：12 位阈值两侧、聚焦强制全显圆点、短串与中段不露明文
#[cfg(test)]
mod tests {
    use super::mask_key;

    #[test]
    fn mask_key_threshold() {
        // 不足 12 位：整串圆点，不截首尾
        assert_eq!(mask_key("abc123", false), "••••••");
        // 恰 12 位：露首尾各 4 位
        assert_eq!(mask_key("abcd1234wxyz", false), "abcd…wxyz");
        // 超 12 位：同样只露首尾 4 位
        assert_eq!(mask_key("abcd1234wxyz9999", false), "abcd…9999");
    }

    #[test]
    fn mask_key_active_full_dots() {
        // 聚焦强制逐字符圆点，位数与原串一致，光标步宽可对
        assert_eq!(mask_key("abcd1234wxyz9999", true), "•".repeat(16));
        assert_eq!(mask_key("abc", true), "•••");
    }

    #[test]
    fn mask_key_no_middle_plaintext() {
        // 12 位以上中段不落屏
        let m = mask_key("abcdefghijkl", false);
        assert_eq!(m, "abcd…ijkl");
        assert!(!m.contains('e') && !m.contains('h'));
    }
}
