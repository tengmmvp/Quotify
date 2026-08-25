//! 配置持久化：exe 同目录 `config.toml`（便携式，配置跟着 exe 走）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::client::Platform;

/// 通知与轮询等全局偏好。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// 轮询间隔（秒），UI 层提供预设档与自定义入口
    pub poll_interval_secs: u64,
    /// 界面语言；None = 跟随系统
    pub language: Option<String>,
    /// 外观模式；None = 跟随系统，可选 "light" / "dark"
    pub appearance: Option<String>,
    /// 用量阈值预警开关（默认关，避免打扰）
    pub notify_threshold_enabled: bool,
    /// 预警阈值百分比
    pub notify_threshold_percent: u8,
    /// 5 小时窗口重置提醒（默认关）
    pub notify_reset_5h_enabled: bool,
    /// 周额度重置提醒（默认关）
    pub notify_reset_weekly_enabled: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            poll_interval_secs: 300,
            language: None,
            appearance: None,
            notify_threshold_enabled: false,
            notify_threshold_percent: 80,
            notify_reset_5h_enabled: false,
            notify_reset_weekly_enabled: false,
        }
    }
}

/// 一个受监控的 GLM 账号。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// 稳定 id（重命名不影响选中状态）
    pub id: String,
    /// 显示名
    pub name: String,
    /// API key（本地明文，与 cc-switch 等同类工具惯例一致）
    pub api_key: String,
    pub platform: Platform,
    /// 团队版标记（仅国内站；请求走 `?type=2` + 组织/项目选择头）
    #[serde(default)]
    pub team: bool,
    /// 团队版：`Bigmodel-Organization` 头的值
    #[serde(default)]
    pub org_id: String,
    /// 团队版：`Bigmodel-Project` 头的值
    #[serde(default)]
    pub project_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub accounts: Vec<Account>,
    /// 当前选中的账号 id；多账号时记住上次选择，图标与面板跟随
    pub selected: Option<String>,
}

impl Config {
    /// 生成新账号 id（时间戳 + 随机后缀防同毫秒碰撞）。
    pub fn new_account_id(&self) -> String {
        let ms = chrono::Utc::now().timestamp_subsec_millis() as i64;
        // 用账号数 + 当前秒拼一个足够防撞的后缀即可，无需随机源
        let sec = chrono::Utc::now().timestamp();
        format!("acc_{sec:x}{ms:x}{}", self.accounts.len())
    }

    pub fn selected_account(&self) -> Option<&Account> {
        let sel = self.selected.as_deref()?;
        self.accounts.iter().find(|a| a.id == sel).or_else(|| self.accounts.first())
    }
}

/// 配置文件路径：exe 同目录 `config.toml`。
pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("config.toml")
}

/// 首次启动生成的带注释模板（占位账号被注释，用户可在 UI 中添加）。
const TEMPLATE: &str = r#"# Quotify 配置文件
# 通常通过面板内「设置」页管理，无需手动编辑；改完重启生效。
# API key 明文保存在本机，请勿分享此文件。

[general]
# 轮询间隔（秒）。面板设置页可选预设档或自定义
poll_interval_secs = 300
# 界面语言：留空跟随系统，可设 "zh" 或 "en"
language = ""
# 外观：留空跟随系统，可设 "light" / "dark"
appearance = ""
# 用量阈值预警（默认关闭）
notify_threshold_enabled = false
notify_threshold_percent = 80
# 重置提醒（默认关闭）
notify_reset_5h_enabled = false
notify_reset_weekly_enabled = false

# 受监控的账号（建议在设置页添加；此处仅为字段示例）
# [[accounts]]
# id = "acc_demo"
# name = "我的 GLM"
# api_key = "你的 API key"
# platform = "cn"          # cn = 国内版 open.bigmodel.cn；intl = 国际版 api.z.ai
# team = false             # 团队版（仅国内站）：true 时下面两项必填
# org_id = ""              # 团队版：组织 ID（bigmodel-organization 请求头）
# project_id = ""          # 团队版：项目 ID（bigmodel-project 请求头）

# 当前选中的账号 id
selected = ""
"#;

/// 解析配置文本；损坏时回退默认配置。
/// 错误日志只报位置、不回显源行——toml 错误的 Display 会带出整行源码，
/// 源行可能含 API key（凭据不得进日志）。
fn parse_or_default(text: &str) -> Config {
    toml::from_str(text).unwrap_or_else(|e| {
        // span 为字节偏移，换算成 1 起始的行列号（源可含中文，按字符计列）
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
        crate::platform::log(&format!("config.toml 解析失败，使用默认配置{pos}"));
        Config::default()
    })
}

/// 读取配置；文件不存在时生成模板并返回默认配置。
pub fn load() -> Config {
    let path = config_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_or_default(&text),
        Err(_) => {
            let _ = std::fs::write(&path, TEMPLATE);
            Config::default()
        }
    }
}

/// 写回配置文件。
pub fn save(config: &Config) {
    let path = config_path();
    if let Ok(text) = toml::to_string_pretty(config)
        && let Err(e) = std::fs::write(&path, text) {
            crate::platform::log(&format!("config.toml 写入失败: {e}"));
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模板文本必须能解析回 Config：示例账号全在注释里，不产生账号；
    /// selected 的 "" 不指向任何账号。
    #[test]
    fn template_parses_back() {
        let cfg = parse_or_default(TEMPLATE);
        assert_eq!(cfg.general.poll_interval_secs, 300);
        assert!(!cfg.general.notify_threshold_enabled);
        assert!(!cfg.general.notify_reset_5h_enabled);
        assert!(cfg.accounts.is_empty());
        assert!(cfg.selected.as_deref().unwrap_or("").is_empty());
    }

    /// 损坏文本回退默认配置（不 panic、不部分采用）。
    #[test]
    fn corrupted_text_falls_back_to_default() {
        let cfg = parse_or_default("this is not valid toml ]][");
        assert_eq!(cfg.general.poll_interval_secs, General::default().poll_interval_secs);
        assert!(cfg.accounts.is_empty());
        assert!(cfg.selected.is_none());
    }

    /// 旧版配置缺 team / org_id / project_id 字段时按默认值反序列化。
    #[test]
    fn account_team_fields_default_when_missing() {
        let text = r#"
[[accounts]]
id = "acc_demo"
name = "demo"
api_key = "sk-demo"
platform = "cn"
"#;
        let cfg: Config = toml::from_str(text).expect("缺省字段应可反序列化");
        assert_eq!(cfg.accounts.len(), 1);
        let a = &cfg.accounts[0];
        assert!(!a.team);
        assert_eq!(a.org_id, "");
        assert_eq!(a.project_id, "");
    }
}
