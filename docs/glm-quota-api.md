# GLM Coding Plan 用量接口逆向参考

> 本文档由同类开源项目交叉审计 + 官方文档/社区 issue 核对整理，供
> `src/api/` 维护时对照。来源：CodexBar / ai-usagebar / cc-switch（issue
> #1588、#1062、#3036、#3820、#4222、#6402）/ opencode-glm-quota /
> glm-quota-monitor / glm-usage-monitor / glm-usage-tray / CPA-Manager-Plus /
> VS Code 扩展 zai-usage-tracker / 官方插件 zai-org/zai-coding-plugins /
> ZCode 官方文档 / 控制台实测样本。样本与结论均注明来源。

## 端点

| 用途 | URL |
|---|---|
| 额度查询 | `GET {base}/api/monitor/usage/quota/limit` |
| 模型用量 | `GET {base}/api/monitor/usage/model-usage?startTime=…&endTime=…` |
| 工具用量 | `GET {base}/api/monitor/usage/tool-usage?startTime=…&endTime=…` |
| 账户余额（仅国内） | `GET https://www.bigmodel.cn/api/biz/account/query-customer-account-report` |

- 国内版 base = `https://open.bigmodel.cn`，国际版 = `https://api.z.ai`；
  `bigmodel.cn`（无 `open.` 前缀，控制台域名）同样可用（实测 +
  cc-switch #3820 评论区），`dev.bigmodel.cn` 亦有项目引用。
- 余额是 `www.bigmodel.cn` 控制台域名（实测同时接受 Bearer 与裸 key）。
- 模型/工具用量的 `startTime/endTime` 用**本地时区**
  `yyyy-MM-dd HH:mm:ss`（官方插件同款算法，抄自 zai-coding-plugins 源码）。

## 鉴权

- `Authorization` 头放 API key；**裸 key 与 `Bearer <key>` 服务端都收**
  （裸 key：ai-usagebar / glm-usage-monitor / opencode-glm-quota；Bearer：
  CodexBar / glm-quota-monitor；两组独立作者均实测可用）。浏览器网页会话
  抓包则可能是 `Bearer <JWT>`（cc-switch #1062，国际站网页）。
- 常规附带头：`Accept-Language: en-US,en`、`Content-Type: application/json`。

## 团队版（重要）

来源：CodexBar `docs/zai.md` + cc-switch #4222（官方 v3.17.0 修复）+
#1588 评论区实测。仅国内站。

- 团队额度查询：quota/limit 追加查询参数 **`type=2`**；团队模型用量追加 `type=3`。
- 必须附带请求头：
  - `Bigmodel-Organization: <org id>`
  - `Bigmodel-Project: <project id>`
- **API Key + `?type=2` + 两个头，三者缺一不可**；缺失任一 selector 时
  接口仍返回 success，但 `data` 为空 / limits 为空——这是「团队版查询
  失败/解析为空」类 issue（cc-switch #4222、#6402）的根因。
- org/project 从团队用量页（`bigmodel.cn/coding-plan/team/usage-stats`）
  DevTools 抓请求头获得。
- 响应结构与个人版相同（TOKENS_LIMIT + TIME_LIMIT）。
- **Quotify 已实现**：账号类型选「团队版」时填组织/项目 ID，请求侧
  自动加 `?type=2` 与两个选择头；类型与平台联动（团队仅国内站）。
- 另有第三种认证路径（glm-usage-tray）：网页 Cookie
  `bigmodel_token_production` 作 token + 同款两个 selector 头，域名
  `bigmodel.cn`（无 `open.`）。

## 响应信封

```json
{ "code": 200, "msg": "success", "success": true, "data": { ... } }
```

- 失败形态：HTTP 200 + `success:false` + `code:401/…`（无效 key 即此形态）。
- 校验共识：`success===true` 且 `code===200`（ai-usagebar 容忍 code 缺失或 0）。
- **V3 实测变体（cc-switch #6402，2026-08-13）**：顶层可能直接是
  `limits` 数组、无 `{data:{limits}}` 信封；type 同批改为 `CREDIT_LIMIT`。
  解析需兼容两种顶层结构（Quotify 已实现）。

