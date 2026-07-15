# 配置说明

MihoyoBBSTools RS 使用 YAML 配置，并支持通过 `${ENV_NAME}` 从环境变量或 GitHub Secrets 注入敏感值。配置加载后会先转换为统一的内部模型，业务模块不得直接依赖旧版配置结构。

`runtime.random_delay_seconds` 会在每轮 `run` 或 `checkin` 开始前生成一次 `0..=配置值` 秒的随机等待；同一轮不会按账号重复累计。设为 `0` 可完全关闭随机延迟。

> 当前项目仍在分阶段实现配置功能。本文档描述目标格式和兼容约束，具体可用字段以对应版本的程序校验结果为准。

## 新版配置示例

```yaml
version: 1

runtime:
  timezone: Asia/Shanghai
  request_timeout_seconds: 30
  retry_count: 3
  game_checkin_max_attempts: 3
  random_delay_seconds: 10
  log_level: info
  logging:
    enabled: true
    directory: logs
    file_prefix: mihoyo-bbs-tools
  schedule:
    enabled: false
    interval_minutes: 720
    run_on_start: true

captcha:
  endpoint: "${CAPTCHA_ENDPOINT}"

accounts:
  - name: example
    enabled: true

    credentials:
      cookie: "${MIHOYO_COOKIE}"

    device:
      name: "Xiaomi MI 6"
      model: "Mi 6"
      id: ""
      fp: ""

    proxy:
      url: null

    china_checkin:
      user_agent: "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36 miHoYoBBS/2.109.0"
      role_blacklist:
        genshin: []
        honkai2: []
        honkai3rd: []
        tears_of_themis: []
        star_rail: []
        zenless_zone_zero: []

    hoyolab:
      cookie: "${HOYOLAB_COOKIE}"
      language: zh-cn
      user_agent: "Mozilla/5.0 (Linux; Android 12) AppleWebKit/537.36 Mobile Safari/537.36"
      games:
        - genshin

    cloud_games:
      china:
        genshin:
          enabled: false
          token: null
        zenless_zone_zero:
          enabled: false
          token: null
      overseas:
        language: zh-cn
        genshin:
          enabled: false
          token: null

    tasks:
      china_game_checkin: true
      hoyolab_checkin: false
      bbs:
        enabled: true
        sign: true
        forums:
          - 5
          - 2
        read: true
        like: true
        cancel_like: true
        share: true
      china_cloud_game: false
      overseas_cloud_game: false
      web_activity:
        enabled: false
        activities:
          - genshin_mizone

    games:
      - genshin
      - star_rail
      - zenless_zone_zero

notifications:
  enabled: true
  error_only: false
  block_keywords: []
  providers:
    - type: telegram
      bot_token: "${TELEGRAM_BOT_TOKEN}"
      chat_id: "${TELEGRAM_CHAT_ID}"
      api_url: "https://api.telegram.org"
      proxy: null

    - type: webhook
      url: "${WEBHOOK_URL}"

    - type: pushplus
      token: "${PUSHPLUS_TOKEN}"
      topic: null
```

