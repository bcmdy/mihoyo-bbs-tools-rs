# YAML 配置参考

MihoyoBBSTools RS 使用 YAML 配置，默认路径是 `config/config.yaml`。本文按节点说明全部字段、默认值和取值范围；首次使用无需手写 YAML，直接执行：

```text
MihoyoBBSToolsRS config add-account
MihoyoBBSToolsRS config setup
```

发布包中的 `config/config.example.yaml` 是当前版本的完整模板，包含所有配置名称和通知渠道示例。也可以随时输出与程序版本匹配的模板：

```text
MihoyoBBSToolsRS print-example-config > config.example.yaml
```

建议保留模板中的字段名，只修改值。修改后运行 `MihoyoBBSToolsRS validate-config`；该命令只读取并校验配置，不访问远程接口。

## 默认行为

`config add-account` 创建的新配置与完整模板默认启用原神国内游戏签到，以及大别野、原神社区签到。阅读、点赞、取消点赞、分享、HoYoLAB、云游戏、Web 活动和通知默认关闭。

需要特别注意：

- `run` 只执行配置中已经启用的项目；`--task` 只能缩小本次范围。
- `runtime.random_delay_seconds` 在每轮 `run` 或 `checkin` 开始前只随机等待一次，不会按账号重复累计；设为 `0` 可关闭。
- `${ENV_NAME}` 会在读取 YAML 时替换为同名环境变量，适合注入 Cookie、Token 和 GitHub Secrets。
- `null`、空字符串 `""`、空列表 `[]` 含义不同，应按字段表填写。
- YAML 必须使用空格缩进，不能使用 Tab。同级字段必须保持相同缩进。

## 配置结构

```text
version
runtime
├── logging
└── schedule
captcha
accounts[]
├── credentials
├── device
├── proxy
├── china_checkin
│   └── role_blacklist
├── hoyolab
├── cloud_games
├── tasks
│   ├── bbs
│   └── web_activity
└── games
notifications
└── providers[]
```

字段的完整 YAML 写法以发布包中的 `config/config.example.yaml` 为准；下面的章节解释每个节点的行为。通知支持 `telegram`、`webhook`、`pushplus`、`ftqq`、`pushme`、`cqhttp`、`wecom`、`wecomrobot`、`pushdeer`、`dingrobot`、`feishubot`、`bark`、`gotify`、`ifttt`、`qmsg`、`discord`、`wxpusher`、`serverchan3`、`smtp` 和 `windows_toast`。

## 全部字段与默认值

### 顶层节点

| 字段 | 类型 | 默认/要求 | 中文说明 |
|---|---|---|---|
| `version` | 整数 | 必填，固定 `1` | 配置格式版本，不要自行修改。 |
| `runtime` | 对象 | 可省略 | 全局网络、日志和常驻调度设置。 |
| `captcha` | 对象 | 可省略 | 验证码求解平台设置。 |
| `accounts` | 对象列表 | 至少一个 | 账号及各账号任务设置。 |
| `notifications` | 对象 | 默认关闭 | 所有账号结束后统一发送的通知。 |

### `runtime`

| 字段 | 类型 | 默认值/范围 | 中文说明 |
|---|---|---|---|
| `timezone` | 字符串 | `Asia/Shanghai` | IANA 时区名称，例如 `Asia/Shanghai`、`UTC`。 |
| `request_timeout_seconds` | 整数 | `30`，1–300 | 单次网络请求超时秒数。 |
| `retry_count` | 整数 | `3`，0–10 | GET 类读取请求的最大尝试次数（包含首次，`0` 仍至少请求一次）；可能产生副作用的 POST 由任务状态复查控制。 |
| `game_checkin_max_attempts` | 整数 | `3`，1–10 | 国内与 HoYoLAB 游戏签到的最大提交次数；每次提交后先复查。 |
| `random_delay_seconds` | 整数 | `10`，0–3600 | 每轮开始前在 `0..=该值` 秒中随机等待；`0` 关闭。 |
| `log_level` | 字符串 | `info` | `trace`、`debug`、`info`、`warn`、`error`。 |
| `logging` | 对象 | 默认启用 | 文件日志设置，见下表。 |
| `schedule` | 对象 | 默认关闭 | `schedule` 常驻运行设置，见下表。 |

