//! 构建脚本：嵌入应用图标（assets/icon.ico，logo 同源）。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.compile().expect("嵌入图标资源失败");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
