//! 配置持久化

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::client::Platform;

/// 默认轮询间隔（秒）
pub const DEFAULT_INTERVAL_SECS: u64 = 300;
/// 轮询间隔下限（秒）
pub const MIN_POLL_SECS: u64 = 10;

/// 全局偏好
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// 轮询间隔（秒）
    pub poll_interval_secs: u64,
    /// 界面语言；None = 跟随系统
    pub language: Option<String>,
    /// 外观模式；None = 跟随系统，可选 "light" / "dark"
    pub appearance: Option<String>,
    /// 网络代理地址；None = 直连，形如 "http://host:port" / "socks5://host:port"
    pub proxy: Option<String>,
    /// 用量阈值预警开关
    pub notify_threshold_enabled: bool,
    /// 预警阈值百分比
    pub notify_threshold_percent: u8,
    /// 5 小时窗口重置提醒
    pub notify_reset_5h_enabled: bool,
    /// 周额度重置提醒
    pub notify_reset_weekly_enabled: bool,
    /// 高峰区间开始 HH:MM
    pub peak_start: String,
    /// 高峰区间结束 HH:MM
    pub peak_end: String,
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
        }
    }
}

/// 一个受监控的 GLM 账号
/// 字段序同添加页表单（名称→平台→类型→组织→项目），凭据收尾；
/// 此序即 config.toml 写出顺序
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 稳定 id
    pub id: String,
    /// 显示名
    pub name: String,
    /// 站点：国内 open.bigmodel.cn / 国际 api.z.ai
    pub platform: Platform,
    /// 团队版标记，仅国内站；请求走 `?type=2` + 组织/项目选择头
    #[serde(default)]
    pub team: bool,
    /// 团队版：`Bigmodel-Organization` 头的值
    #[serde(default)]
    pub org_id: String,
    /// 团队版：`Bigmodel-Project` 头的值
    #[serde(default)]
    pub project_id: String,
    /// API key
    pub api_key: String,
}

/// 应用全量配置
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub accounts: Vec<Account>,
    pub selected: Option<String>,
}

impl Config {
    /// 生成账号 id
    pub fn new_account_id(&self) -> String {
        let now = chrono::Utc::now();
        let ms = now.timestamp_subsec_millis() as i64;
        let sec = now.timestamp();
        format!("acc_{sec:x}{ms:x}{}", self.accounts.len())
    }

    /// 选中账号
    pub fn selected_account(&self) -> Option<&Account> {
        let sel = self.selected.as_deref()?;
        self.accounts
            .iter()
            .find(|a| a.id == sel)
            .or_else(|| self.accounts.first())
    }
}

/// 配置文件路径
pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.toml")
}

/// 首次启动写出的模板
const TEMPLATE: &str = r#"# Quotify 配置文件
# 通常通过面板内「设置」页管理，无需手动编辑；改完重启生效。
# API key 明文保存在本机，请勿分享此文件。

[general]
# 轮询间隔（秒）
poll_interval_secs = 300
# 界面语言：留空跟随系统，可设 "zh" 或 "en"
language = ""
# 外观：留空跟随系统，可设 "light" / "dark"
appearance = ""
# 网络代理：留空直连，可设 "http://host:port" 或 "socks5://host:port"
proxy = ""
# 用量阈值预警（默认关闭）
notify_threshold_enabled = false
notify_threshold_percent = 80
# 重置提醒（默认关闭）
notify_reset_5h_enabled = false
notify_reset_weekly_enabled = false
# 高峰区间（工作日生效），HH:MM 格式
peak_start = "14:00"
peak_end = "18:00"

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
selected = ""
"#;

/// 解析配置文本，损坏时坏文件改名留档并回退默认配置
fn parse_or_default(text: &str, path: &Path) -> Config {
    toml::from_str(text).unwrap_or_else(|e| {
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
        crate::platform::log(&format!("config.toml 解析失败，使用默认配置{pos}: {e}"));
        backup_broken(path);
        Config::default()
    })
}

/// 坏文件改名 .bak 留档后再回退默认，用户手改的内容有处可寻
fn backup_broken(path: &Path) {
    if path.exists() {
        let bak = path.with_extension("toml.bak");
        if let Err(e) = std::fs::rename(path, &bak) {
            crate::platform::log(&format!("config.toml 改名 .bak 失败: {e}"));
        }
    }
}

/// 读取配置；读不出或解析失败时坏文件先留档，再回退默认
pub fn load() -> Config {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_or_default(&text, &path),
        Err(_) => {
            backup_broken(&path);
            let _ = std::fs::write(&path, TEMPLATE);
            Config::default()
        }
    }
}

/// 写回配置文件；先写临时文件再改名覆盖，中途失败原文件不被截断
pub fn save(config: &Config) {
    let path = config_path();
    let tmp = path.with_extension("toml.tmp");
    let Ok(text) = toml::to_string_pretty(config) else {
        return;
    };
    if let Err(e) = std::fs::write(&tmp, &text) {
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

    /// 模板须能解析回 Config 且不产生实际账号
    #[test]
    fn template_parses_back() {
        let cfg = parse_or_default(TEMPLATE, Path::new("no-such-config.toml"));
        assert_eq!(cfg.general.poll_interval_secs, 300);
        assert!(!cfg.general.notify_threshold_enabled);
        assert!(!cfg.general.notify_reset_5h_enabled);
        assert!(cfg.accounts.is_empty());
        assert!(cfg.selected.as_deref().unwrap_or("").is_empty());
    }

    /// 损坏文本回退默认配置，不 panic、不部分采用
    #[test]
    fn corrupted_text_falls_back_to_default() {
        let cfg = parse_or_default(
            "this is not valid toml ]][",
            Path::new("no-such-config.toml"),
        );
        assert_eq!(
            cfg.general.poll_interval_secs,
            General::default().poll_interval_secs
        );
        assert!(cfg.accounts.is_empty());
        assert!(cfg.selected.is_none());
    }

    /// 旧版配置缺团队字段时按默认值反序列化
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
