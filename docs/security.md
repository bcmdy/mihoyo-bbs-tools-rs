# 安全说明

MihoyoBBSTools RS 需要使用 Cookie、SToken 和可选的通知凭据。任何能够读取这些值的人都可能操作对应账号或通知渠道，因此真实配置必须按密码文件保护。

## 哪些内容属于敏感信息

- 米游社与 HoYoLAB Cookie、SToken、LToken、Cookie Token。
- 云游戏 Token。
- Telegram Bot Token、PushPlus Token、SendKey、Webhook 和其他推送凭据。
- SMTP 用户名、授权码或密码。
- 含用户名、密码或访问令牌的代理地址和验证码端点。
- 完整账号 ID、角色 UID、真实配置文件及未检查的运行日志。

## 推荐的保存方式

按使用场景选择：

| 场景 | 推荐方式 |
|---|---|
| 个人 Windows/Linux 电脑 | 保存在不共享的 `config/config.yaml`，限制为当前用户可读 |
| GitHub Actions | 使用加密 Secrets；完整 YAML 可保存为 `MIHOYO_CONFIG_YAML` Secret |
| Docker | 运行时只读挂载配置，不写入镜像 |
| Kubernetes | 使用 Secret，只读挂载；不要使用 ConfigMap |
| 上层程序或云函数 | 通过标准输入配合 `--config - --read-only` 注入 |

不要把真实配置提交到 Git、上传为 Actions Artifact、复制进 Docker 构建目录，或通过聊天和截图发送。公开示例中只使用 `${ENV_NAME}` 占位符。

Cookie 应通过 `config add-account` 或 `config setup` 的提示输入，不要放在命令参数中。命令参数会进入终端历史和进程列表。

## 环境变量与只读运行

YAML 中的 `${ENV_NAME}` 会在加载时读取同名环境变量。例如：

```yaml
credentials:
  cookie: "${MIHOYO_COOKIE}"
  stoken: "${MIHOYO_STOKEN}"
```

环境变量缺失时程序只显示变量名称，不显示变量值。即使某个通知渠道暂时关闭，只要它仍保留 `${ENV_NAME}`，加载配置时也需要对应变量；不使用的渠道应从 `providers` 删除。

国内凭据自动刷新后的保存规则：

- 普通 version 1 YAML：使用安全文件权限和原子替换写回。
- `${ENV_NAME}` 提供的 Cookie：只更新本轮内存，不把 Secret 展开写进 YAML。
- `--read-only`、标准输入、Python 旧配置和 DaCapo JSON：只更新本轮内存。

`--read-only` 只禁止修改配置文件，不会禁用网络任务或通知。需要禁止通知时还应添加 `--no-notify`。

## 日志、报告和反馈问题

程序不会主动输出 Cookie、SToken、通知 Token、SMTP 密码或代理认证信息。文本和 JSON 报告只显示执行任务所需的脱敏账号上下文；`run --output json` 把结构化报告写入标准输出，普通日志写入标准错误或日志文件。

日志仍可能包含账号名称、备注、角色 UID 尾号、请求目标和任务结果。向他人反馈问题前应人工检查并删除不希望公开的信息。不要直接上传整个 `logs` 目录或真实 JSON 报告。

`notifications.block_keywords` 可以在发送通知前把指定文字替换为等长星号，但它不能替代安全存储和人工检查。

## 网络与代理

- HTTPS 默认验证 TLS 证书，程序不提供全局关闭证书校验的选项。
- 账号 Cookie 和认证请求头只用于对应的米游社、HoYoLAB 或云游戏接口。
- HTTP/HTTPS/SOCKS 代理中的用户名和密码属于敏感信息。
- Telegram 的 `proxy` 只作用于该通知渠道；账号代理作用于对应账号的业务请求。
- SMTP 推荐使用 `implicit` 或 `starttls`。`tls: none` 会明文传输认证信息，只适用于可信隔离网络。
- 验证码端点和自定义通知 API 可能包含第三方服务凭据，使用前应确认服务可信。

可能产生副作用的签到、点赞等请求不会按普通 GET 请求盲目重试。程序会先复查服务端任务状态，再决定是否提交缺失任务。

## GitHub Actions 与发布

- 不要在 Pull Request、工作流日志或 Artifact 中输出真实 Secret。
- 来自 Fork 的工作流不应获得仓库真实账号凭据。
- “定时执行任务”只在仓库变量 `ENABLE_SCHEDULED_RUN=true` 且 Secret `MIHOYO_CONFIG_YAML` 存在时执行真实任务。
- 正式发布只针对已经存在的 `v<版本>` 标签；手动运行“发布”工作流也必须输入对应的已存在标签版本。
- 下载发布包后可使用旁边的 `.sha256` 文件核对完整性。

## Docker 与 Kubernetes

- Secret 不得通过 Dockerfile 的 `ARG`、`ENV`、`COPY` 或镜像层写入镜像。
- 容器应使用非 root 用户，只读挂载配置，并保持只读根文件系统。
- Kubernetes 使用 Secret 保存完整 YAML，禁用 ServiceAccount Token，并限制容器权限。
- Kubernetes 配置卷是只读的，自动刷新只对当次 Job 有效；需要持久化时应在集群外更新 Secret。

## 发现泄露时

1. 立即撤销或轮换相关 Cookie、Token、Bot Token、Webhook、SMTP 授权码和代理密码。
2. 停止仍在使用旧凭据的计划任务、容器和工作流。
3. 删除可下载的日志或构建产物，并检查访问记录。
4. 更新安全存储中的新值，再重新启用自动任务。

已经进入 Git 历史的 Secret 不能靠普通删除可靠撤回，必须视为已泄露并立即轮换。
