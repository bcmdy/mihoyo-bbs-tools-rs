# MihoyoBBSTools RS

首次使用请看 [快速开始](docs/快速开始.md)；需要通知、代理或完整配置说明时再看 [详细使用说明](docs/使用说明.md)。发布包已包含两份说明文件和完整 YAML 配置模板。

MihoyoBBSTools 的 Rust 重构项目，目标是在保持主要配置和部署方式兼容的同时，提供更安全、可测试并可跨平台发布的米游社与 HoYoLAB 自动任务工具。

> 当前项目已完成核心签到、米游社任务、国内/国际云游戏、原项目 Web 活动状态处理、验证码平台和通知渠道重构，仍按实施文档继续迁移部署与平台适配能力。实际进度以主分支代码和 GitHub Actions 结果为准。

## 项目标识

- GitHub 仓库名：`mihoyo-bbs-tools-rs`
- Cargo 包名：`mihoyo-bbs-tools`
- 可执行文件名：`MihoyoBBSToolsRS`
- Rust Edition：2024

## 计划能力

- 国内米游社与国际 HoYoLAB 游戏签到。
- 单账号、多账号和每账号任务开关。
- 米游社社区任务、云游戏和现有 Web 活动。
- 验证码服务、HTTP/HTTPS/SOCKS 代理和多渠道统一推送报告。
- YAML、环境变量与 GitHub Secrets 配置。
- GitHub Actions、Docker、Linux 和 Windows 发布。

功能按照 [重构实施文档](RUST_REWRITE_GUIDE.md) 分阶段迁移。当前已完成新版配置、旧配置迁移、国内与 HoYoLAB 签到、米游社任务、国内/国际云游戏、原项目 Web 活动状态处理、验证码求解、SMTP 邮件、Windows 本地通知及原项目主要网络通知渠道。

