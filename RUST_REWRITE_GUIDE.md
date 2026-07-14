# MihoyoBBSTools RS 重构实施文档

> 本文档用于在一个全新的 GitHub 仓库中实施 Rust 重构。开发机器只负责编辑代码和执行 Git 操作，不安装 Rust、MSVC、Windows SDK 或其他本地编译环境；所有格式检查、静态检查、测试、编译、打包和真实接口验证均由 GitHub Actions 完成。

## 0. 项目标识

新项目统一使用以下名称：

| 用途 | 名称 |
|---|---|
| 项目展示名 | `MihoyoBBSTools RS` |
| GitHub 仓库名 | `mihoyo-bbs-tools-rs` |
| Rust Cargo 包名 | `mihoyo-bbs-tools` |
| 命令行可执行文件名 | `MihoyoBBSToolsRS` |
| 默认本地目录名 | `mihoyo-bbs-tools-rs` |

仓库名称中的 `-rs` 用于明确表示这是 Rust 重构项目；Cargo 包名保持简洁，发布可执行文件统一命名为 `MihoyoBBSToolsRS`。

README、发布说明和项目网站中必须包含以下非官方声明：

> MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。

不得使用官方 Logo、角色头像或容易使用户误认为官方产品的视觉元素和宣传文案。

## 1. 项目目标

将现有 Python 版 MihoyoBBSTools 重构为安全、可测试、可跨平台发布的 Rust 应用，同时尽量保持现有配置格式和部署方式兼容。

重构后的目标能力：

- 支持国内米游社与国际 HoYoLAB。
- 支持单账号和多账号。
- 支持原神、崩坏系列、星穹铁道、绝区零等现有签到能力。
- 支持米游社社区任务，包括帖子浏览、点赞、分享、米游币任务等。
- 支持云游戏签到和当前项目已有的 Web 活动。
- 支持验证码服务接入。
- 支持代理。
- 支持现有主要推送渠道。
- 支持 YAML 配置、环境变量和 GitHub Secrets。
- 支持 GitHub Actions、Docker、Linux 和 Windows 发布。
- 敏感信息不得出现在日志、测试快照或构建产物中。

不要求第一版一次性达到全部功能。重构必须分阶段进行，每个阶段都应可以通过 CI 验证，并保持主分支可编译。

## 2. 核心开发原则

### 2.1 完全使用 GitHub Actions 编译

本地不运行以下命令：

```text
cargo build
cargo test
cargo fmt
cargo clippy
rustc
```

这些命令全部由 GitHub Actions 执行。本地只需要：

```text
Git
文本编辑器或 Codex
GitHub 仓库访问权限
```

每次修改通过分支提交到远程仓库，根据 Actions 日志修复问题。

### 2.2 主分支始终可用

- `main` 只接收通过全部 CI 的代码。
- 每一个迁移阶段使用独立分支。
- Pull Request 必须通过格式、Clippy、测试和编译检查。
- 不允许将明显未完成且无法编译的代码直接合并到 `main`。

推荐分支命名：

```text
codex/scaffold
codex/config
codex/http-client
codex/game-checkin
codex/bbs-tasks
codex/push
codex/docker
```

提交信息和 PR 摘要使用中文，例如：

```text
初始化 Rust 项目结构与持续集成
实现 YAML 多账号配置解析
增加国内原神签到客户端
修复 Cookie 失效时泄露请求头的问题
```

### 2.3 不直接复制无许可证项目代码

可以参考公开项目的产品行为、接口形式和架构思路。

已知可以作为合规参考的项目：

- `simplyshiro/kirara`：Apache-2.0，可参考国际服签到客户端。
- `dvgamerr-app/go-hoyolab`：MIT，可参考跨平台发布和通知设计。

## 3. 建议技术栈

使用 Rust stable，Edition 2024。