除 Telegram、Webhook、PushPlus 外，通知还支持：`ftqq`、`pushme`、`cqhttp`、`wecom`、`wecomrobot`、`pushdeer`、`dingrobot`、`feishubot`、`bark`、`gotify`、`ifttt`、`qmsg`、`discord`、`wxpusher`、`serverchan3`、`smtp` 和 `windows_toast`。各渠道使用与服务商对应的字段；所有凭据均建议使用环境变量注入。`error_only: true` 时仅在核心任务非成功时推送，`block_keywords` 会在发送前将正文中的指定关键词替换为等长星号。

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
MihoyoBBSToolsRS config remove-account --config config/config.yaml "备注"
MihoyoBBSToolsRS config setup --config config/config.yaml
```

`config edit` 使用 `VISUAL` 或 `EDITOR` 指定的编辑器（Windows 默认记事本）修改完整 YAML，并在覆盖原文件前校验。`add-account` 从标准输入读取完整 Cookie，避免 Cookie 出现在命令行历史和进程列表；程序通过公开资料接口查询米游社昵称，并将账号名称写为 `mys用户:<米游社昵称>`。可选备注仅用于区分账号。程序会从 Cookie 的 `stoken` 字段自动提取并写入 SToken；Cookie 缺少 `stoken` 时会拒绝添加，以免社区任务在运行时才认证失败。

当 `add-account` 指向的配置文件不存在时，它会在账号名称和 Cookie 全部校验成功后创建父目录及新配置。新文件只包含本次添加的账号，不会复制示例账号或 `${MIHOYO_COOKIE}` 等占位符。空文件、损坏的 YAML、目标为目录或其他读取错误不会触发自动重建；`edit`、`remove-account`、`run`、`checkin` 和 `validate-config` 仍要求配置文件已经存在。新建配置不会覆盖并发创建或已经存在的文件，Unix 下使用 `0600` 权限。

米游社总开关是 `tasks.bbs.enabled`，`sign`、`read`、`like`、`cancel_like`、`share` 分别控制社区签到、阅读、点赞、点赞后恢复状态和分享。`forums` 按顺序指定社区板块，默认 `[5, 2]`，即大别野和原神；首个板块也用于获取阅读、点赞和分享所需的帖子。支持的板块 ID 为：`1` 崩坏3、`2` 原神、`3` 崩坏学园2、`4` 未定事件簿、`5` 大别野、`6` 崩坏：星穹铁道、`8` 绝区零、`9` 崩坏：因缘精灵、`10` 星布谷地。启用米游社任务时列表不能为空，也不能包含未知或重复 ID。旧写法 `bbs: true`/`false` 仍兼容，并自动采用默认板块；程序只执行已开启且服务端显示尚未完成的任务。旧版 `mihoyobbs.checkin_list` 会迁移到该字段。

国内签到或米游社任务明确返回凭据失效时，程序会使用该账号的 SToken 自动刷新 `cookie_token`，每个任务流程至多尝试一次，然后重试原任务。普通 YAML Cookie 会原子写回；`${ENV_NAME}` 提供的 Cookie 只在本次运行内更新，程序不会把展开后的 Secret 写入配置文件，后续运行前应由用户更新对应环境变量或 Secret。HoYoLAB 使用独立国际服凭据体系，不套用国内 SToken 刷新。

国内和 HoYoLAB 游戏签到由 `runtime.game_checkin_max_attempts` 控制最大尝试次数，默认 `3`，范围 `1..=10`。每次提交后都会重新查询该角色的签到状态；只有复查仍显示今日未签到、仍可领取时，才会再次提交该角色，已经确认完成的角色不会重复签到。复查请求本身失败时会立即停止，避免在状态未知时盲目提交。

确认今日已签到后，报告会显示实际尝试次数、累计天数和对应的当天奖励。奖励列表查询失败只影响详情文字，不会推翻已确认的签到结果。米游社社区签到、阅读、点赞和分享提交后也会再次读取任务状态，只有服务端确认奖励已领取（或今日已无可领取米游币）才输出成功。

### 交互式设置

`config setup` 显式进入数字菜单，可设置请求与重试、文件日志、验证码端点、账号启用状态与备注、Cookie/SToken、设备、账号代理、国内签到 User-Agent 与角色黑名单、HoYoLAB 独立 Cookie/语言/User-Agent/游戏、云游戏、任务以及全部通知渠道。通知渠道支持在菜单中添加、编辑和删除，Telegram 的 API 地址与独立代理也可直接设置，不再要求跳转编辑器。`高级 YAML 编辑` 仅作为可选入口保留。

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

`checkin --region` 的可选值为 `china`、`hoyolab`、`all`，默认 `all`。CLI 筛选只会缩小本次执行范围，并继续与账号 `enabled`、任务开关和游戏列表取交集，不能通过命令行重新启用 YAML 中已禁用的内容；未选择的任务也不会作为失败项写入报告。

`schedule` 使用与 `run` 相同的 `--task` 参数，每轮执行完成后按 `runtime.schedule.interval_minutes` 等待，并在下一轮重新读取配置。`enabled: false` 时拒绝启动；运行过程中把该值改为 `false`，下一轮读取配置时会正常退出。`run_on_start: false` 会先等待一个完整间隔。调度器始终串行，不会重叠执行两轮任务。

## 环境变量替换

- `${ENV_NAME}` 表示读取名为 `ENV_NAME` 的环境变量。
- 环境变量不存在时，配置加载必须返回明确错误。
- 错误信息可以包含环境变量名称，但不能包含变量值。
- 示例配置和仓库文档只能提交占位符，不能提交真实 Secret。
- 首版不应隐式提供敏感字段默认值。

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

未知的普通字段应产生警告，可能影响认证、任务选择或网络安全的未知值必须报错。校验失败时进程退出码为 `2`。

## 多账号

每个账号拥有独立的名称、启用状态、国内凭据、HoYoLAB 凭据、设备信息、代理、任务开关和区域游戏列表。不同账号的 HTTP 客户端和认证上下文必须隔离，不能复用另一个账号的 Cookie、设备信息或代理认证信息。

单个账号失败不应阻止其他安全任务执行，但遇到验证码时应停止该账号的高风险重复请求。全部任务结束后，由统一报告聚合每个账号的成功、已完成、跳过、失败和验证码状态。

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

兼容层负责读取经过支持的 Python 版单账号和多账号 YAML，并转换为统一内部模型。兼容过程遵循以下规则：

- 缺失字段使用经过文档确认的安全默认值。
- 旧版顶层 `device` 会迁移到对应账号的 `device`，保留 `name`、`model`、`id` 和 `fp`。
- 旧版 `games.cn` 的 User-Agent、游戏选择和角色黑名单会迁移到国内签到配置。
- 旧版 `games.os` 的独立 Cookie、语言和游戏选择会迁移到 `hoyolab`；旧 Python 实际未执行国际服 UID 黑名单，因此该字段只产生明确警告，不会伪装成已支持。
- 旧版国内/国际云游戏总开关、单游戏开关、Token 和国际服语言会迁移到账号的 `cloud_games`。
- 无法迁移的字段产生明确警告。
- 旧格式解析完成后，业务代码只使用新版内部模型。
- `migrate-config` 输出新版 YAML，输出内容不得包含额外日志或未脱敏 Secret。
- 转换后的配置再次读取时应得到等价内部结果。

兼容测试使用从旧项目抽取并人工脱敏的 Fixture，不得提交真实账号配置或历史日志。

## 配置文件保护

真实配置文件应加入 `.gitignore`，并限制为仅运行用户可读。不要把真实配置复制进 Docker 镜像，也不要将其作为 GitHub Actions Artifact 上传。更完整的要求见 [安全说明](security.md)。

## 非官方声明

MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。