详细完成情况和剩余工作见 [重构实施文档的当前进度](RUST_REWRITE_GUIDE.md#当前实现进度)。

## 命令行

当前可用命令：

```text
MihoyoBBSToolsRS version
MihoyoBBSToolsRS validate-config
MihoyoBBSToolsRS print-example-config
MihoyoBBSToolsRS create-launcher
MihoyoBBSToolsRS checkin
MihoyoBBSToolsRS checkin --region china|hoyolab|all
MihoyoBBSToolsRS migrate-config SOURCE [TARGET]
MihoyoBBSToolsRS migrate-config --input SOURCE [--output TARGET]
MihoyoBBSToolsRS run
MihoyoBBSToolsRS run --task china-checkin,hoyolab-checkin,bbs,china-cloud-game,overseas-cloud-game,web-activity
MihoyoBBSToolsRS run --config - --read-only --no-notify --output json
MihoyoBBSToolsRS run-directory config [--prefix mhy_]
MihoyoBBSToolsRS qinglong
MihoyoBBSToolsRS dacapo settings.json
MihoyoBBSToolsRS schedule
MihoyoBBSToolsRS config edit
MihoyoBBSToolsRS config add-account --name "备注"
MihoyoBBSToolsRS config remove-account "备注"
MihoyoBBSToolsRS config setup
```

配置格式和环境变量规则见 [配置说明](docs/configuration.md)，凭据保护与日志要求见 [安全说明](docs/security.md)。

新建账号默认仅执行原神游戏签到和米游社社区签到；阅读、点赞、取消点赞与分享默认关闭。文件日志默认写入 `logs/mihoyo-bbs-tools_YYYY-MM-DD.log` 并按天滚动，可通过 `runtime.logging` 修改或关闭。

`run --task` 可以临时缩小本次运行范围，可选值为 `china-checkin`、`hoyolab-checkin`、`bbs`、`china-cloud-game`、`overseas-cloud-game` 和 `web-activity`；不提供时仍按原顺序尝试全部已实现任务。`checkin --region` 可选择 `china`、`hoyolab` 或 `all`，默认值为 `all`。这些命令行选项不会绕过配置文件中的账号、任务或游戏禁用状态。

云函数和流水线可以使用 `run --config -` 从标准输入读取完整 YAML；该输入始终只读，不会尝试写回自动刷新的凭据。`--read-only` 可让普通配置文件也只读运行，`--no-notify` 禁止所有外部通知，`--output json` 让标准输出只包含一个结构化 JSON 对象；运行日志写入标准错误或配置的日志文件，不会混入 JSON。

`run-directory` 按文件名顺序执行目录中的 `.yaml`/`.yml`，自动排除 `*.example.yaml` 和 `*.example.yml`。`--prefix` 可限制文件名前缀，单个配置加载或任务失败不会阻止后续文件；各文件使用自己的账号、任务和通知设置。文件之间默认随机等待 3–10 秒，可通过 `--delay-min-seconds`、`--delay-max-seconds` 调整，均设为 `0` 可关闭。

`qinglong`（别名 `ql`）兼容原 Python 入口的 `AutoMihoyoBBS_config_path`、`AutoMihoyoBBS_config_prefix`、`AutoMihoyoBBS_config_multi`、`QL_DIR` 和 `AutoMihoyoBBS_push_project`。多配置模式复用 `run-directory` 的故障隔离与汇总；Rust 版不会动态加载青龙 `notify.py`，通知统一由各 YAML 的 `notifications` 提供。

`dacapo` 直接读取 DaCapo 生成的 JSON，在内存中转换账号、设备、国内/国际签到、社区、云游戏、Web 活动和通知设置，不再创建含凭据的临时 YAML/INI。发布包的 `dacapo/template.yml` 可直接用于新版集成；输入始终按只读配置处理。

`schedule` 按 `runtime.schedule.interval_minutes` 常驻串行执行，每轮重新加载配置并应用随机延迟。该命令要求 `runtime.schedule.enabled: true`；`run_on_start` 控制启动后立即执行还是先等待一个间隔。停止时向进程发送 Ctrl+C 或由服务管理器终止。

仓库还提供每日 `00:05`（北京时间，GitHub Cron 为 `16:05 UTC`）的一次性 Actions 工作流。只有仓库变量 `ENABLE_SCHEDULED_RUN=true` 且 Secret `MIHOYO_CONFIG_YAML` 已配置时才执行，fork 默认不会运行真实任务。

首次使用时可以直接运行 `config add-account`。即使默认的 `config/config.yaml` 及其父目录尚不存在，程序也会在 Cookie 和账号信息校验成功后创建只包含该账号的新配置，并以 `mys用户:<米游社昵称>` 作为账号名称。`config setup` 提供完整的数字设置菜单，可配置运行、日志、验证码、国内凭据、HoYoLAB 独立 Cookie/语言/游戏、角色黑名单、设备、代理、任务、云游戏及通知渠道；普通运行命令不会自动进入交互界面，因此不会阻塞 CI、Docker 或计划任务。

Windows 可执行 `MihoyoBBSToolsRS create-launcher`，在 EXE 同目录生成 `MihoyoBBSToolsRS-run.bat`。BAT 固定记录生成时的 EXE 绝对路径与工作目录，因此移动到桌面或其他目录后仍能异步启动原程序；已有文件默认不会覆盖，需要覆盖时添加 `--force`，也可用 `--output` 指定 BAT 输出位置。

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

`id` 留空时会根据该账号的 Cookie 使用 UUID v3 确定性生成，因此 Cookie 变化会导致自动生成的设备 ID 改变；需要稳定设备身份时请填写固定值。国内游戏签到请求会发送设备 ID；米游社 App 类社区请求会发送设备 ID、名称、型号，并在 `fp` 非空时发送设备指纹。HoYoLAB 和米游币任务状态的 Web 请求按原项目协议不附加这些设备字段。完整规则见 [配置说明](docs/configuration.md#设备信息)。

运行时如接口明确返回国内凭据失效，程序会在每个任务流程中只尝试一次 SToken 换取新 `cookie_token`，成功后重试并原子写回 YAML；若 Cookie 来自 `${ENV_NAME}`，只更新本次运行内存，不会把 Secret 展开写入配置。游戏签到每次提交后都会查询服务端状态，仅在仍未签到时重试该角色，默认最多尝试 3 次；确认后显示实际尝试次数、累计天数和当天奖励。奖励详情单独查询失败不会把已经确认的签到改判为失败。米游社签到、阅读、点赞和分享同样会在动作后重新查询任务状态，只有确认米游币任务完成才报告成功。

## 开发与验证

本项目以 GitHub Actions 作为唯一的 Rust 编译与验证环境。开发机器只负责编辑文件和 Git 操作，不要求安装 Rust、MSVC、Windows SDK 或 Docker，也不在本地执行 Cargo 命令。

每个 Pull Request 必须通过：

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features --locked -- -D warnings`
- `cargo test --all-features --locked`
- Linux 和 Windows 的 `cargo build --release --locked`

真实接口测试必须与普通 CI 分离，只能手动触发，并应使用配置了人工批准的 GitHub Environment。普通测试只使用脱敏 Fixture 和 Mock HTTP 服务。

依赖精简、TLS 后端选择和 Linux/Windows 实测数据见 [构建体积优化记录](docs/size-optimization.md)。

## Docker

仓库中的 `Dockerfile` 用于构建最终运行镜像。镜像采用多阶段构建，运行阶段不包含 Rust 工具链。镜像应由 GitHub Actions 构建，不要求开发机执行 `docker build`。

版本标签会发布 `ghcr.io/bcmdy/mihoyo-bbs-tools-rs:<版本>` 与 `latest`，同时支持 `linux/amd64`、`linux/arm64` 和 `linux/arm/v7`。`main` 分支只更新 `main` 与提交 SHA 标签，不覆盖稳定版 `latest`。

镜像默认执行完整任务流程：

```text
MihoyoBBSToolsRS run
```

常驻定时运行可把容器命令改为 `schedule`，并在挂载的配置中启用 `runtime.schedule.enabled`。调度间隔由程序内部控制，容器不需要额外 Cron 进程。

真实配置和凭据必须在运行时通过受控配置文件、环境变量或 Secret 注入，不能写入镜像。

Kubernetes 用户可使用 [CronJob 示例](deploy/kubernetes/README.md)。清单默认按北京时间每日 `00:05` 运行，禁止任务重叠，并把名为 `mihoyo-bbs-tools-config` 的 Secret 只读挂载为配置。生产部署应固定已发布的镜像版本，不要长期跟随 `latest`。

NixOS 或启用 Flakes 的 Linux 可执行 `nix run github:bcmdy/mihoyo-bbs-tools-rs -- run`，也可在仓库中使用 `nix build`。Nix 包支持 `x86_64-linux` 与 `aarch64-linux`，安装用户文档和配置模板，并为 HTTPS 设置 Nix 的 CA 证书路径。

## 许可证

本项目采用 [MIT License](LICENSE)。从原 MihoyoBBSTools 迁移或改写的部分保留原项目版权声明。

## 非官方声明

MihoyoBBSTools RS 是社区维护的非官方开源项目，与米哈游、HoYoverse 及其关联公司无隶属、授权、认可或合作关系；相关商标归各自权利人所有。

本项目不使用官方 Logo、角色头像或其他容易使用户误认为官方产品的视觉元素。