`runtime.logging`：

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `enabled` | 布尔 | `true` | 是否写入日志文件。 |
| `directory` | 路径字符串 | `logs` | 日志目录；相对路径以运行工作目录为基准。 |
| `file_prefix` | 字符串 | `mihoyo-bbs-tools` | 文件名前缀，最终生成 `<前缀>_YYYY-MM-DD.log`。 |

`runtime.schedule`：

| 字段 | 类型 | 默认值/范围 | 中文说明 |
|---|---|---|---|
| `enabled` | 布尔 | `false` | 是否允许启动 `schedule` 命令。 |
| `interval_minutes` | 整数 | `720`，1–10080 | 一轮完成到下一轮开始的等待分钟数。 |
| `run_on_start` | 布尔 | `true` | `true` 启动后立即执行；`false` 先等待一个间隔。 |

### `captcha`

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `endpoint` | HTTP/HTTPS URL 或 `null` | `null` | pass_nine 兼容的完整求解地址；不使用时为 `null`。 |

### `accounts[]`

| 字段 | 类型 | 默认/要求 | 中文说明 |
|---|---|---|---|
| `name` | 字符串 | 必填且唯一 | 账号显示名；自动添加时为 `mys用户:<昵称>`。 |
| `remark` | 字符串或 `null` | `null` | 只用于区分账号的备注。 |
| `enabled` | 布尔 | `true` | 整个账号的总开关。 |
| `credentials` | 对象 | 必填 | 国内 Cookie 与 `stoken`。 |
| `device` | 对象 | 有默认值 | 设备名称、型号、ID 和 FP。 |
| `proxy` | 对象 | `url: null` | 该账号业务请求使用的代理。 |
| `china_checkin` | 对象 | 有默认值 | 国内签到 User-Agent 与角色黑名单。 |
| `hoyolab` | 对象 | 新账号完整写出 | HoYoLAB 独立凭据、语言和游戏。 |
| `cloud_games` | 对象 | 默认全部关闭 | 国内与国际服云游戏设置。 |
| `tasks` | 对象 | 建议保留全部字段 | 该账号的任务开关。 |
| `games` | 字符串列表 | 模板为 `[genshin]` | 参与国内签到的游戏。 |

`accounts[].credentials`：

| 字段 | 类型 | 默认/要求 | 中文说明 |
|---|---|---|---|
| `cookie` | 字符串 | 必填 | 国内签到、米游社社区和 Web 活动使用的完整 Cookie。 |
| `stoken` | 字符串 | 社区任务需要 | `config add-account` 或菜单更新 Cookie 时自动提取。 |

`accounts[].device`：

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `name` | 字符串 | `Xiaomi MI 6` | 上报的设备名称。 |
| `model` | 字符串 | `Mi 6` | 上报的设备型号。 |
| `id` | 字符串 | `""` | 留空时按账号 Cookie 确定性生成；需要稳定设备身份时填写固定值。 |
| `fp` | 字符串 | `""` | 米游社 App 类社区请求使用的设备指纹；留空时不发送。 |

`accounts[].proxy`：

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `url` | URL 或 `null` | `null` | 支持 HTTP、HTTPS、SOCKS5、SOCKS5H；省略协议按 HTTP。 |

`accounts[].china_checkin`：

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `user_agent` | 字符串 | 内置米游社移动端 UA | 国内游戏签到请求使用的 User-Agent。 |
| `role_blacklist.genshin` | UID 列表 | `[]` | 跳过的原神完整角色 UID。 |
| `role_blacklist.honkai2` | UID 列表 | `[]` | 跳过的崩坏学园2完整角色 UID。 |
| `role_blacklist.honkai3rd` | UID 列表 | `[]` | 跳过的崩坏3完整角色 UID。 |
| `role_blacklist.tears_of_themis` | UID 列表 | `[]` | 跳过的未定事件簿完整角色 UID。 |
| `role_blacklist.star_rail` | UID 列表 | `[]` | 跳过的崩坏：星穹铁道完整角色 UID。 |
| `role_blacklist.zenless_zone_zero` | UID 列表 | `[]` | 跳过的绝区零完整角色 UID。 |