| 能力 | 建议 crate |
|---|---|
| 异步运行时 | `tokio` |
| HTTP 客户端 | `reqwest` |
| TLS | `rustls`，禁用 OpenSSL 默认依赖 |
| JSON | `serde`、`serde_json` |
| YAML | `serde_yaml_ng` |
| 时间和时区 | `chrono`、`chrono-tz` |
| Cron 表达式 | `cron` |
| 命令行 | `clap` |
| 日志 | `tracing`、`tracing-subscriber` |
| 错误类型 | `thiserror` |
| 随机数 | `rand` |
| UUID | `uuid` |
| MD5 | `md-5` |
| SHA256/HMAC | `sha2`、`hmac` |
| Base64 | `base64` |
| URL | `url` |
| 敏感字符串 | 项目自定义 `SecretString` |
| 测试 HTTP 服务 | `wiremock`，仅开发依赖 |
| 临时文件 | `tempfile`，仅开发依赖 |

HTTP 客户端使用 `rustls`，并显式选择 `ring` 密码学 provider：

```toml
reqwest = {
    version = "0.13",
    default-features = false,
    features = ["form", "json", "query", "rustls-no-provider", "socks"]
}
rustls = {
    version = "0.23",
    default-features = false,
    features = ["ring", "std", "tls12"]
}
```

程序入口和统一 HTTP 客户端都会确保进程级 `ring` provider 已安装，因而不经过二进制入口的库测试也可以正常建立 TLS 连接。CI 检查普通依赖树，发现 `aws-lc-rs` 或 `aws-lc-sys` 时直接失败。

当前任务流程按顺序执行，没有依赖多线程调度器，因此 Tokio 仅启用 `macros`、`rt` 和 `time`，主入口使用 current-thread runtime。后续若引入并发任务，应重新评估这一选择。

应用程序仓库必须提交 `Cargo.lock`，CI、Release 和 Docker 均使用锁定依赖，以保证 Actions 构建可重复。

### 3.1 构建体积优化

Release 构建使用以下配置：

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = "symbols"
```

该配置优先缩减发布体积。完整 LTO 和单 codegen unit 可能增加 CI 编译时间；`opt-level = "z"` 优先体积而非纯计算性能；`panic = "abort"` 会在 panic 时直接终止进程，不执行栈展开。项目主要受网络 I/O 限制，预计运行时影响有限，但引入 CPU 密集型功能时应重新测量。

各阶段实测数据和取舍见 [构建体积优化记录](docs/size-optimization.md)。发布包不默认使用 UPX，以避免杀毒软件误报和启动时解压。

## 4. 推荐目录结构

```text
.
├── .github/
│   └── workflows/
│       ├── ci.yml
│       ├── release.yml
│       ├── integration-checkin.yml
│       └── docker.yml
├── config/
│   └── config.example.yaml
├── docs/
│   ├── configuration.md
│   ├── migration.md
│   └── security.md
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── cli.rs
│   ├── error.rs
│   ├── config/
│   │   ├── mod.rs
│   │   ├── model.rs
│   │   ├── loader.rs
│   │   └── legacy.rs
│   ├── http/
│   │   ├── mod.rs
│   │   ├── client.rs
│   │   ├── headers.rs
│   │   ├── proxy.rs
│   │   └── retry.rs
│   ├── auth/
│   │   ├── mod.rs
│   │   ├── cookie.rs
│   │   ├── stoken.rs
│   │   └── device.rs
│   ├── checkin/
│   │   ├── mod.rs
│   │   ├── china.rs
│   │   ├── hoyolab.rs
│   │   ├── games.rs
│   │   └── response.rs
│   ├── bbs/
│   │   ├── mod.rs
│   │   ├── posts.rs
│   │   ├── tasks.rs
│   │   └── rewards.rs
│   ├── cloud_game/
│   │   ├── mod.rs
│   │   ├── china.rs
│   │   └── overseas.rs
│   ├── captcha/
│   │   └── mod.rs
│   ├── push/
│   │   ├── mod.rs
│   │   ├── model.rs
│   │   └── providers/
│   ├── scheduler/
│   │   └── mod.rs
│   └── service/
│       ├── mod.rs
│       ├── runner.rs
│       └── report.rs
├── tests/
│   ├── config_compatibility.rs
│   ├── checkin_api.rs
│   └── fixtures/
├── Cargo.toml
├── Cargo.lock
├── Dockerfile
├── LICENSE
└── README.md
```

初期不必创建所有空文件。应按里程碑逐步增加模块，避免产生大量没有实现的占位代码。

## 5. CLI 设计

推荐统一入口：

```text
MihoyoBBSToolsRS checkin
MihoyoBBSToolsRS run
MihoyoBBSToolsRS validate-config
MihoyoBBSToolsRS migrate-config
MihoyoBBSToolsRS print-example-config
MihoyoBBSToolsRS version
```

含义：

- `checkin`：只执行游戏签到。
- `run`：执行账号刷新、游戏签到、社区任务、云游戏和推送等完整流程。
- `validate-config`：只校验配置，不访问远程接口。
- `migrate-config`：将旧版配置转换为新版配置。
- `print-example-config`：输出脱敏示例配置。
- `version`：输出版本、Git 提交和目标平台。

所有命令都必须返回有意义的进程退出码：

| 退出码 | 含义 |
|---:|---|
| 0 | 全部成功，或无需重复执行 |
| 1 | 存在普通任务失败 |
| 2 | 配置无效 |
| 3 | 认证信息无效或过期 |
| 4 | 验证码阻止执行 |
| 5 | 网络或代理故障 |
| 10 | 未分类内部错误 |

## 6. 配置设计

第一阶段尽量兼容当前 YAML 配置，同时定义一个结构清晰的新格式。

建议的新配置示例：

```yaml
version: 1

