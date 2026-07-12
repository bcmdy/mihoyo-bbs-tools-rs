# MihoyoBBSTools RS

MihoyoBBSTools 的 Rust 重构项目，目标是在保持主要配置和部署方式兼容的同时，提供更安全、可测试并可跨平台发布的米游社与 HoYoLAB 自动任务工具。

> 当前项目已完成核心签到、米游社任务、验证码平台和首批推送渠道重构，仍按实施文档继续迁移云游戏、Web 活动和调度发布能力。实际进度以主分支代码和 GitHub Actions 结果为准。

## 项目标识

- GitHub 仓库名：`mihoyo-bbs-tools-rs`
- Cargo 包名：`mihoyo-bbs-tools`
- 可执行文件名：`mihoyo-bbs-tools`
- Rust Edition：2024

## 计划能力

- 国内米游社与国际 HoYoLAB 游戏签到。
- 单账号、多账号和每账号任务开关。
- 米游社社区任务、云游戏和现有 Web 活动。
- 验证码服务、HTTP/HTTPS/SOCKS 代理和多渠道统一推送报告。
- YAML、环境变量与 GitHub Secrets 配置。
- GitHub Actions、Docker、Linux 和 Windows 发布。

功能按照 [重构实施文档](RUST_REWRITE_GUIDE.md) 分阶段迁移。当前已完成新版配置、旧配置迁移、国内与 HoYoLAB 签到、米游社任务、验证码求解及原项目主要网络通知渠道；云游戏、Web 活动、SMTP 邮件和 Windows 本地通知仍在后续阶段。

详细完成情况和剩余工作见 [重构实施文档的当前进度](RUST_REWRITE_GUIDE.md#当前实现进度)。

## 命令行

当前可用命令：

```text
mihoyo-bbs-tools version
mihoyo-bbs-tools validate-config
mihoyo-bbs-tools print-example-config
mihoyo-bbs-tools checkin
mihoyo-bbs-tools checkin --region china|hoyolab|all
mihoyo-bbs-tools migrate-config SOURCE [TARGET]
mihoyo-bbs-tools migrate-config --input SOURCE [--output TARGET]
mihoyo-bbs-tools run
mihoyo-bbs-tools run --task china-checkin,hoyolab-checkin,bbs
mihoyo-bbs-tools config edit
mihoyo-bbs-tools config add-account --name "备注"
mihoyo-bbs-tools config remove-account "备注"
mihoyo-bbs-tools config setup
```

配置格式和环境变量规则见 [配置说明](docs/configuration.md)，凭据保护与日志要求见 [安全说明](docs/security.md)。

`run --task` 可以临时缩小本次运行范围，可选值为 `china-checkin`、`hoyolab-checkin` 和 `bbs`；不提供时仍按原顺序尝试全部已实现任务。`checkin --region` 可选择 `china`、`hoyolab` 或 `all`，默认值为 `all`。这些命令行选项不会绕过配置文件中的账号、任务或游戏禁用状态。

首次使用时可以直接运行 `config add-account`。即使默认的 `config/config.yaml` 及其父目录尚不存在，程序也会在 Cookie 和账号信息校验成功后创建只包含该账号的新配置。`config setup` 提供显式进入的数字设置菜单；普通运行命令不会自动进入交互界面，因此不会阻塞 CI、Docker 或计划任务。

配置迁移同时支持位置参数和原有的 `-i/--input`、`-o/--output` 选项。省略输出路径时，会在输入文件同目录生成 `.migrated.yaml` 或 `.migrated.yml` 文件；详细命名和不覆盖规则见 [配置迁移说明](docs/config-migration.md)。

## 设备配置

设备信息按账号配置，字段顺序建议使用 `name`、`model`、`id`、`fp`：

```yaml
device:
  name: "Xiaomi MI 6"
  model: "Mi 6"
  id: ""
  fp: ""
```

`id` 留空时会根据该账号的 Cookie 使用 UUID v3 确定性生成，因此 Cookie 变化会导致自动生成的设备 ID 改变；需要稳定设备身份时请填写固定值。`fp` 当前仅保存并在旧配置迁移时保留，尚未用于请求头或接口参数。完整规则见 [配置说明](docs/configuration.md#设备信息)。

## 开发与验证

本项目以 GitHub Actions 作为唯一的 Rust 编译与验证环境。开发机器只负责编辑文件和 Git 操作，不要求安装 Rust、MSVC、Windows SDK 或 Docker，也不在本地执行 Cargo 命令。

每个 Pull Request 必须通过：

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features --locked -- -D warnings`
- `cargo test --all-features --locked`
- Linux 和 Windows 的 `cargo build --release --locked`

真实接口测试必须与普通 CI 分离，只能手动触发，并应使用配置了人工批准的 GitHub Environment。普通测试只使用脱敏 Fixture 和 Mock HTTP 服务。

## Docker

仓库中的 `Dockerfile` 用于构建最终运行镜像。镜像采用多阶段构建，运行阶段不包含 Rust 工具链。镜像应由 GitHub Actions 构建，不要求开发机执行 `docker build`。

镜像默认执行完整任务流程：

```text
mihoyo-bbs-tools run
```

真实配置和凭据必须在运行时通过受控配置文件、环境变量或 Secret 注入，不能写入镜像。

## 许可证

本项目采用 [MIT License](LICENSE)。从原 MihoyoBBSTools 迁移或改写的部分保留原项目版权声明。

## 非官方声明

MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。

本项目不使用官方 Logo、角色头像或其他容易使用户误认为官方产品的视觉元素。
