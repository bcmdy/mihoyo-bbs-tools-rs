# 配置说明

MihoyoBBSTools RS 使用 YAML 配置，并支持通过 `${ENV_NAME}` 从环境变量或 GitHub Secrets 注入敏感值。配置加载后会先转换为统一的内部模型，业务模块不得直接依赖旧版配置结构。

> 当前项目仍在分阶段实现配置功能。本文档描述目标格式和兼容约束，具体可用字段以对应版本的程序校验结果为准。

## 新版配置示例

```yaml
version: 1

runtime:
  timezone: Asia/Shanghai
  request_timeout_seconds: 30
  retry_count: 3
  random_delay_seconds: 10
  log_level: info

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

    tasks:
      china_game_checkin: true
      hoyolab_checkin: false
      bbs:
        enabled: true
        sign: true
        read: true
        like: true
        cancel_like: true
        share: true
      china_cloud_game: false
      overseas_cloud_game: false
      web_activity: false

    games:
      - genshin
      - star_rail
      - zenless_zone_zero

notifications:
  enabled: true
  providers:
    - type: telegram
      bot_token: "${TELEGRAM_BOT_TOKEN}"
      chat_id: "${TELEGRAM_CHAT_ID}"

    - type: webhook
      url: "${WEBHOOK_URL}"

    - type: pushplus
      token: "${PUSHPLUS_TOKEN}"
      topic: null
```

## 验证码与推送

`captcha.endpoint` 是完整的 HTTP/HTTPS 求解地址，程序不会自动追加路径；服务需兼容 pass_nine 的 GET 协议，接收 `gt`、`challenge`、`use_v3_model=true`，并在顶层或 `data` 中返回 `validate` 和可选的 `challenge`。国内签到遇到验证码时会求解并携带验证参数重试一次；米游社社区签到、点赞和取消点赞按“创建验证 → 平台求解 → 服务端校验 → 重试原操作”的顺序执行，原操作最多重试一次。未配置端点、求解失败或重试后仍要求验证码时会在运行报告中明确标记，不会无限重试。

通知当前支持 `telegram`、`webhook` 和 `pushplus`。所有账号任务结束后统一发送报告；单个渠道失败不会阻止其他渠道，也不会覆盖核心任务的退出码。Bot Token、PushPlus Token、Chat ID 和 Webhook URL 均按敏感信息处理，不会写入错误消息或日志。

环境变量会在配置反序列化前统一展开，因此即使 `notifications.enabled` 为 `false`，配置文件中已经写入的 `${ENV_NAME}` 仍必须存在。不使用通知时应保留空的 `providers: []`，不要放置尚未配置 Secret 的渠道。

## 配置编辑与账号管理

```text
mihoyo-bbs-tools config edit --config config/config.yaml
mihoyo-bbs-tools config add-account --config config/config.yaml --name "备注"
mihoyo-bbs-tools config remove-account --config config/config.yaml "备注"
```

`config edit` 使用 `VISUAL` 或 `EDITOR` 指定的编辑器（Windows 默认记事本）修改完整 YAML，并在覆盖原文件前校验。`add-account` 从标准输入读取完整 Cookie，避免 Cookie 出现在命令行历史和进程列表；备注可省略，此时使用 Cookie 中的 UID 生成名称。程序会从 Cookie 的 `stoken` 字段自动提取 SToken，因此配置中的 `credentials.stoken` 可以省略。Cookie 缺少 `stoken` 时会拒绝添加，以免社区任务在运行时才认证失败。

米游社总开关是 `tasks.bbs.enabled`，`sign`、`read`、`like`、`cancel_like`、`share` 分别控制社区签到、阅读、点赞、点赞后恢复状态和分享。旧写法 `bbs: true`/`false` 仍兼容；程序只执行已开启且服务端显示尚未完成的任务。

## 环境变量替换

- `${ENV_NAME}` 表示读取名为 `ENV_NAME` 的环境变量。
- 环境变量不存在时，配置加载必须返回明确错误。
- 错误信息可以包含环境变量名称，但不能包含变量值。
- 示例配置和仓库文档只能提交占位符，不能提交真实 Secret。
- 首版不应隐式提供敏感字段默认值。

在 GitHub Actions 中，应将 GitHub Secrets 映射为同名环境变量，而不是动态生成并上传包含明文凭据的配置 Artifact。

## 配置校验

`mihoyo-bbs-tools validate-config` 只读取和校验配置，不访问远程接口。校验至少包括：

- `version` 是程序支持的配置版本。
- 账号名称非空且在配置中唯一。
- 启用的任务具备所需凭据。
- 时区、日志级别、代理 URL 和验证码服务 URL 合法。
- 超时、重试次数和随机延迟处于安全范围。
- 游戏名称和推送提供商类型可识别。
- 环境变量占位符均可解析。

未知的普通字段应产生警告，可能影响认证、任务选择或网络安全的未知值必须报错。校验失败时进程退出码为 `2`。

## 多账号

每个账号拥有独立的名称、启用状态、凭据、设备信息、代理、任务开关和游戏列表。不同账号的 HTTP 客户端和认证上下文必须隔离，不能复用另一个账号的 Cookie、设备信息或代理认证信息。

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
- `fp` 是设备指纹。当前版本会读取、校验、序列化并在旧配置迁移时保留该值，但尚未将其用于请求头或接口参数；留空不会影响当前已实现任务。

整个 `device` 块可以省略，此时使用上述默认值和自动生成规则。设备 ID 和指纹不应随意复制到其他账号。

## 代理

账号代理通过 `proxy.url` 配置，目标支持 HTTP、HTTPS 和 SOCKS 代理。包含用户名和密码的代理 URL 属于敏感信息，不能出现在日志或推送中。

未配置代理时使用直接连接。代理连接失败应分类为网络或代理故障，对应退出码 `5`。

## 旧版配置兼容

兼容层负责读取经过支持的 Python 版单账号和多账号 YAML，并转换为统一内部模型。兼容过程遵循以下规则：

- 缺失字段使用经过文档确认的安全默认值。
- 旧版顶层 `device` 会迁移到对应账号的 `device`，保留 `name`、`model`、`id` 和 `fp`。
- 无法迁移的字段产生明确警告。
- 旧格式解析完成后，业务代码只使用新版内部模型。
- `migrate-config` 输出新版 YAML，输出内容不得包含额外日志或未脱敏 Secret。
- 转换后的配置再次读取时应得到等价内部结果。

兼容测试使用从旧项目抽取并人工脱敏的 Fixture，不得提交真实账号配置或历史日志。

## 配置文件保护

真实配置文件应加入 `.gitignore`，并限制为仅运行用户可读。不要把真实配置复制进 Docker 镜像，也不要将其作为 GitHub Actions Artifact 上传。更完整的要求见 [安全说明](security.md)。

## 非官方声明

MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。
