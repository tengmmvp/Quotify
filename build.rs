//! 构建脚本：嵌入应用图标（assets/icon.ico，logo 同源）与版本信息。
//! FileDescription/ProductName 必须显式大写——winresource 默认取 Cargo
//! 包名（小写 quotify），而任务管理器「名称」列显示的是 FileDescription。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("FileDescription", "Quotify");
        res.set("ProductName", "Quotify");
        res.set("FileVersion", env!("CARGO_PKG_VERSION"));
        res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        res.compile().expect("嵌入图标资源失败");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
