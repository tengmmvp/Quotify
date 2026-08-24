# Quotify

Windows 任务栏托盘的 GLM Coding Plan 用量小组件。悬停托盘图标，苹果风格的圆角面板在图标上方弹出，一眼看清 5 小时窗口、周额度与 MCP 工具用量；点击锁定面板从容查看。

![面板](docs/panel.png)

## 特性

- **环形托盘图标**：5 小时窗口余量一目了然，颜色随用量档位变化（绿 → 橙 → 红），查询失败时灰环
- **悬停即出面板**：macOS 菜单栏风格的 flyout，180ms 淡入 + 上浮；移开自动收起，点击锁定
- **完整用量视图**：套餐版本（V1/V2/V3）与等级（Lite/Pro/Max）、5 小时窗口、周额度、MCP 工具月度用量（胶囊进度条 + 百分比 + 绝对量）、重置倒计时、近 24h 模型用量迷你图、账户余额（国内版）
- **多账号**：面板内添加 / 切换 / 删除 GLM 账号，支持国内版（open.bigmodel.cn）与国际版（api.z.ai）
- **设置页**：轮询间隔（预设 + 自定义）、用量预警与重置通知（默认关闭）、开机自启、界面语言（中英双语，跟随系统）、检查更新
- **失败兜底**：断网或接口异常时保留最后一次成功数据并标注「数据截至 xx:xx」+ 重试按钮
- **轻量**：单文件 exe（约 1.5MB），静止时工作集约 5–11MB、零 CPU 占用；面板收起后自动归还内存

## 使用

1. 从 [Releases](https://github.com/TengMMVP/quotify/releases) 下载 `quotify.exe`，放到任意目录
2. 运行后托盘出现环形图标（首次可能在托盘溢出区 `^` 中，建议在任务栏设置里设为显示）
3. 悬停图标弹出面板 → 点击齿轮进入设置 → 添加账号（填 API key、选择平台）即可开始使用
4. 配置保存在 exe 同目录 `config.toml`（便携式，含明文 key，请勿分享该文件）

## 开发

```powershell
cargo check   # 快速迭代
cargo test    # 解析层单测（V1/V2/V3 响应结构、认证双格式等）
cargo build --release
```

技术栈：纯 Win32（Shell_NotifyIcon v4 / Direct2D / DirectWrite / GDI+）+ ureq，无 UI 框架、无运行时依赖。

## License

MIT