runtime:
  timezone: Asia/Shanghai
  request_timeout_seconds: 30
  retry_count: 3
  random_delay_seconds: 10
  log_level: info

captcha:
  endpoint: null

accounts:
  - name: example
    enabled: true

    credentials:
      cookie: "${MIHOYO_COOKIE}"
      stoken: "${MIHOYO_STOKEN}"

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
```

### 配置要求

- 支持 `${ENV_NAME}` 环境变量替换。
- 环境变量不存在时返回明确错误，不得把原始 Secret 打进日志。
- 账号名称必须唯一。
- 未知字段默认应产生警告；关键配置未知值应报错。
- Cookie 和 Token 使用敏感字符串包装，不实现普通 `Debug` 输出。
- 配置校验必须是独立函数，并有单元测试。
- 提供旧配置兼容层，但核心业务只能使用统一后的内部配置模型。

## 7. 核心抽象

### 7.1 HTTP 客户端

所有网络访问必须通过统一客户端，不允许业务模块各自创建随意配置的 `reqwest::Client`。

统一客户端负责：

- 默认超时。
- 重试和退避。
- HTTP/HTTPS/SOCKS 代理。
- User-Agent。
- Cookie 和公共请求头。
- 响应状态检查。
- JSON 反序列化错误上下文。
- 对请求头、查询参数和响应日志进行脱敏。

默认只重试适合重试的请求：连接失败、超时、部分 5xx。签到 POST 是否重试必须谨慎，优先在重试前查询签到状态，防止重复提交。

### 7.2 任务接口

建议为所有任务定义一致的异步接口：

```rust
#[async_trait::async_trait]
pub trait Task {
    fn name(&self) -> &'static str;
    async fn execute(&self, context: &TaskContext) -> TaskResult;
}
```

`TaskResult` 不应只返回布尔值，而应记录：

- 任务名称。
- 账号名称。
- 游戏或业务类型。
- 状态：成功、已完成、跳过、失败、需要验证码。
- 对用户安全的消息。
- 内部错误分类。
- 开始时间和耗时。

### 7.3 汇总报告

所有任务完成后生成统一报告，再交给推送模块。推送提供商不得直接依赖签到模块的数据结构。

## 8. 功能迁移映射

| Python 模块 | Rust 目标模块 | 迁移阶段 |
|---|---|---:|
| `config.py` | `config` | 1 |
| `request.py` | `http` | 1 |
| `error.py` | `error.rs` | 1 |
| `loghelper.py` | `tracing` 初始化 | 1 |
| `tools.py` | `auth/device` 与通用工具 | 2 |
| `login.py` | `auth` | 2 |
| `account.py` | `auth`、`service` | 2 |
| `gamecheckin.py` | `checkin/china.rs` | 3 |
| `hoyo_checkin.py` | `checkin/hoyolab.rs` | 4 |
| `mihoyobbs.py` | `bbs` | 5 |
| `captcha.py` | `captcha` | 5 |
| `cloudgames.py` | `cloud_game/china.rs` | 6 |
| `os_cloudgames.py` | `cloud_game/overseas.rs` | 6 |
| `web_activity.py` | 独立活动任务 | 6 |
| `push.py` | `push/providers` | 7 |
| `main.py` | `service/runner.rs` | 8 |
| `main_multi.py` | 统一账号循环 | 8 |
| `docker.py` | 外部调度或 `scheduler` | 8 |
| `server.py` | 后续可选服务模式 | 9 |

## 9. 实施里程碑

### 当前实现进度

截至 `codex/size-optimization` 实验分支，各阶段状态如下。本节中的依赖与体积优化尚未合并到 `main`，不代表主分支当前状态。标记“已完成”表示代码、离线测试和普通 CI 已具备；真实接口、镜像或发布产物仍需人工验证的项目不会提前标记完成。

| 阶段 | 状态 | 说明 |
|---|---|---|
| 阶段 0：初始化仓库 | 已完成 | Rust 2024 工程、README、许可证、CI 与 `Cargo.lock` 已提交；CI、Release 和 Docker 均使用锁定依赖。 |
| 阶段 1：配置、日志与 HTTP | 大部分完成 | YAML、多账号、环境变量替换、校验、代理、重试、全局日志级别、按日文件日志和 Secret 脱敏已实现；HTTP 响应体限制及更完整的重试/超时合约测试仍需补齐。`RUST_LOG` 仅接受全局级别，不支持模块过滤表达式。 |
| 阶段 2：账号与认证 | 大部分完成 | Cookie、SToken、登录票据与 `cookie_token` 刷新、设备 ID/FP 请求契约、DS 与 MD5 已实现；HMAC-SHA256 已用于钉钉，通用签名抽象和固定测试向量仍待补充。 |
| 阶段 3：国内游戏签到 | 代码完成，待真实验收 | 六款国服游戏、角色查询、状态查询、签到、成功复查、当天奖励详情、设备 ID 与错误分类已实现；真实账号仍需人工验证。 |
| 阶段 4：HoYoLAB 国际服签到 | 代码完成，待真实验收 | 五款国际服游戏、多账号、独立接口、成功复查、当天奖励详情与统一报告已实现；真实账号仍需人工验证。 |
| 阶段 5：米游社社区任务和验证码 | 代码完成，待真实验收 | 可配置社区板块、签到、阅读、点赞、取消点赞、分享、动作后任务状态确认、设备 FP、米游币汇总及验证码闭环已实现；真实账号仍需人工验收。 |
| 阶段 6：云游戏与 Web 活动 | 未开始 | 国内/国际云游戏和 Web 活动尚未迁移。 |
| 阶段 7：推送 | 部分完成 | 已实现 Telegram、Webhook、PushPlus、Server酱、企业微信、钉钉、飞书、Bark、Gotify、Discord、WxPusher 等主要网络通知渠道；SMTP 与 Windows 本地通知待补充。 |
| 阶段 8：完整运行、Docker 与迁移 | 部分完成 | `run`、旧配置迁移、Docker、Release 工作流及 Linux/Windows 发布附件已实现并验证；定时工作流和 Docker 远程运行验收仍缺失。 |
| 阶段 9：可选服务端模式 | 未开始（可选） | 暂无实现需求。 |

### 尚未完成的主要工作

按阻塞程度排序：

1. 增加手动真实接口测试工作流和人工验收记录，验证国内签到、HoYoLAB 与米游社任务。
2. 使用真实账号验收验证码求解闭环和首批推送渠道。
3. 迁移国内/国际云游戏和 Web 活动，并避免配置开启后被静默忽略。
4. 按实际需求补充 SMTP 邮件和 Windows 本地通知。
5. 增加 GitHub Actions 定时运行，并实际消费时区和随机延迟配置。
6. 增加 Docker 构建/运行工作流，验证最终镜像可在无 Rust 环境运行。
7. 补齐通用 SHA256/HMAC 签名抽象及固定测试向量、HTTP 响应体大小限制、重试/超时合约测试和多账号故障隔离测试。
8. 增加 Secret 历史扫描、依赖许可证清单及 arm64/armv7 构建验证。

### 已按实现完成的近期项目

- [x] 国内签到和米游社任务在认证失效后使用 SToken 刷新 `cookie_token`、重试，并在安全条件允许时写回配置。
- [x] 国内与 HoYoLAB 签到提交后再次确认服务端状态，并显示当天奖励详情。
- [x] 米游社社区签到、阅读、点赞和分享提交后重新查询任务状态，避免仅凭动作接口误报成功。
- [x] 社区签到板块支持账号级配置，默认顺序为大别野与原神，并迁移旧版 `checkin_list`。
- [x] 国内签到发送设备 ID；米游社 App 类请求发送设备 ID/名称/型号及非空 FP；不需要设备字段的接口保持不发送。
- [x] 提交 `Cargo.lock`，CI、Release 与 Docker 使用锁定依赖。
- [x] TLS provider 切换为 `ring`，CI 阻止 AWS-LC 依赖回归，并通过公开端点真实 HTTPS 握手测试。
- [x] 昵称查询改为异步请求并复用统一 HTTP 客户端，移除 `reqwest/blocking`。
- [x] 精简 Tokio runtime、日志过滤依赖和未使用的错误/Secret 依赖。
- [x] CI 输出 Linux、Windows 原始程序和发布压缩包体积。

### 阶段 0：初始化仓库

交付物：

- 创建名为 `mihoyo-bbs-tools-rs` 的 GitHub 仓库。
- Rust 2024 项目。
- Cargo 包名保持为 `mihoyo-bbs-tools`，可执行文件名为 `MihoyoBBSToolsRS`。
- `LICENSE`、`README.md`、`.gitignore`。
- README 包含非官方项目声明。
- CI 工作流。
- `cargo fmt`、`cargo clippy`、`cargo test`、Linux/Windows 编译全部通过。
- 程序只需支持 `version` 命令。

验收条件：

- 本地未安装 Rust也能通过推送触发完整构建。
- `Cargo.lock` 已提交。
- CI 权限为最小化的 `contents: read`。

### 阶段 1：配置、日志与 HTTP

交付物：

- YAML 配置模型。
- 环境变量替换。
- `validate-config`。
- 统一 HTTP 客户端。
- 超时、代理、重试和脱敏日志。
- 使用 Wiremock 测试请求头、代理配置外的请求行为和错误响应。

### 阶段 2：账号与认证

交付物：

- Cookie 解析。
- SToken、账号 ID 和设备信息模型。
- 登录凭据刷新流程。
- DS、MD5、SHA256/HMAC 等签名工具。
- 固定测试向量，不访问真实接口即可验证签名结果。

### 阶段 3：国内游戏签到

交付物：

- 国内角色查询。
- 国内游戏签到状态查询。
- 签到执行。
- 已签到、Cookie 失效、无角色、验证码等状态分类。
- 至少覆盖原神、星穹铁道和绝区零，再补其他游戏。

### 阶段 4：HoYoLAB 国际服签到

交付物：

- 原神、崩坏3、星穹铁道、未定事件簿、绝区零。
- 多账号和每账号启用游戏列表。
- 与国内签到共享报告模型，但不共享错误的请求头和接口常量。

### 阶段 5：米游社社区任务和验证码

交付物：

- 社区任务状态查询。
- 帖子浏览、点赞、取消点赞、分享等现有流程。
- 米游币任务汇总。
- 验证码服务客户端。
- 遇到验证码时停止高风险重复请求，并生成明确报告。

### 阶段 6：云游戏与 Web 活动

交付物：

- 国内云游戏。
- 国际云游戏。
- Web 活动任务。
- 每个功能都可以单独关闭。

### 阶段 7：推送

优先迁移常用渠道，不要求第一批实现当前 `push.py` 中全部渠道。

建议顺序：

1. Telegram。
2. 企业微信机器人。
3. 钉钉机器人。
4. Server酱。
5. Bark。
6. Discord。
7. 其他兼容渠道。

推送失败不应覆盖核心签到结果，但应使最终报告显示部分失败。

### 阶段 8：完整运行、Docker 与迁移

交付物：

- `run` 完整流程。
- 旧 YAML 配置迁移。
- GitHub Actions 定时执行。
- Docker 多阶段镜像。
- Linux 和 Windows Release。
- 文档说明如何从 Python 版本迁移。

### 阶段 9：可选服务端模式

只有确实需要长期运行的 Web 管理或内部调度时才实现。不要为了复刻旧 `server.py` 而提前引入 Web 框架。

如需要 Web API，优先考虑 `axum`，并单独进行身份认证、CSRF、限流和 Secret 存储设计。

## 10. GitHub Actions 设计

### 10.1 持续集成 `.github/workflows/ci.yml`

```yaml
name: Rust CI

