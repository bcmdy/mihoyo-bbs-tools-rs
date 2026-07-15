# MihoyoBBSTools RS

面向 Windows 与 Linux 的米游社、HoYoLAB 自动签到和社区任务工具。支持多账号、验证码平台、代理、日志及多种通知渠道。

> MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系。使用前请自行确认账号安全和相关服务条款。

[下载最新版本](https://github.com/bcmdy/mihoyo-bbs-tools-rs/releases/latest) · [快速开始](docs/快速开始.md) · [完整使用说明](docs/使用说明.md) · [YAML 配置参考](docs/configuration.md) · [安全说明](docs/security.md)

## 主要功能

- 国内游戏签到与 HoYoLAB 国际服游戏签到。
- 米游社社区签到，可独立选择板块；阅读、点赞、取消点赞和分享均可单独开关。
- 国内云原神、云绝区零和国际服云原神签到。
- 多账号独立 Cookie、任务、设备、代理和游戏配置，并统一汇总运行结果与通知。
- 游戏签到提交后复查、失败时按配置重试，并显示累计天数和当天奖励。
- 国内 Cookie 凭据失效时使用 SToken 自动刷新；普通 YAML 可安全写回。
- 验证码平台、HTTP/HTTPS/SOCKS 代理和 Telegram 独立代理。
- Telegram、微信相关渠道、SMTP、Webhook、Windows 本地通知等多种推送方式。
- 按日滚动文件日志、结构化 JSON 报告、目录批量运行和常驻调度。
- Windows x86_64、Linux x86_64/ARM64/ARMv7 发布包及多架构容器镜像。

## 下载

从 [GitHub Releases](https://github.com/bcmdy/mihoyo-bbs-tools-rs/releases) 下载与系统匹配的压缩包：

| 系统 | 架构 | 发布包名称 |
|---|---|---|
| Windows | x86_64 | `MihoyoBBSToolsRS-<版本>-windows-x86_64.zip` |
| Linux | x86_64 | `MihoyoBBSToolsRS-<版本>-linux-x86_64.tar.gz` |
| Linux | ARM64 / aarch64 | `MihoyoBBSToolsRS-<版本>-linux-arm64.tar.gz` |
| Linux | ARMv7 | `MihoyoBBSToolsRS-<版本>-linux-armv7.tar.gz` |

发布包名称带版本号，包内程序名始终为 `MihoyoBBSToolsRS.exe` 或 `MihoyoBBSToolsRS`，升级时可直接替换程序。每个发布包旁的 `.sha256` 文件用于校验下载内容。

发布包同时包含：

- `快速开始.md`、`使用说明.md`、`configuration.md` 和 `security.md`；
- `config/config.example.yaml` 完整配置模板；
- 可选的 `dacapo/template.yml` 第三方适配模板。

## 两步开始使用

Windows：

```powershell
.\MihoyoBBSToolsRS.exe config add-account
.\MihoyoBBSToolsRS.exe run
```

Linux：

```bash
chmod +x MihoyoBBSToolsRS
./MihoyoBBSToolsRS config add-account
./MihoyoBBSToolsRS run
```

执行 `config add-account` 后按提示粘贴完整 Cookie。程序会提取 SToken、获取米游社昵称，并在配置不存在时自动创建 `config/config.yaml`。添加其他账号时再次执行同一命令；需要备注时使用 `--name "小号"`。

Cookie、SToken 和通知 Token 都属于账号凭据。只在程序提示后粘贴 Cookie，不要把它写进命令参数、截图、日志、Git 提交或公开聊天。

更详细的下载、终端和故障排查步骤见 [快速开始](docs/快速开始.md)。

## 默认会执行什么

通过 `config add-account` 新建的账号默认只启用：

- 原神国内游戏签到；
- 米游社大别野和原神社区签到。

阅读、点赞、取消点赞、分享、HoYoLAB、云游戏和 Web 活动默认关闭。`run` 只执行配置中已经启用的账号、游戏和任务；`run --task` 只能进一步缩小本次范围，不能重新启用配置中已关闭的功能。旧配置和迁移后的配置会保留原有任务选择，不保证与新模板默认值相同。

## 常用命令

| 命令 | 用途 |
|---|---|
| `config add-account` | 安全输入 Cookie，添加账号或自动创建配置 |
| `config setup` | 用数字菜单修改运行、账号、任务、通知等配置 |
| `run` | 执行配置中已启用的全部任务 |
| `checkin --region china` | 只执行国内游戏签到 |
| `validate-config` | 校验 YAML，不访问远程接口 |
| `create-launcher` | Windows 下生成可移动的异步启动 BAT |
| `version`、`-V`、`--version` | 查看程序版本 |
| `<命令> --help` | 查看某个子命令的完整参数 |

按需使用的命令：

| 命令 | 用途 |
|---|---|
| `run --task china-checkin,bbs` | 临时限定本次任务范围 |
| `schedule` | 按 `runtime.schedule` 常驻串行运行 |
| `run-directory` | 依次运行目录中的多个 YAML |
| `migrate-config` | 将 Python v11–v15 配置迁移为新版 YAML |
| `print-example-config` | 输出当前版本的完整脱敏模板 |
| `qinglong` / `ql` | 使用原 Python 项目的青龙环境变量入口 |
| `dacapo` | 读取 DaCapo 生成的 JSON，以只读内存配置运行 |

完整参数、任务值、退出码和自动运行方式见 [使用说明](docs/使用说明.md)。

## 配置与账号

默认配置路径为 `config/config.yaml`。推荐使用：

```powershell
.\MihoyoBBSToolsRS.exe config setup
.\MihoyoBBSToolsRS.exe validate-config
```

数字菜单可以配置全部常用节点，包括多账号、国内与 HoYoLAB 游戏、社区板块、云游戏、角色黑名单、设备 ID/FP、代理、验证码、日志、调度及全部通知渠道。熟悉 YAML 的用户可参考 [配置字段说明](docs/configuration.md) 或运行 `config edit`。

相关文档：

- [完整使用说明](docs/使用说明.md)：从首次配置到通知、日志、自动运行和常见问题。
- [YAML 配置参考](docs/configuration.md)：字段默认值、取值范围和次级节点。
- [旧配置迁移](docs/config-migration.md)：Python v11–v15 配置迁移方法。
- [安全说明](docs/security.md)：凭据、日志、网络和部署安全要求。

## 自动运行与可选部署

普通 Windows 用户可以运行 `create-launcher` 生成异步启动 BAT，或使用任务计划程序定时执行 `run`。Linux 可使用 cron；需要程序常驻时可启用 `runtime.schedule` 后运行 `schedule`。

仓库还提供以下可选方式，普通桌面用户无需配置：

| 方式 | 说明 |
|---|---|
| GitHub Actions | 使用 Secret 中的完整 YAML 每日执行一次；fork 默认不会运行真实任务 |
| Docker / GHCR | `amd64`、`arm64`、`arm/v7` 多架构镜像 |
| Kubernetes | [CronJob 示例](deploy/kubernetes/README.md)，通过 Secret 只读挂载配置 |
| Nix / NixOS | 支持 `x86_64-linux` 与 `aarch64-linux` |
| 青龙 | 兼容原项目常用环境变量，不加载外部 `notify.py` |
| 多配置目录 | 串行执行多个 YAML，单个配置失败不阻止后续文件 |

DaCapo 是可选的第三方 JSON 配置适配入口，不是签到任务或必需组件。普通用户无需安装或配置 DaCapo，可以忽略 `dacapo` 命令和发布包中的 `dacapo` 目录，优先使用 `config add-account`、`config setup` 和 `run`。

## 安全提示

- 真实配置应保存在受控目录并加入 `.gitignore`，不要上传为 Actions Artifact。
- 自动化环境优先使用环境变量、GitHub Secrets、Kubernetes Secret 或标准输入注入 YAML。
- `run --config - --read-only --no-notify --output json` 适合只读自动化调用。
- 不要关闭 TLS 证书校验，不要把含认证信息的代理或 Webhook 地址发到公开位置。
- 日志默认保存到 `logs/mihoyo-bbs-tools_YYYY-MM-DD.log`，反馈问题前仍应人工检查并脱敏。

## 开发与项目状态

本项目使用 GitHub Actions 完成 Rust 格式检查、Clippy、测试、Linux/Windows 构建、Nix 构建、TLS 握手和多架构容器验证。实现状态与仍需人工验收的项目见 [重构实施文档](RUST_REWRITE_GUIDE.md#当前实现进度)，依赖与体积优化记录见 [构建体积优化说明](docs/size-optimization.md)。

项目标识：

- GitHub 仓库：`bcmdy/mihoyo-bbs-tools-rs`
- Cargo 包：`mihoyo-bbs-tools`
- 可执行文件：`MihoyoBBSToolsRS`
- Rust Edition：2024

## 许可证

本项目采用 [MIT License](LICENSE)。从原 MihoyoBBSTools 迁移或改写的部分保留原项目版权声明。相关商标归各自权利人所有；项目不使用官方 Logo、角色头像或其他容易使用户误认为官方产品的视觉元素。