`accounts[].hoyolab`：

| 字段 | 类型 | 默认/要求 | 中文说明 |
|---|---|---|---|
| `cookie` | 字符串 | 启用时必填 | HoYoLAB 国际服独立 Cookie。 |
| `language` | 字符串 | `zh-cn` | `zh-cn`、`en-us`、`ja-jp`、`ko-kr`。 |
| `user_agent` | 字符串 | 内置移动端 UA | HoYoLAB 请求使用的 User-Agent。 |
| `games` | 字符串列表 | 启用时至少一个 | 不支持 `honkai2`，其他可用值见游戏表。 |

`accounts[].cloud_games`：

| 字段 | 类型 | 模板值 | 中文说明 |
|---|---|---|---|
| `china.genshin.enabled` | 布尔 | `false` | 国内云原神单游戏开关。 |
| `china.genshin.token` | 字符串或 `null` | `null` | 国内云原神 Token。 |
| `china.zenless_zone_zero.enabled` | 布尔 | `false` | 国内云绝区零单游戏开关。 |
| `china.zenless_zone_zero.token` | 字符串或 `null` | `null` | 国内云绝区零 Token。 |
| `overseas.language` | 字符串 | `zh-cn` | 国际服语言，可选值同 HoYoLAB。 |
| `overseas.genshin.enabled` | 布尔 | `false` | 国际服云原神单游戏开关。 |
| `overseas.genshin.token` | 字符串或 `null` | `null` | 国际服云原神 Token。 |

`accounts[].tasks`：

| 字段 | 类型 | 模板值 | 中文说明 |
|---|---|---|---|
| `china_game_checkin` | 布尔 | `true` | 国内游戏签到。 |
| `hoyolab_checkin` | 布尔 | `false` | HoYoLAB 国际服签到。 |
| `bbs` | 对象 | 见下表 | 米游社社区任务。不要写成空对象 `{}`。 |
| `china_cloud_game` | 布尔 | `false` | 国内云游戏区域总开关。 |
| `overseas_cloud_game` | 布尔 | `false` | 国际服云游戏区域总开关。 |
| `web_activity` | 对象 | 见下表 | Web 活动状态处理。 |

`accounts[].tasks.bbs`：

| 字段 | 类型 | 模板值 | 中文说明 |
|---|---|---|---|
| `enabled` | 布尔 | `true` | 米游社社区任务总开关。 |
| `sign` | 布尔 | `true` | 社区板块签到。 |
| `forums` | 整数列表 | `[5, 2]` | 默认依次为大别野、原神；首项也用于获取帖子。 |
| `read` | 布尔 | `false` | 阅读帖子。 |
| `like` | 布尔 | `false` | 点赞帖子。 |
| `cancel_like` | 布尔 | `false` | 点赞成功后取消点赞，仅在 `like: true` 时有意义。 |
| `share` | 布尔 | `false` | 分享帖子。 |

`forums` 的板块 ID：`1` 崩坏3、`2` 原神、`3` 崩坏学园2、`4` 未定事件簿、`5` 大别野、`6` 崩坏：星穹铁道、`8` 绝区零、`9` 崩坏：因缘精灵、`10` 星布谷地。列表不能包含未知或重复 ID。

`accounts[].tasks.web_activity`：

| 字段 | 类型 | 模板值 | 中文说明 |
|---|---|---|---|
| `enabled` | 布尔 | `false` | 是否处理活动状态。 |
| `activities` | 字符串列表 | `[genshin_mizone]` | 当前只识别已结束的 `genshin_mizone`；启用后输出 `Skipped`。 |

`accounts[].games` 国内签到可用值：