on:
  workflow_dispatch:
  push:
    branches:
      - main
      - "codex/**"
    paths:
      - "**/*.rs"
      - "Cargo.toml"
      - "Cargo.lock"
      - ".github/workflows/ci.yml"
  pull_request:
    paths:
      - "**/*.rs"
      - "Cargo.toml"
      - "Cargo.lock"
      - ".github/workflows/ci.yml"

permissions:
  contents: read

concurrency:
  group: rust-ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

jobs:
  quality:
    name: 格式、静态检查与测试
    runs-on: ubuntu-latest

    steps:
      - name: 检出代码
        uses: actions/checkout@v4

      - name: 安装 Rust
        run: |
          rustup toolchain install stable --profile minimal --component rustfmt,clippy
          rustup default stable

      - name: 检查格式
        run: cargo fmt --all -- --check

      - name: Clippy 静态检查
        run: cargo clippy --all-targets --all-features --locked -- -D warnings

      - name: 执行测试
        run: cargo test --all-features --locked

  build:
    name: 编译 ${{ matrix.os }}
    runs-on: ${{ matrix.os }}

    strategy:
      fail-fast: false
      matrix:
        os:
          - ubuntu-latest
          - windows-latest

    steps:
      - name: 检出代码
        uses: actions/checkout@v4

      - name: 安装 Rust
        shell: bash
        run: |
          rustup toolchain install stable --profile minimal
          rustup default stable

      - name: 编译 Release
        run: cargo build --release --locked
