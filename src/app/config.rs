//! Configuration persistence

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::Platform;
use crate::service::poller::DEFAULT_INTERVAL_SECS;

/// Global preferences
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// Polling interval (seconds)
    pub poll_interval_secs: u64,
    /// UI language; None = follow the system
    pub language: Option<String>,
    /// Appearance; None = follow the system, accepts "light" / "dark"
    pub appearance: Option<String>,
    /// Proxy address; None = direct connection, e.g. "http://host:port" / "socks5://host:port"
    pub proxy: Option<String>,
    /// Usage threshold alert toggle
    pub notify_threshold_enabled: bool,
    /// Alert threshold percentage
    pub notify_threshold_percent: u8,
    /// 5-hour window reset notification
    pub notify_reset_5h_enabled: bool,
    /// Weekly quota reset notification
    pub notify_reset_weekly_enabled: bool,
    /// Peak window start HH:MM
    pub peak_start: String,
    /// Peak window end HH:MM
    pub peak_end: String,
    /// Date of the last read repo news item (YYYY-MM-DD)
    pub last_news_read: Option<String>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            poll_interval_secs: DEFAULT_INTERVAL_SECS,
            language: None,
            appearance: None,
            proxy: None,
            notify_threshold_enabled: false,
            notify_threshold_percent: 80,
            notify_reset_5h_enabled: false,
            notify_reset_weekly_enabled: false,
            peak_start: "14:00".into(),
            peak_end: "18:00".into(),
            last_news_read: None,
        }
    }
}

/// One monitored GLM account
/// Field order mirrors the add-account form (name → platform → type →
/// org → project) with credentials last; this order is also the
/// config.toml write-out order
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Stable id
    pub id: String,
    /// Display name
    pub name: String,
    /// Site: CN open.bigmodel.cn / international api.z.ai
    pub platform: Platform,
    /// Team-edition flag, CN site only; requests carry `?type=2` + the org/project selection headers
    #[serde(default)]
    pub team: bool,
    /// Team edition: value of the `Bigmodel-Organization` header
    #[serde(default)]
    pub org_id: String,
    /// Team edition: value of the `Bigmodel-Project` header
    #[serde(default)]
    pub project_id: String,
    /// API key
    pub api_key: String,
}

/// Account-count cap: an imported file is untrusted, and an oversized list
/// would inflate every account surface (popup rows, settings cards); over
/// the cap the import is rejected outright, never truncated — silently
/// dropped entries would pass an import off as successful.
pub(crate) const MAX_ACCOUNTS: usize = 100;

/// Full application configuration
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub accounts: Vec<Account>,
    pub selected: Option<String>,
}

impl Config {
    /// Generates an account id: session entropy plus an in-process
    /// counter. The two cover everything — the counter distinguishes
    /// remove-then-add within the same millisecond, the entropy
    /// distinguishes ids across sessions and imports (an imported id is
    /// reused as-is, never regenerated), and stacking timestamp or
    /// sub-second parts on top only lengthens what lands in the user's
    /// config file.
    pub fn new_account_id(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        static SESSION: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let entropy = *SESSION.get_or_init(|| {
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        });
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        format!("acc_{entropy:x}{seq:x}")
    }

    /// The selected account and its index: looked up by selected, falling
    /// back to the first entry when missing. The single implementation of
    /// the selection-fallback semantics, shared by the polling target and
    /// the presentation layer.
    pub fn selected_account(&self) -> Option<(usize, &Account)> {
        let sel = self.selected.as_deref()?;
        self.accounts
            .iter()
            .enumerate()
            .find(|(_, a)| a.id == sel)
            .or_else(|| self.accounts.first().map(|a| (0, a)))
    }

    /// selected missing or dangling backfills the first account: shared by
    /// normalization and account removal. While dangling, the polling
    /// target still works through selected_account's first-entry fallback,
    /// but the config would keep persisting a dead reference.
    pub(crate) fn backfill_selected(&mut self) {
        let valid = self
            .accounts
            .iter()
            .any(|a| self.selected.as_deref() == Some(a.id.as_str()));
        if !valid {
            self.selected = self.accounts.first().map(|a| a.id.clone());
        }
    }
}

/// Config file path
pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.toml")
}

/// Template written on first launch: optional keys are shown as commented
/// examples — assigning an empty string would differ from Default's None
const TEMPLATE: &str = r#"# Quotify 配置文件
# 通常通过面板内「设置」页管理，无需手动编辑；改完重启生效。
# API key 明文保存在本机，请勿分享此文件。

[general]
# 轮询间隔（秒）
poll_interval_secs = 300
# 界面语言：留空跟随系统，可设 "zh" 或 "en"
# language = ""
# 外观：留空跟随系统，可设 "light" / "dark"
# appearance = ""
# 网络代理：留空直连，可设 "http://host:port" 或 "socks5://host:port"
# proxy = ""
# 用量阈值预警（默认关闭）
notify_threshold_enabled = false
notify_threshold_percent = 80
# 重置提醒（默认关闭）
notify_reset_5h_enabled = false
notify_reset_weekly_enabled = false
# 高峰区间（工作日生效），HH:MM 格式
peak_start = "14:00"
peak_end = "18:00"
# 已读仓库动态的日期
# last_news_read = ""