## data 字段

- `planName` / `plan` / `plan_type` / `packageName` / `level` ——套餐名多字段
  fallback，首个非空者用（CodexBar 与我们一致）。`level` 实测值为小写
  （"max"/"pro"，V1 Max 控制台样本 + #6402 之外的多数样本）。
- `limits[]`：额度桶数组，见下。

### limits[] 条目字段

| 字段 | 语义 |
|---|---|
| `type` | `TOKENS_LIMIT`（旧）/ `CREDIT_LIMIT`（积分制 V3，同一窗口的改名）/ `TIME_LIMIT`（MCP 通道，绝不是月度 Coding 窗） |
| `unit` | **时间单位**：`1`=天、`3`=小时、`4`=天（glm-usage-monitor 实测枚举）、`5`=分钟、`6`=周 |
| `number` | 时长数量；`window = number × unit` |
| `percentage` | 已用百分比（整数；绝对值可用时应重算，见下） |
| `nextResetTime` | 重置时刻，epoch 毫秒；`0`/`null`/缺失 = 无（如 0% 的 5h 桶） |
| `usage` | 总量（绝对值；**TIME_LIMIT 语境下也是总量**，官方源码曾拼错为 `totol`） |
| `currentValue` | 当前用量 |
| `remaining` | 剩余 |
| `usageDetails[]` | MCP 按模型明细 `{modelCode, usage}`（实测三件套：search-prime / web-reader / zread，之和 = currentValue） |

### 真实样本

V1 Max（2026-08-25 控制台实测，bigmodel.cn）：单 5h 桶 + MCP 桶，无周窗，
level 小写：

```json
{"code":200,"msg":"操作成功","data":{"limits":[
  {"type":"TIME_LIMIT","unit":5,"number":1,"usage":4000,"currentValue":85,
   "remaining":3915,"percentage":2,"nextResetTime":1789707570998,
   "usageDetails":[{"modelCode":"search-prime","usage":56},
     {"modelCode":"web-reader","usage":29},{"modelCode":"zread","usage":0}]},
  {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":11,
   "nextResetTime":1787633490996}],"level":"max"},"success":true}
```

V3 / CREDIT_LIMIT（cc-switch #6402 实测 dump，顶层裸数组）：

```json
[{"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":28000,"currentValue":2585,
  "remaining":25414,"percentage":9,"nextResetTime":1786592963348},
 {"type":"CREDIT_LIMIT","unit":6,"number":1,"usage":140000,"currentValue":58386,
  "remaining":81613,"percentage":41,"nextResetTime":1786692650981}]
```

V3 信封形态（ai-usagebar 抓包，2026-05-23）：

```json
{"code":200,"msg":"Operation successful","data":{
  "limits":[
    {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":0},
    {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":0,"nextResetTime":1779792169974},
    {"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,"currentValue":0,"remaining":1000,
     "percentage":0,"nextResetTime":1779964969979,
     "usageDetails":[{"modelCode":"search-prime","usage":0}]}],
  "level":"pro"},
 "success":true}
```

V1 老套餐（cc-switch #1588 评论区，2026-02-12 前订阅）：仅 1 个
TOKENS_LIMIT（带 unit），TIME_LIMIT 带 usageDetails：

```json
{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":2,"nextResetTime":1774967594803}
{"type":"TIME_LIMIT","unit":5,"number":1,"usage":1000,"currentValue":0,"remaining":1000,
 "percentage":0,"nextResetTime":1776664808974,"usageDetails":[…]}
```

30 天滚动窗（CodexBar 测试夹具——**证明窗口不止 5h/周两种**）：

```json
{"type":"TOKENS_LIMIT","unit":1,"number":30,"percentage":50,"nextResetTime":1787112000000}
```

MCP 显式时长 / 未知 unit 形态（CodexBar 夹具）：

```json
{"type":"TIME_LIMIT","unit":3,"number":5,"percentage":22,"nextResetTime":1785816000000}
{"type":"TIME_LIMIT","unit":0,"number":1,"percentage":22,"nextResetTime":1785816000000}
```

## 分类共识（多项目对照）

