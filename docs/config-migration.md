# 配置迁移输出

配置模块提供以下底层 API，供 `migrate-config` CLI 使用：

- `config::migrate_config(input)`：读取新配置，或将 Python v11–v15 配置迁移到统一模型。
- `config::to_yaml(config)`：序列化统一模型。
- `config::write_migrated_config(input, output)`：迁移并新建输出文件。

`write_migrated_config` 不会覆盖输入文件，也不会覆盖任何已经存在的输出文件。输出目录必须已经存在。Unix 平台创建文件时使用 `0600` 权限；其他平台使用当前用户和目录的系统 ACL。

迁移输出需要保留 Cookie、SToken、通知 Token 和带认证信息的代理 URL，因此生成的 YAML 属于敏感文件。序列化结果不得写入日志、错误消息、测试快照或构建产物。配置结构的 `Debug` 输出仍会对这些字段脱敏。

如果迁移过程中存在当前模型无法表达的旧字段，API 会在 `LoadedConfig.warnings` 中返回警告。调用方应在不包含 Secret 的前提下展示这些警告。

## 命令行用法

`migrate-config` 支持位置参数，也保留原有长、短命名选项：

```text
mihoyo-bbs-tools migrate-config SOURCE
mihoyo-bbs-tools migrate-config SOURCE TARGET
mihoyo-bbs-tools migrate-config -i SOURCE
mihoyo-bbs-tools migrate-config -i SOURCE -o TARGET
mihoyo-bbs-tools migrate-config --input SOURCE
mihoyo-bbs-tools migrate-config --input SOURCE --output TARGET
```

输入路径必填，输出路径可省略。位置参数和命名选项不能混合或重复表达同一输入、输出；参数冲突会在读取配置前由命令行解析阶段拒绝。旧脚本使用的 `--input/--output` 和 `-i/-o` 调用方式保持可用。迁移成功后，命令会打印最终使用的输出路径。

### 默认输出路径

省略输出路径时，在输入文件同目录生成目标文件：

```text
config.yaml         -> config.migrated.yaml
config.yml          -> config.migrated.yml
config              -> config.migrated.yaml
configs/legacy.yaml -> configs/legacy.migrated.yaml
```

`.yaml` 和 `.yml` 会保留原扩展名，并在扩展名前加入 `.migrated`；无扩展名时追加 `.migrated.yaml`。程序不会尝试 `-1`、`-2` 等备用名称，默认目标已存在时会直接失败。

### 相对路径规则

相对输入和显式相对输出都以进程当前工作目录为基准，不以可执行文件或输入文件所在目录为基准。因此以下形式均有效：

```text
mihoyo-bbs-tools migrate-config --input config.yaml --output new_config.yaml
mihoyo-bbs-tools migrate-config --input .\config.yaml --output .\new_config.yaml
mihoyo-bbs-tools migrate-config --input config\old.yaml --output output\new.yaml
```

纯文件名输出的空父路径按当前目录 `.` 处理。显式输出的父目录仍必须已经存在，迁移命令不会隐式创建任意输出目录。规范化后如果输入和输出指向同一文件，或输出文件已经存在，命令都会拒绝写入；无论使用默认还是显式目标，都不会覆盖已有文件。

## 设备信息迁移

Python v11–v15 配置中的顶层 `device` 会迁移到新版账号级 `accounts[].device`：

```yaml
accounts:
  - name: migrated-account
    device:
      name: "Legacy Device"
      model: "Legacy Model"
      id: "legacy-device-id"
      fp: "legacy-device-fp"
```

迁移会保留旧配置中的 `name`、`model`、`id` 和 `fp`。缺失的 `name` 和 `model` 分别使用 `Xiaomi MI 6` 和 `Mi 6`；缺失或留空的 `id` 会在运行时根据账号 Cookie 使用 UUID v3 确定性地自动生成。`fp` 当前仅在配置模型与迁移输出中保留，尚未用于请求头或接口参数。

由于旧格式的 `device` 位于账号数据所在的配置根级，迁移后它只属于该配置转换出的账号。多账号配置应为每个账号分别保留设备信息，避免多个账号共用同一设备 ID。