# 受监控的账号（建议在设置页添加；此处仅为字段示例）
# [[accounts]]
# id = "acc_demo"
# name = "Demo"
# platform = "cn"          # cn = 国内版 open.bigmodel.cn；intl = 国际版 api.z.ai
# team = false             # 团队版（仅国内站）：true 时下面两项必填
# org_id = ""              # 团队版：组织 ID（bigmodel-organization 请求头）
# project_id = ""          # 团队版：项目 ID（bigmodel-project 请求头）
# api_key = "你的 API key"

# 当前选中的账号 id
# selected = ""
"#;

/// Parses config text: on corruption the broken file is renamed for the
/// record and the default config is used as fallback
fn parse_or_default(text: &str, path: &Path) -> (Config, bool) {
    toml::from_str(text)
        .map(|c| (c, false))
        .unwrap_or_else(|e| {
            let pos = e
                .span()
                .map(|s| {
                    let before = text.get(..s.start.min(text.len())).unwrap_or("");
                    let line = before.matches('\n').count() + 1;
                    let col = before
                        .rsplit('\n')
                        .next()
                        .map(|l| l.chars().count() + 1)
                        .unwrap_or(1);
                    format!("（第 {line} 行第 {col} 列）")
                })
                .unwrap_or_default();
            crate::platform::log(&format!(
                "config.toml 解析失败，使用默认配置{pos}: {}",
                e.message()
            ));
            backup_broken(path);
            (Config::default(), true)
        })
}

/// Renames the broken file to .bak for the record before falling back to
/// defaults, so the user's hand-edited content remains retrievable
fn backup_broken(path: &Path) {
    if path.exists() {
        let bak = path.with_extension("toml.bak");
        if let Err(e) = std::fs::rename(path, &bak) {
            crate::platform::log(&format!("config.toml 改名 .bak 失败: {e}"));
            return;
        }
        crate::platform::secure_file_acl(&bak);
    }
}

/// Loads the config: parse failure keeps the file for the record and
/// falls back to defaults; a missing file writes the template; any other
/// read failure only falls back to defaults without touching disk.
pub fn load() -> Config {
    let path = config_path();
    // Clean up the temp file left behind when the previous process was
    // killed between write → rename.
    let _ = std::fs::remove_file(path.with_extension("toml.tmp"));
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            // Files written by older versions never had their ACL tightened;
            // catch up once for all at startup [idempotent].
            crate::platform::secure_file_acl(&path);
            let (cfg, broken) = parse_or_default(&text, &path);
            if broken {
                let s = crate::ui::i18n::detect_system_lang().strings();
                crate::platform::notify::show(super::notify::NOTIFY_TITLE, s.notify_config_broken);
            }
            cfg
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // The template itself carries no key, but the user may fill one
            // in by hand: tighten at creation — no window where the file
            // lands on disk first and gets its ACL changed afterwards.
            if let Err(e) = crate::platform::write_private(&path, TEMPLATE) {
                crate::platform::log(&format!("config.toml 模板写入失败: {e}"));
            }
            Config::default()
        }
        Err(e) => {
            crate::platform::log(&format!("config.toml 读取失败，本次用默认配置: {e}"));
            Config::default()
        }
    }
}

/// Persists the config file; writes a temp file first then renames over
/// the original, so a mid-way failure never truncates the original
pub fn save(config: &Config) {
    let path = config_path();
    let tmp = path.with_extension("toml.tmp");
    let text = match toml::to_string_pretty(config) {
        Ok(text) => text,
        Err(e) => {
            crate::platform::log(&format!("config.toml 序列化失败: {e}"));
            return;
        }
    };
    // The tmp file carries a restricted DACL at creation, then rename
    // finalizes it: renaming preserves the security descriptor, so there
    // is never a "plain text exposed first" window.
    if let Err(e) = crate::platform::write_private(&tmp, &text) {
        crate::platform::log(&format!("config.toml 写入失败: {e}"));
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        crate::platform::log(&format!("config.toml 写入失败: {e}"));
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parsed template must equal the default config field by field;
    /// any drift between TEMPLATE and Default is fully flagged here. Assert
    /// parseability first — parse_or_default swallows errors into the
    /// default fallback, making the equality assertion vacuously true on
    /// broken syntax.
    #[test]
    fn template_parses_back() {
        toml::from_str::<Config>(TEMPLATE).expect("TEMPLATE 必须可解析");
        let (cfg, broken) = parse_or_default(TEMPLATE, Path::new("no-such-config.toml"));
        assert!(!broken);
        assert_eq!(cfg, Config::default());
    }

    /// Corrupted text falls back to the default config with the fallback
    /// flag set: no panic, no partial adoption
    #[test]
    fn corrupted_text_falls_back_to_default() {
        let (cfg, broken) = parse_or_default(
            "this is not valid toml ]][",
            Path::new("no-such-config.toml"),
        );
        assert!(broken);
        assert_eq!(
            cfg.general.poll_interval_secs,
            General::default().poll_interval_secs
        );
        assert!(cfg.accounts.is_empty());
        assert!(cfg.selected.is_none());
    }

    /// Configs from older versions missing team fields deserialize with
    /// default values
    #[test]
    fn account_team_fields_default_when_missing() {
        let text = r#"
[[accounts]]
id = "acc_demo"
name = "demo"
platform = "cn"
api_key = "sk-demo"
"#;
        let cfg: Config = toml::from_str(text).expect("缺省字段应可反序列化");
        assert_eq!(cfg.accounts.len(), 1);
        let a = &cfg.accounts[0];
        assert!(!a.team);
        assert_eq!(a.org_id, "");
        assert_eq!(a.project_id, "");
    }
}