```

初期不加入第三方 Cargo 缓存 Action，先确保流程稳定。编译时间明显影响开发后，再评估缓存，并固定第三方 Action 的提交 SHA。

当前 CI、Release 和 Docker 全部使用已提交的 `Cargo.lock` 与 `--locked`。CI 还检查普通依赖树，防止 AWS-LC 被间接依赖重新引入；Linux 和 Windows Release 构建会把原始程序与压缩包的精确字节数写入 Actions Summary。公开 HTTPS 握手测试保存在默认忽略的 `tests/tls_smoke.rs`，由独立工作流在 Ubuntu 和 Windows 上执行，不混入普通离线测试。

### 10.2 Release 工作流

Release 工作流只在版本标签触发：

```yaml
on:
  push:
    tags:
      - "v*"
```

至少发布：

- Linux x86_64。
- Windows x86_64。
- SHA256 校验文件。

ARM64、ARMv7 和 Docker 多架构镜像应在核心功能稳定后增加，避免初期把时间消耗在交叉编译问题上。

### 10.3 真实签到集成测试

真实接口测试必须与普通 CI 分离，只允许手动触发：

```yaml
name: 真实签到测试

on:
  workflow_dispatch:
    inputs:
      account:
        description: 要测试的脱敏账号名称
        required: false
        type: string

