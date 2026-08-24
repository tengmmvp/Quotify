//! Quotify — Windows 任务栏托盘的 GLM Coding Plan 用量小组件。
//!
//! 分层：`app` 装配 / `api` 数据 / `ui` 界面 / `platform` 平台服务 / `service` 后台服务。

// release 构建使用 Windows GUI 子系统：托盘程序不附带控制台窗口
// （关闭控制台会杀死进程）；debug 保留控制台便于查看日志输出。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app;
mod platform;
mod service;
mod ui;

fn main() {
    // 单实例：已有实例时唤醒它（弹面板）后静默退出
    let guard = platform::instance::acquire();
    if !guard.is_first() {
        guard.wake_existing();
        return;
    }
    std::process::exit(app::run());
}
