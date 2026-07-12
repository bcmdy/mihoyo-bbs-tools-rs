# 配置迁移输出

配置模块提供以下底层 API，供 `migrate-config` CLI 使用：

- `config::migrate_config(input)`：读取新配置，或将 Python v11–v15 配置迁移到统一模型。
- `config::to_yaml(config)`：序列化统一模型。
- `config::write_migrated_config(input, output)`：迁移并新建输出文件。

`write_migrated_config` 不会覆盖输入文件，也不会覆盖任何已经存在的输出文件。输出目录必须已经存在。Unix 平台创建文件时使用 `0600` 权限；其他平台使用当前用户和目录的系统 ACL。

迁移输出需要保留 Cookie、SToken、通知 Token 和带认证信息的代理 URL，因此生成的 YAML 属于敏感文件。序列化结果不得写入日志、错误消息、测试快照或构建产物。配置结构的 `Debug` 输出仍会对这些字段脱敏。

如果迁移过程中存在当前模型无法表达的旧字段，API 会在 `LoadedConfig.warnings` 中返回警告。调用方应在不包含 Secret 的前提下展示这些警告。