| 值 | 游戏 |
|---|---|
| `genshin` | 原神 |
| `honkai2` | 崩坏学园2 |
| `honkai3rd` | 崩坏3 |
| `tears_of_themis` | 未定事件簿 |
| `star_rail` | 崩坏：星穹铁道 |
| `zenless_zone_zero` | 绝区零 |

### `notifications`

| 字段 | 类型 | 默认值 | 中文说明 |
|---|---|---|---|
| `enabled` | 布尔 | `false` | 通知总开关；启用时至少配置一个渠道。 |
| `error_only` | 布尔 | `false` | 只在核心任务出现失败时推送。 |
| `block_keywords` | 字符串列表 | `[]` | 推送前替换为等长星号的关键词。 |
| `providers` | 对象列表 | `[]` | 通知渠道列表，可同时配置多个。 |

每个 `providers[]` 都必须有 `type`。各渠道字段：

| `type` | 必填字段 | 可选字段 |
|---|---|---|
| `telegram` | `bot_token`、`chat_id` | `api_url`、`proxy` |
| `webhook` | `url` | 无 |
| `pushplus` | `token` | `topic` |
| `ftqq` | `sendkey` | `api_url` |
| `pushme` | `token` | `api_url` |
| `cqhttp` | `url`，`qq`/`group` 二选一 | 无 |
| `wecom` | `corp_id`、`agent_id`、`secret` | `to_user`、`api_url` |
| `wecomrobot` | `url` | `mobile` |
| `pushdeer` | `token` | `api_url` |
| `dingrobot` | `webhook` | `secret` |
| `feishubot` | `webhook` | 无 |
| `bark` | `token` | `api_url`、`icon` |
| `gotify` | `token`、`api_url` | `priority` |
| `ifttt` | `event`、`key` | `api_url` |
| `qmsg` | `key` | `api_url` |
| `discord` | `webhook` | 无 |
| `wxpusher` | `app_token`，`uids`/`topic_ids` 至少一项 | `api_url` |
| `serverchan3` | `sendkey` | `tags` |
| `smtp` | `host`、`port`、`from`、`to`、`username`、`password`、`subject` | `tls`、`timeout_seconds` |
| `windows_toast` | 无 | `title_prefix` |

