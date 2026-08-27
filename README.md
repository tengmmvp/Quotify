<div align="center">

# Quotify

<a href="https://github.com/tengmmvp/Quotify/releases"><img src="https://img.shields.io/github/v/release/tengmmvp/Quotify?style=for-the-badge&label=Release&color=2EA043" alt="Release"></a>
<a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.85%2B-DEA584?style=for-the-badge&logo=rust" alt="Rust 1.85+"></a>
<a href="https://github.com/tengmmvp/Quotify/releases"><img src="https://img.shields.io/badge/Platform-Windows_10%2F11-0078D4?style=for-the-badge&logo=windows11" alt="Platform"></a>
<a href="LICENSE"><img src="https://img.shields.io/badge/License-Apache--2.0-6F42C1?style=for-the-badge" alt="License"></a>

</div>

> **Quotify 是一款常驻 Windows 任务栏托盘的 GLM Coding Plan 用量监控小组件，实时展示 5 小时窗口、周额度、MCP 工具用量与账户余额，帮助你在编码时随时掌握套餐余量、合理安排开发节奏。**

## 预览

|              |                        深色                        |                        浅色                         |
| :----------: | :------------------------------------------------: | :-------------------------------------------------: |
| **用量面板** |  ![用量面板深色](docs/screenshots/panelDark.png)   |  ![用量面板浅色](docs/screenshots/panelLight.png)   |
| **设置面板** | ![设置面板深色](docs/screenshots/settingsDark.png) | ![设置面板浅色](docs/screenshots/settingsLight.png) |

## 特性

- **原生精致**：原生 UI 渲染，无 Electron、无运行时依赖
- **极低开销**：单文件 exe 约 1.6MB；纯软件渲染不拉起显卡驱动栈，常驻内存约 30MB 以内、零 CPU 占用
- **绿色便携**：免安装，exe 放到任意目录即可运行，配置随行（同目录 `config.toml`），整体搬移不丢状态

## 功能

- **环形托盘图标**：5 小时窗口余量一目了然，颜色随用量档位变化
- **悬停即出面板**：悬停图标即弹出，移开自动收起，点击锁定从容查看
- **完整用量视图**：5 小时窗口、周额度、MCP 工具月度用量、重置倒计时
- **深浅色主题**：跟随系统实时切换，也可手动锁定浅色或深色

## 使用

1. **下载运行**：从 [Releases](https://github.com/tengmmvp/Quotify/releases) 下载 `Quotify.exe` 放到任意目录，双击即用
2. **添加账号**：悬停托盘图标弹出用量面板 → 点击齿轮进入设置面板 → 填 API key、选择平台即可开始使用
3. **日常查看**：环形图标常驻托盘，颜色即余量档位；悬停看详情，点击锁定面板
4. **配置文件**：所有状态保存在 exe 同目录 `config.toml`，便携可迁移；内含明文 key，请勿分享该文件

## 开发

需要 Windows 与 Rust 1.85+。

```powershell
cargo check              # 快速迭代
cargo test               # 解析层单测
cargo build --release    # 产物 target/release/Quotify.exe
```

## License

本项目遵循 [Apache-2.0](LICENSE) 许可证。
