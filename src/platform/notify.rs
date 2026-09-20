//! System toast notifications

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::core::HSTRING;

/// Toast ownership identifier: notifications sent under an identifier not
/// registered in the registry are dropped outright by the system.
const AUMID: &str = "Quotify.Quotify";

/// Register the AUMID at startup: the Action Center then shows the app
/// name Quotify; repeated calls have no side effects.
pub fn ensure_aumid() {
    match windows_registry::CURRENT_USER
        .create(r"Software\Classes\AppUserModelId\Quotify.Quotify")
        .and_then(|key| key.set_string("DisplayName", "Quotify"))
    {
        Ok(()) => {}
        Err(e) => crate::platform::log(&format!("AUMID 登记失败，通知将不可用: {e}")),
    }
}

/// Show a system toast: one bold title line plus one body line.
pub fn show(title: &str, body: &str) {
    // The text is spliced into XML; escape &<> so it cannot break the
    // structure.
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
        crate::platform::log("Toast XML 解析失败");
        return;
    }
    let Ok(toast) = ToastNotification::CreateToastNotification(&doc) else {
        return;
    };
    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
    {
        Ok(n) => n,
        Err(e) => {
            crate::platform::log(&format!("Toast notifier 创建失败: {e}"));
            return;
        }
    };
    if let Err(e) = notifier.Show(&toast) {
        crate::platform::log(&format!("Toast 发送失败: {e}"));
    }
}