通知字段的逐项用途、Telegram/微信/SMTP 设置流程见 [使用说明](使用说明.md#12-通知)。

## 验证码与推送

`captcha.endpoint` 是完整的 HTTP/HTTPS 求解地址，程序不会自动追加路径；服务需兼容 pass_nine 的 GET 协议，接收 `gt`、`challenge`、`use_v3_model=true`，并在顶层或 `data` 中返回 `validate` 和可选的 `challenge`。国内签到遇到验证码时会求解并携带验证参数重试一次；米游社社区签到、点赞和取消点赞按“创建验证 → 平台求解 → 服务端校验 → 重试原操作”的顺序执行，原操作最多重试一次。未配置端点、求解失败或重试后仍要求验证码时会在运行报告中明确标记，不会无限重试。

所有账号任务结束后统一发送报告；单个渠道失败不会阻止其他渠道，也不会覆盖核心任务的退出码。Bot Token、PushPlus Token、Chat ID、Webhook URL 和代理认证信息均按敏感信息处理，不会写入错误消息或日志。

环境变量会在配置反序列化前统一展开，因此即使 `notifications.enabled` 为 `false`，配置文件中已经写入的 `${ENV_NAME}` 仍必须存在。不使用通知时应保留空的 `providers: []`，不要放置尚未配置 Secret 的渠道。

Telegram 的 `api_url` 默认使用 `https://api.telegram.org`，一般无需修改。无法直接访问 Telegram API 时，可通过该渠道独立的 `proxy` 字段配置 HTTP、HTTPS、SOCKS5 或 SOCKS5H 代理，例如 `proxy: "127.0.0.1:7890"`；省略协议时按 HTTP 代理处理，不使用代理时设为 `null`。代理仅作用于 Telegram，不影响其他通知渠道；带用户名和密码的代理地址属于敏感信息，不会写入错误消息或日志。

SMTP 使用 `host`、`port`、`from`、`to`、`username`、`password`、`subject`、`tls` 和可选 `timeout_seconds`。`tls` 支持 `implicit`（通常为 465）、`starttls`（通常为 587）和 `none`（通常为 25）；默认及推荐使用 `implicit`。`timeout_seconds: null` 时复用全局请求超时，显式值必须在 1 到 300 秒之间。`none` 会明文传输 SMTP 认证信息，只能用于可信隔离网络，不建议连接公网邮件服务器。

`windows_toast` 使用 Windows 自带 WinRT 通知，不需要额外安装组件；`title_prefix` 默认是 `MihoyoBBSTools RS`，设为空字符串可只显示任务状态标题。该渠道仅支持有交互桌面会话的 Windows，Linux 会明确报告不支持；Windows 服务、Session 0 或计划任务选择“无论用户是否登录都运行”时会报告通知提交失败。

## 配置编辑与账号管理

```text
MihoyoBBSToolsRS config edit --config config/config.yaml
MihoyoBBSToolsRS config add-account --config config/config.yaml --name "备注"
MihoyoBBSToolsRS config remove-account --config config/config.yaml "mys用户:昵称"
MihoyoBBSToolsRS config setup --config config/config.yaml
```

`config edit` 使用 `VISUAL` 或 `EDITOR` 指定的编辑器（Windows 默认记事本）修改完整 YAML，并在覆盖原文件前校验。`add-account` 从标准输入读取完整 Cookie，避免 Cookie 出现在命令行历史和进程列表；程序通过公开资料接口查询米游社昵称，并将账号名称写为 `mys用户:<米游社昵称>`。可选备注仅用于区分账号。程序会从 Cookie 的 `stoken` 字段自动提取并写入 SToken；Cookie 缺少 `stoken` 时会拒绝添加，以免社区任务在运行时才认证失败。

当 `add-account` 指向的配置文件不存在时，它会在账号名称和 Cookie 全部校验成功后创建父目录及新配置。新文件只包含本次添加的账号，不会复制示例账号或 `${MIHOYO_COOKIE}` 等占位符。空文件、损坏的 YAML、目标为目录或其他读取错误不会触发自动重建；`edit`、`remove-account`、`run`、`checkin` 和 `validate-config` 仍要求配置文件已经存在。新建配置不会覆盖并发创建或已经存在的文件，Unix 下使用 `0600` 权限。

米游社总开关是 `tasks.bbs.enabled`，`sign`、`read`、`like`、`cancel_like`、`share` 分别控制社区签到、阅读、点赞、点赞后恢复状态和分享。`forums` 按顺序指定社区板块，默认 `[5, 2]`，即大别野和原神；首个板块也用于获取阅读、点赞和分享所需的帖子。支持的板块 ID 为：`1` 崩坏3、`2` 原神、`3` 崩坏学园2、`4` 未定事件簿、`5` 大别野、`6` 崩坏：星穹铁道、`8` 绝区零、`9` 崩坏：因缘精灵、`10` 星布谷地。启用米游社任务时列表不能为空，也不能包含未知或重复 ID。旧写法 `bbs: true`/`false` 仍兼容，并自动采用默认板块；程序只执行已开启且服务端显示尚未完成的任务。旧版 `mihoyobbs.checkin_list` 会迁移到该字段。

国内签到或米游社任务明确返回凭据失效时，程序会使用该账号的 SToken 自动刷新 `cookie_token`，每个任务流程至多尝试一次，然后重试原任务。普通 YAML Cookie 会原子写回；`${ENV_NAME}` 提供的 Cookie 只在本次运行内更新，程序不会把展开后的 Secret 写入配置文件，后续运行前应由用户更新对应环境变量或 Secret。HoYoLAB 使用独立国际服凭据体系，不套用国内 SToken 刷新。

国内和 HoYoLAB 游戏签到由 `runtime.game_checkin_max_attempts` 控制最大尝试次数，默认 `3`，范围 `1..=10`。每次提交后都会重新查询该角色的签到状态；只有复查仍显示今日未签到、仍可领取时，才会再次提交该角色，已经确认完成的角色不会重复签到。复查请求本身失败时会立即停止，避免在状态未知时盲目提交。

确认今日已签到后，报告会显示实际尝试次数、累计天数和对应的当天奖励。奖励列表查询失败只影响详情文字，不会推翻已确认的签到结果。米游社社区签到、阅读、点赞和分享提交后也会再次读取任务状态，只有服务端确认奖励已领取（或今日已无可领取米游币）才输出成功。

### 交互式设置

`config setup` 显式进入数字菜单，可设置请求与重试、文件日志、常驻调度、验证码端点、账号启用状态与备注、Cookie/SToken、设备、账号代理、国内签到 User-Agent 与角色黑名单、HoYoLAB 独立 Cookie/语言/User-Agent/游戏、云游戏、任务以及全部通知渠道。通知渠道支持在菜单中添加、编辑和删除，Telegram 的 API 地址与独立代理也可直接设置，不再要求跳转编辑器。`高级 YAML 编辑` 仅作为可选入口保留。

多选支持连续数字（如 `123`）或逗号分隔（如 `1,2,3`），重复编号会自动去重；`0` 取消当前操作且不写配置。无效、越界或空输入会提示重新输入，EOF 会安全退出。该命令只适用于交互终端；标准输入不可用或不是交互终端时会明确失败，不会无限等待。菜单和错误信息不会显示 Cookie、SToken、通知 Token 或代理认证信息。

## 临时运行范围

可以在不修改 YAML 的情况下缩小本次运行范围：

```text
MihoyoBBSToolsRS run --task china-checkin,hoyolab-checkin,bbs,china-cloud-game,overseas-cloud-game,web-activity
MihoyoBBSToolsRS run --task bbs
MihoyoBBSToolsRS schedule
MihoyoBBSToolsRS checkin --region china
MihoyoBBSToolsRS checkin --region hoyolab
MihoyoBBSToolsRS checkin --region all
```

`run --task` 可选择 `china-checkin`、`hoyolab-checkin`、`bbs`、`china-cloud-game`、`overseas-cloud-game`、`web-activity`，支持重复使用参数或逗号分隔多值。省略 `--task` 时依次尝试所有已实现且在账号配置中启用的任务。

`run --config -` 从标准输入读取新版或受支持的旧版 YAML，适用于云函数、CI 和 Secret 挂载。标准输入配置始终标记为只读；普通文件可通过 `--read-only` 禁止凭据刷新写回。只读不代表禁用网络任务，也不阻止本轮内存中的凭据刷新，只限制持久化副作用。

`--no-notify` 禁止调用所有通知渠道。`--output json` 输出 `schema_version: 1` 的单一 JSON 对象，包含实际进程退出码、脱敏任务记录和通知投递结果；控制台日志固定写入标准错误，启用文件日志时按配置写入日志文件。该组合适合由上层程序解析：

```text
MihoyoBBSToolsRS run --config - --read-only --no-notify --output json
```

`checkin --region` 的可选值为 `china`、`hoyolab`、`all`，默认 `all`。CLI 筛选只会缩小本次执行范围，并继续与账号 `enabled`、任务开关和游戏列表取交集，不能通过命令行重新启用 YAML 中已禁用的内容；未选择的任务也不会作为失败项写入报告。

`schedule` 使用与 `run` 相同的 `--task` 参数，每轮执行完成后按 `runtime.schedule.interval_minutes` 等待，并在下一轮重新读取配置。`enabled: false` 时拒绝启动；运行过程中把该值改为 `false`，下一轮读取配置时会正常退出。`run_on_start: false` 会先等待一个完整间隔。调度器始终串行，不会重叠执行两轮任务。

## 环境变量替换

- `${ENV_NAME}` 表示读取名为 `ENV_NAME` 的环境变量。
- 环境变量不存在时，程序会报告配置错误，并显示变量名称但不显示变量值。
- 示例配置和公开文档只能使用占位符，不能包含真实 Secret。
- Cookie、Token 等敏感字段没有可用的隐式默认值；启用对应功能前必须显式配置。

在 GitHub Actions 中，应将 GitHub Secrets 映射为同名环境变量，而不是动态生成并上传包含明文凭据的配置 Artifact。

## 配置校验

`MihoyoBBSToolsRS validate-config` 只读取和校验配置，不访问远程接口。校验至少包括：

- `version` 是程序支持的配置版本。
- 账号名称非空且在配置中唯一。
- 启用的任务具备所需凭据。
- 时区、日志级别、代理 URL 和验证码服务 URL 合法。
- 超时、重试次数和随机延迟处于安全范围。
- 游戏名称和推送提供商类型可识别。
- 环境变量占位符均可解析。

普通未知字段会产生警告；无法识别的认证、任务、游戏、通知或网络安全相关值会直接报错。校验失败时进程退出码为 `2`。

## 多账号

每个账号拥有独立的名称、启用状态、国内凭据、HoYoLAB 凭据、设备信息、代理、任务开关和区域游戏列表。程序不会把一个账号的 Cookie、设备信息或代理认证信息用于另一个账号。

单个账号失败不会阻止其他可安全执行的账号任务。遇到验证码时，程序会停止无意义的高风险重复请求；全部任务结束后统一汇总成功、已完成、跳过、失败和验证码状态。

## 设备信息

设备信息通过账号下的 `device` 配置，不能再放在新版配置的顶层：

```yaml
accounts:
  - name: example
    device:
      name: "Xiaomi MI 6"
      model: "Mi 6"
      id: ""
      fp: ""
```

- `name` 是设备名称，省略时默认为 `Xiaomi MI 6`。
- `model` 是设备型号，省略时默认为 `Mi 6`。
- `id` 是接口使用的设备标识。留空时，程序会根据该账号的 Cookie 使用 UUID v3 确定性地自动生成；Cookie 改变时，自动生成的设备标识也会改变。如需保持设备身份稳定，应显式填写旧配置中的设备 ID 或一个固定值。
- `fp` 是设备指纹。非空时用于米游社 App 类社区请求的 `x-rpc-device_fp` 请求头；留空时完全省略该请求头。国内签到按原协议只发送设备 ID，HoYoLAB 与米游币任务状态 Web 请求不发送设备字段。

整个 `device` 块可以省略，此时使用上述默认值和自动生成规则。设备 ID 和指纹不应随意复制到其他账号。

## 代理

账号代理通过 `proxy.url` 配置，目标支持 HTTP、HTTPS 和 SOCKS 代理。包含用户名和密码的代理 URL 属于敏感信息，不能出现在日志或推送中。

未配置代理时使用直接连接。代理连接失败应分类为网络或代理故障，对应退出码 `5`。

## 国内与 HoYoLAB 区域配置

`accounts[].games` 只控制国内游戏签到。`china_checkin.user_agent` 控制国内签到请求头，`china_checkin.role_blacklist` 按游戏保存完整角色 UID；命中黑名单的角色会显示 `Skipped`，其他角色仍正常执行。

`accounts[].hoyolab` 使用独立 Cookie、语言、User-Agent 和游戏列表，不再与国内签到共用 `credentials.cookie` 或 `games`。HoYoLAB 支持原神、崩坏3、未定事件簿、崩坏：星穹铁道和绝区零，不支持崩坏学园2。语言可选 `zh-cn`、`en-us`、`ja-jp`、`ko-kr`。HoYoLAB Cookie 不参与国内 SToken 自动刷新。

为兼容此前生成的 version 1 配置，账号缺少整个 `hoyolab` 节点且启用了 HoYoLAB 时，程序会临时复用 `credentials.cookie` 和 `games` 并输出迁移警告；新配置和菜单添加的账号都会写出完整 `hoyolab` 节点，应直接填写独立凭据。

## 云游戏

`tasks.china_cloud_game` 和 `tasks.overseas_cloud_game` 是区域总开关；`cloud_games` 保存每个云游戏的独立开关和 Token。国内支持云原神与云绝区零，国际服支持云原神。配置示例中的 `token: null` 表示未配置，Token 已保存但临时停用时只需把对应的 `enabled` 改为 `false`。

国际服 `language` 支持 `zh-cn`、`en-us`、`ja-jp`、`ko-kr`。云游戏 Token 与 Cookie 一样属于敏感凭据，建议使用环境变量注入，不会在日志、调试输出或交互菜单中回显。账号代理同样作用于云游戏请求。

云游戏签到报告包含本次增加的免费分钟数、当前总免费时长、畅玩卡状态和米云币或邦邦点数量。接口返回 Token 失效时退出码为 `3`，网络失败为 `5`；本次未增加免费时长会标记为 `AlreadyCompleted`。

国内云游戏首次响应未显示新增时长且总免费时长低于 600 分钟时，会等待 3–6 秒后再次查询，并用前后总时长差确认异步到账。接口返回 `-100` 既可能表示 Token 失效，也可能表示防沉迷限制，因此程序只报告认证失败，不会自动删除 Token 或改写开关；请确认原因后在设置菜单中更新或清空。

## Web 活动

`tasks.web_activity.enabled` 控制 Web 活动，`activities` 是活动标识列表。目前原 Python 唯一实现的 `genshin_mizone` 已于 `2025-10-31` 结束；启用后程序会生成明确的 `Skipped` 报告并且不会请求失效接口。未知活动名在配置解析阶段直接报错，空列表也会生成明确跳过记录，不再静默忽略。

## 旧版配置兼容

程序可以读取受支持的 Python 版单账号和多账号 YAML。兼容规则如下：

- 缺失字段使用经过文档确认的安全默认值。
- 旧版顶层 `device` 会迁移到对应账号的 `device`，保留 `name`、`model`、`id` 和 `fp`。
- 旧版 `games.cn` 的 User-Agent、游戏选择和角色黑名单会迁移到国内签到配置。
- 旧版 `games.os` 的独立 Cookie、语言和游戏选择会迁移到 `hoyolab`；旧 Python 实际未执行国际服 UID 黑名单，因此该字段只产生明确警告，不会伪装成已支持。
- 旧版国内/国际云游戏总开关、单游戏开关、Token 和国际服语言会迁移到账号的 `cloud_games`。
- 无法迁移的字段产生明确警告。
- `migrate-config` 输出可直接使用的新版 YAML；生成文件包含迁移后的真实凭据，应按敏感文件保管。
- 直接运行 Python v11–v15 配置时只进行内存迁移，凭据刷新不会写回旧文件；使用 `migrate-config` 生成的新版 YAML 后才允许安全写回。

## DaCapo JSON 适配

DaCapo 是可选的第三方 JSON 配置入口。普通用户无需使用，可以忽略本节、`dacapo` 命令和发布包中的 `dacapo` 目录。

`dacapo JSON_PATH` 读取 DaCapo 传入的 JSON，叶子字段同时兼容直接标量和 `{ "value": ... }`。转换全程在内存完成，不生成含 Cookie、Token 的临时 YAML 或 INI，也不会把自动刷新的凭据写回 DaCapo 输入。

独立 `stuid` 和 `mid` 会在 Cookie 缺少对应值时分别补为 `account_id` 和 `account_mid_v2`；独立 `stoken` 写入统一凭据模型。`国服游戏.重试次数` 映射到 `runtime.game_checkin_max_attempts`，不是 HTTP 重试次数。国内角色黑名单完整迁移；DaCapo/原 Python 的国际服黑名单从未在实际签到中执行，因此只产生明确警告。未知 Web 活动直接报错，不会静默忽略。

新版 `integrations/dacapo/template.yml` 为所有可选通知补充了必要的次级字段。启用通知后，缺少 Chat ID、API 地址、SMTP 主机/邮箱或 WxPusher 接收目标等必要字段会在联网前报错。JSON 字符串同样支持 `${ENV_NAME}`，环境变量缺失时不会回显对应 Secret。

## 配置文件保护

真实配置文件应加入 `.gitignore`，并限制为仅运行用户可读。不要把真实配置复制进 Docker 镜像，也不要将其作为 GitHub Actions Artifact 上传。更完整的要求见 [安全说明](security.md)。

## 非官方声明

MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。
