//! 系统 Toast 通知

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::core::HSTRING;

/// Toast 归属标识：未在注册表登记的标识发通知会被系统直接丢弃
const AUMID: &str = "Quotify.Quotify";

/// 启动时登记 AUMID：通知中心里显示应用名 Quotify；重复调用无副作用
pub fn ensure_aumid() {
    match windows_registry::CURRENT_USER
        .create(r"Software\Classes\AppUserModelId\Quotify.Quotify")
        .and_then(|key| key.set_string("DisplayName", "Quotify"))
    {
        Ok(()) => {}
        Err(e) => crate::platform::log(&format!("[Quotify] AUMID 登记失败，通知将不可用: {e}")),
    }
}

/// 弹出系统 Toast：标题一行加粗 + 正文一行
pub fn show(title: &str, body: &str) {
    // 文案拼进 XML，&<> 转义防破坏结构
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text>\
         </binding></visual></toast>",
        esc(title),
        esc(body)
    );
    let Ok(doc) = XmlDocument::new() else {
        return;
    };
    if doc.LoadXml(&HSTRING::from(xml.as_str())).is_err() {
        crate::platform::log("[Quotify] Toast XML 解析失败");
        return;
    }
    let Ok(toast) = ToastNotification::CreateToastNotification(&doc) else {
        return;
    };
    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
    {
        Ok(n) => n,
        Err(e) => {
            crate::platform::log(&format!("[Quotify] Toast notifier 创建失败: {e}"));
            return;
        }
    };
    if let Err(e) = notifier.Show(&toast) {
        crate::platform::log(&format!("[Quotify] Toast 发送失败: {e}"));
    }
}