permissions:
  contents: read

jobs:
  checkin:
    runs-on: ubuntu-latest
    environment: integration

    steps:
      - uses: actions/checkout@v4

      - name: 安装 Rust
        run: |
          rustup toolchain install stable --profile minimal
          rustup default stable

      - name: 编译并执行
        env:
          MIHOYO_COOKIE: ${{ secrets.MIHOYO_COOKIE }}
          MIHOYO_STOKEN: ${{ secrets.MIHOYO_STOKEN }}
          CAPTCHA_ENDPOINT: ${{ secrets.CAPTCHA_ENDPOINT }}
        run: cargo run --release --locked -- checkin
```

推荐为 `integration` Environment 配置人工批准，防止普通提交自动使用真实 Cookie。

真实测试要求：

- 不在 Pull Request 中自动运行。
- 不接受来自 Fork 的 Secret。
- 不打印 Cookie、SToken、代理密码或完整请求头。
- 不上传包含配置文件的 Artifact。
- 不缓存运行时配置和账号信息。
- 先测试状态查询，再逐步开放实际签到请求。

### 10.4 定时运行

功能稳定后增加独立工作流：

```yaml
on:
  schedule:
    - cron: "5 16 * * *"
  workflow_dispatch:
```

GitHub Cron 使用 UTC，`16:05 UTC` 对应北京时间次日 `00:05`。定时任务可能延迟，不应依赖秒级准确执行。

## 11. 测试策略

### 11.1 单元测试

必须覆盖：

- Cookie 解析。
- 环境变量替换。
- YAML 配置校验。
- DS、MD5、HMAC 等签名固定向量。
- 游戏名称与 API 参数映射。
- 错误码到业务状态的转换。
- 日志脱敏函数。
- 汇总报告格式。

### 11.2 HTTP 合约测试

使用 Wiremock 或等价本地 Mock 服务验证：

- 请求方法和 URL。
- 必要请求头。
- Query 和 JSON Body。
- 成功响应。
- 已签到响应。
- Cookie 失效。
- 验证码。
- 429、5xx、超时和无效 JSON。
- 重试次数和禁止重试的请求。

测试不得依赖真实米哈游接口，否则 CI 会不稳定，并可能触发风控。

### 11.3 配置兼容测试

从旧项目抽取脱敏后的配置样例作为 Fixture。不得提交真实 Cookie 或历史日志。

测试内容：

- 旧单账号配置可读取。
- 旧多账号配置可读取。
- 缺失字段使用安全默认值。
- 无法迁移的字段产生明确警告。
- 转换后的配置再次读取结果一致。

## 12. 安全要求

### Secret 管理

- GitHub Secrets 保存真实凭据。
- 仓库只提交 `${ENV_NAME}` 占位符。
- `.gitignore` 排除真实配置、日志和临时文件。
- 不允许将 Cookie 作为 CLI 参数，因为参数可能出现在进程列表和 Actions 日志中。
- 优先通过环境变量或受控配置文件读取。

### 日志脱敏

以下内容必须脱敏：

- `Cookie` 请求头。
- `Authorization`。
- SToken、LToken、Cookie Token。
- Telegram Bot Token。
- Webhook URL 中的密钥。
- 代理用户名和密码。
- 完整账号 ID；默认只显示尾部少量字符。

禁止直接记录完整的 `reqwest::Request`、配置结构体或环境变量集合。

### 网络安全

- 默认验证 TLS 证书。
- 不提供全局关闭证书验证的配置。
- 验证码 Endpoint 和代理 URL 必须校验协议。
- Webhook 重定向不得导致敏感请求头发送到不同主机。
- 设置响应体大小上限，避免异常服务返回无限数据。

## 13. Docker 策略

Docker 只用于最终运行和发布，不作为开发机器的必要条件。

建议多阶段镜像：

```dockerfile
FROM rust:1-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

