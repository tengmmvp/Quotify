//! 构建脚本
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
