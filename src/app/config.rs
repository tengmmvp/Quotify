//! 配置持久化

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::Platform;
use crate::service::poller::DEFAULT_INTERVAL_SECS;

/// 全局偏好
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// 已读动态的日期（YYYY-MM-DD）
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

/// 一个受监控的 GLM 账号
/// 字段序同添加页表单（名称→平台→类型→组织→项目），凭据收尾；
/// 此序即 config.toml 写出顺序
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

/// 首次启动写出的模板：可选项用注释示例，赋空串会与 Default 的 None 不等
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
        crate::platform::log(&format!(
            "config.toml 解析失败，使用默认配置{pos}: {}",
            e.message()
        ));
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
            return;
        }
        crate::platform::secure_file_acl(&bak);
    }
}

/// 读取配置：解析失败留档回退默认；不存在写模板；其余读失败
/// 只回退默认不动磁盘。
pub fn load() -> Config {
    let path = config_path();
    // 清理上次进程在 write→rename 之间被杀时遗留的临时文件
    let _ = std::fs::remove_file(path.with_extension("toml.tmp"));
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            // 旧版本写下的文件没有收紧过 ACL，启动时统一补一次[幂等]
            crate::platform::secure_file_acl(&path);
            parse_or_default(&text, &path)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // 模板本身不含 key，但用户可能直接手填，落盘即统一收紧；
            // 写失败时无文件可加固，只记日志
            match std::fs::write(&path, TEMPLATE) {
                Ok(()) => crate::platform::secure_file_acl(&path),
                Err(e) => {
                    crate::platform::log(&format!("config.toml 模板写入失败: {e}"));
                }
            }
            Config::default()
        }
        Err(e) => {
            crate::platform::log(&format!("config.toml 读取失败，本次用默认配置: {e}"));
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
    // tmp 先行收紧：改名会保留安全描述符，中途被杀也不留宽松 ACL 的
    // 明文残留；rename 之后的收紧退化为幂等兜底
    crate::platform::secure_file_acl(&tmp);
    if let Err(e) = std::fs::rename(&tmp, &path) {
        crate::platform::log(&format!("config.toml 写入失败: {e}"));
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    crate::platform::secure_file_acl(&path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模板解析结果与默认配置逐字段相等，TEMPLATE 与 Default 漂移在此全量报警；
    /// 先断言可解析——parse_or_default 吞错回退默认，语法损坏时相等断言空真
    #[test]
    fn template_parses_back() {
        toml::from_str::<Config>(TEMPLATE).expect("TEMPLATE 必须可解析");
        let cfg = parse_or_default(TEMPLATE, Path::new("no-such-config.toml"));
        assert_eq!(cfg, Config::default());
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