FROM alpine:3

RUN apk add --no-cache ca-certificates tzdata

ENV TZ=Asia/Shanghai
WORKDIR /app

COPY --from=builder /app/target/release/MihoyoBBSToolsRS /usr/local/bin/MihoyoBBSToolsRS

ENTRYPOINT ["MihoyoBBSToolsRS"]
CMD ["run"]
```

Docker 镜像由 GitHub Actions 构建，不要求开发机执行 `docker build`。

第一阶段只发布 `linux/amd64`。稳定后再使用 Buildx 增加：

```text
linux/amd64
linux/arm64
linux/arm/v7
```

## 14. 从 Python 项目迁移的方法

迁移每个模块时遵循以下步骤：

1. 记录 Python 模块的输入、输出和外部副作用。
2. 把 API URL、游戏 ID、请求头和错误码整理为测试 Fixture。
3. 先为 Rust 模块编写 Mock HTTP 测试。
4. 实现最小功能使测试通过。
5. 在 Actions 中通过 fmt、Clippy、测试和编译。
6. 使用手动真实测试验证一个测试账号。
7. 确认日志没有泄露 Secret。
8. 再扩展到多账号和其他游戏。

不要机械逐行翻译 Python。应保留业务行为，重新设计类型、错误和模块边界。

## 15. 功能完成标准

一个功能只有同时满足以下条件才算完成：

- 有清晰的配置入口。
- 有明确的 Rust 类型和错误类型。
- 有成功、失败和边界条件测试。
- 普通测试不访问真实接口。
- 日志已脱敏。
- Clippy 无警告。
- Linux 与 Windows 编译通过。
- README 或相关文档已更新。
- 真实测试结果经过人工确认。
- 不破坏现有已完成模块。

## 16. 第一批任务清单

新仓库创建后，按以下顺序实施：

- [x] 创建 `mihoyo-bbs-tools-rs` 仓库和同名项目目录。
- [x] 初始化 Cargo 包名为 `mihoyo-bbs-tools`、可执行文件名为 `MihoyoBBSToolsRS` 的 Rust 2024 二进制项目。
- [x] 添加中文 README 和许可证。
- [x] 添加 `ci.yml`。
- [x] 实现 `version` 命令。
- [x] 推送并确保 Linux/Windows CI 通过。
- [x] 添加 YAML 配置模型和示例。
- [x] 实现 `validate-config`。
- [x] 添加环境变量替换和 Secret 脱敏。
- [x] 实现统一 HTTP 客户端。
- [x] 使用 Mock 服务测试 HTTP 行为。
- [x] 实现 Cookie 解析和签名工具。
- [x] 实现一个国内游戏的状态查询。
- [ ] 手动运行真实接口测试。
- [x] 实现该游戏的实际签到。
- [x] 扩展其他国内游戏。
- [x] 实现 HoYoLAB 五款游戏。
- [x] 迁移米游社社区任务。
- [x] 迁移验证码平台。
- [ ] 迁移云游戏。
- [x] 实现首批推送渠道。
- [x] 添加并验证 Release 工作流和 Linux/Windows 发布附件。
- [x] 添加 Docker 多阶段镜像。
- [ ] 添加 GitHub Actions 定时工作流。

## 17. 每次提交前的远程检查

由于本地不安装 Rust，每次提交后检查 GitHub Actions 中的以下结果：

```text
1. cargo fmt --all -- --check
2. cargo clippy --all-targets --all-features --locked -- -D warnings
3. cargo test --all-features --locked
4. cargo build --release --locked（Linux）
5. cargo build --release --locked（Windows）
```

修复顺序建议：

1. YAML 工作流语法错误。
2. Rust 编译错误。
3. 格式错误。
4. Clippy 警告。
5. 单元测试失败。
6. 平台专属编译错误。
7. 真实接口错误。

不要在代码尚未编译时反复触发真实签到测试。

## 18. 最终验收清单

- [x] 新版可以读取至少一种旧版配置。
- [x] 国内和国际签到均支持多账号。
- [ ] 社区任务结果与 Python 版本基本一致。
- [ ] 云游戏和 Web 活动可以单独启用或关闭。
- [x] 推送汇总不会泄露敏感信息。
- [x] Cookie 无效和验证码状态可明确区分。
- [ ] GitHub Actions 可以定时运行。
- [x] Linux 和 Windows Release 可以下载。
- [ ] Docker 镜像可在无 Rust 环境中运行。
- [ ] amd64 稳定后再验证 arm64、armv7。
- [ ] 仓库历史中不存在真实 Cookie、Token 和日志。
- [ ] 所有第三方依赖和参考代码许可证清晰。

## 19. 推荐的第一个 Pull Request

第一个 PR 只完成以下内容：

```text
Rust 项目骨架
CLI version 命令
基础错误类型
Tracing 日志初始化
GitHub Actions CI
Linux/Windows Release 编译
README 中的开发说明
```

不要在第一个 PR 同时实现签到接口。先验证“完全依赖 GitHub Actions 开发”的工作流是否顺畅，再进入业务迁移。