1. **TOKENS/CREDIT 桶按窗口时长分类，不按 unit 数值身份**：
   CodexBar 全部按 `windowMinutes`（乘数表 1440/60/1/10080 分钟）排序——
   最短者为 session（5h）主窗，最长者为周窗；中间桶丢弃。
   `unit:1 number:30` 的 30 天滚动窗只有时长法能正确归类。
   ai-usagebar / opencode-glm-quota 按 unit 身份（3/6）严格匹配，
   unit:4 或 30 天窗会漏。**Quotify 已改为时长法**（另补 unit:4=天乘数）。
2. **TIME_LIMIT 恒为 MCP 通道**，不冒充月度 Coding 窗；`unit:5 number:1`
   是「月度」标记（按 30 天处理），也可能带真实时长或未知 unit。
3. 百分比重算（有绝对值时不信任服务端 percentage）：
   `used = max(usage - remaining, currentValue)`，钳制 0–100
   （CodexBar 与 Quotify 公式一致；其余项目多数直接用 percentage）。
4. `nextResetTime` 为 epoch 毫秒；0/null 视为缺失。
5. 仅 unit 身份而无绝对值时（V1 形态），percentage 直接消费。
6. **空 limits = 可行动错误**：团队版缺 selector 头或 key 无 Coding Plan
   权限时返回 success + 空 limits，应报错而非静默空面板（Quotify 已实现）。

## 余额（仅国内）

- 字段：`availableBalance`（优先）→ `balance`（回退）；明细
  `rechargeAmount` / `giveAmount` / `totalSpendAmount`。
- 陷阱（CodexBar 注）：`Number(null)===0`——只有真正的数值参与展示，
  否则会渲染误导性的 ¥0.00。
- 余额失败不得影响主数据（best-effort）。智谱官方说明：Coding Plan
  账号本无「API 计费余额」概念，该端点属按量付费控制台（cc-switch #3820 澄清）。

## 额度重置卡（2026-08 上线）

来源：ZCode 官方文档 `zcode.z.ai/cn/docs/usage-stats`「额度重置卡」+
`zcode.z.ai/cn/changelog`（v3.8.1）。

- 5h / 周额度用尽后，可在端内点击重置机会**立即恢复 100%**；有有效期，
  多张时自动使用最早获得的那张，过期自动失效。
- ZCode 3.8.1+ 在**闲时**自动向 Coding Plan 用户下发 5h 重置卡，需同时
  满足：处于闲时时段（与闲时任务的范围不是同一套，动态调整）、5h 用量
  超阈值、当日下发次数未达上限。活动（如新套餐发布）也会发放。
- **只下发给「登录 ZCode 且连接了 Coding Plan」的账号**——仅 API Key
  接入（Quotify 的接法）或未开通套餐的账号都**不会收到重置卡**。
- 重置动作走 ZCode 客户端私有协议（账号会话），未出现在 monitor 系列
  接口中；全部同类开源项目 + GitHub 代码搜索均无逆向实现。
- 对本项目的意义：**无法也无需支持「使用重置卡」**；重置发生后下一次
  轮询自然反映（percentage 回落，现有「重置通知」可感知）。

## 积分制峰谷（V3，信息性）

CodexBar 依据 docs.z.ai/devpack：积分套餐高峰 1x / 低谷 0.5x；
高峰 = 周一至周五 06:00–10:00 UTC（即北京时间 14:00–18:00）。
接口不提供，纯客户端按钟点计算。

## 已知口径坑

- `model-usage` 的 `totalUsage.totalTokensUsage` 与模型明细
  `modelSummaryList` 求和**不同源**（glm-usage-monitor 实测 389M vs 单模型
  685M），做总量时用明细求和。
- `tool-usage` 响应形状存在两代分歧（`toolUsage[]` vs
  `totalUsage{…toolSummaryList}`），旧形状在现行 API 下会静默拿空数组。
- 模型请求路由（`/api/anthropic`、`/api/coding/paas/v4`）与用量查询路由
  （`/api/monitor/...`）是两套，不能互相推断（cc-switch #3820）。
- quota 查询必须与账号站点一致（open.bigmodel.cn ↔ api.z.ai），
  硬编码错站直接失败。
