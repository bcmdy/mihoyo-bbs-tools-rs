# 配置迁移输出

配置模块提供以下底层 API，供 `migrate-config` CLI 使用：

- `config::migrate_config(input)`：读取新配置，或将 Python v11–v15 配置迁移到统一模型。
- `config::to_yaml(config)`：序列化统一模型。
- `config::write_migrated_config(input, output)`：迁移并新建输出文件。

`write_migrated_config` 不会覆盖输入文件，也不会覆盖任何已经存在的输出文件。输出目录必须已经存在。Unix 平台创建文件时使用 `0600` 权限；其他平台使用当前用户和目录的系统 ACL。

迁移输出需要保留 Cookie、SToken、通知 Token 和带认证信息的代理 URL，因此生成的 YAML 属于敏感文件。序列化结果不得写入日志、错误消息、测试快照或构建产物。配置结构的 `Debug` 输出仍会对这些字段脱敏。

如果迁移过程中存在当前模型无法表达的旧字段，API 会在 `LoadedConfig.warnings` 中返回警告。调用方应在不包含 Secret 的前提下展示这些警告。

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
