# 旧配置迁移说明

`migrate-config` 用于把原 Python MihoyoBBSTools v11–v15 的 YAML 转换为 MihoyoBBSTools RS version 1 配置。迁移不会修改源文件，也不会覆盖已经存在的目标文件。

## 是否必须迁移

程序可以直接读取受支持的旧配置，但只会在内存中转换。国内凭据刷新后能够完成本轮任务，却不会把新版账号结构写回旧文件。

准备长期使用 Rust 版本时，建议先迁移并改用新文件；这样可以使用完整设置菜单、独立 HoYoLAB 配置和安全的凭据写回。

## 命令用法

推荐使用位置参数：

```text
MihoyoBBSToolsRS migrate-config SOURCE
MihoyoBBSToolsRS migrate-config SOURCE TARGET
```

旧脚本使用的参数方式仍然支持：

```text
MihoyoBBSToolsRS migrate-config -i SOURCE
MihoyoBBSToolsRS migrate-config -i SOURCE -o TARGET
MihoyoBBSToolsRS migrate-config --input SOURCE --output TARGET
```

Windows 示例：

```powershell
.\MihoyoBBSToolsRS.exe migrate-config .\old.yaml
.\MihoyoBBSToolsRS.exe validate-config --config .\old.migrated.yaml
.\MihoyoBBSToolsRS.exe run --config .\old.migrated.yaml
```

位置参数和 `--input/--output` 不能混合使用。迁移成功后，程序会显示实际生成的文件路径。

## 默认输出文件

省略目标路径时，会在源文件同目录生成：

```text
config.yaml         -> config.migrated.yaml
config.yml          -> config.migrated.yml
config              -> config.migrated.yaml
configs/legacy.yaml -> configs/legacy.migrated.yaml
```

程序不会尝试自动生成 `-1`、`-2` 等备用名称。如果目标已经存在，请先选择其他名称；程序不会覆盖输入文件或已有文件。

显式目标的父目录必须已经存在。所有相对路径都以命令执行时的当前目录为基准，不以 EXE 或源文件所在目录为基准。

## 会迁移的主要内容

- 国内 Cookie、SToken、任务开关和游戏列表。
- 国内签到 User-Agent 与各游戏角色 UID 黑名单。
- 旧版顶层 `device` 的 `name`、`model`、`id` 和 `fp`。
- HoYoLAB 独立 Cookie、语言和游戏选择。
- 国内/国际云游戏总开关、单游戏开关、Token 和语言。
- 米游社社区签到板块。旧 `push.ini` 通知引用不会迁移，需要在新版 `notifications` 中重新配置。

旧版国内角色黑名单会执行。原 Python 国际服签到实际没有按 UID 使用国际服黑名单，因此迁移时会明确警告，不会将它标记为已支持。其他无法表达或已经移除的字段也会逐项显示警告。

## 迁移后的检查

1. 阅读迁移命令输出的全部警告。
2. 运行 `validate-config --config <新文件>`。
3. 检查账号、游戏、社区板块、云游戏和通知开关。
4. 首次运行可先用 `checkin --config <新文件> --region china` 缩小范围。
5. 确认新配置工作正常后，再更新定时任务使用的路径。

迁移文件会保留 Cookie、SToken、云游戏 Token、通知凭据和带认证信息的代理地址，因此属于敏感文件。不要提交到 Git、上传为 Actions Artifact、发送到聊天或放入 Docker 镜像。

完整字段含义见 [YAML 配置参考](configuration.md)，凭据保护要求见 [安全说明](security.md)。
