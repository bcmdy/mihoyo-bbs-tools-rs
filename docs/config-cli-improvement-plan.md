# 配置与命令行改进实施方案

## 1. 文档目的

本文档用于指导后续会话实施以下改进，不包含业务代码修改：

1. `config add-account` 在配置文件或父目录不存在时自动创建配置。
2. 为运行命令增加可选择的功能参数。
3. 修复 `migrate-config` 对相对输出路径的处理。
4. 为 `migrate-config` 增加位置参数和默认输出路径。
5. 增加可通过数字输入操作的交互式设置功能。

实施时应保持现有脚本兼容、安全写入约束和敏感信息保护规则。

## 2. 当前实现概况

主要入口和职责如下：

- `src/cli.rs`：使用 Clap 定义命令和参数。
- `src/main.rs`：解析命令并调用配置、签到、社区任务等模块。
- `src/config/mod.rs`：配置模型、读取、校验、迁移和安全创建文件。
- `src/config/editor.rs`：配置编辑、添加账号和删除账号。
- `src/service/mod.rs`：导出当前已实现的任务运行器。

当前存在以下直接问题：

- `config add-account` 最终调用 `mutate_raw`，该函数无条件读取现有文件，配置不存在时直接返回 `ConfigError::Read`。
- `migrate-config` 的 `input` 和 `output` 都是必填命名选项，无法使用位置参数，也无法省略输出路径。
- 纯文件名输出路径（例如 `new_config.yaml`）的父路径是空路径；当前代码会对其执行 `canonicalize`，Windows 上返回 `os error 3`。
- `run` 固定尝试执行国内签到、HoYoLAB 签到和米游社任务，没有本次运行范围过滤参数。
- 配置管理只有编辑器、添加账号和删除账号，没有数字菜单或结构化任务设置命令。

## 3. 实施原则

### 3.1 向后兼容

- 未提供新参数时，`run`、`checkin` 的实际执行范围保持现状。
- 保留 `migrate-config --input SOURCE --output TARGET` 以及短选项 `-i/-o`。
- 不改变 `config edit`、`config add-account`、`config remove-account` 的现有调用方式。
- 新增功能不得要求现有 YAML 配置升级版本。

### 3.2 安全写入

- 不覆盖迁移输入文件。
- 不覆盖已存在的迁移输出文件。
- 新配置文件使用 `create_new(true)` 创建，防止并发首次创建时互相覆盖。
- Unix 下新建的敏感配置继续使用 `0600` 权限。
- 配置修改必须先序列化并校验，再替换原文件。
- Cookie、SToken、通知 Token、代理认证信息不得写入日志或错误消息。

### 3.3 交互与自动化分离

- 数字菜单只能通过显式命令进入，推荐命令为 `config setup`。
- `run`、`checkin` 或无参数启动时不得自动进入菜单，避免 CI、Docker、计划任务和非 TTY 环境阻塞。
- 自动化场景使用稳定的英文枚举参数；数字编号只用于交互菜单。

## 4. 修改项一：缺少配置时自动创建

### 4.1 目标行为

以下命令在 `config/config.yaml` 和 `config` 目录均不存在时应成功：

```text
MihoyoBBSToolsRS config add-account
```

成功后应创建父目录和有效配置，配置中只包含本次添加的账号，不得包含示例账号或 `${MIHOYO_COOKIE}` 等占位符。

### 4.2 设计方案

修改 `src/config/editor.rs`：

1. 将标准输入读取与账号添加逻辑拆分。
   - 保留 `add_account_from_stdin(path, name)` 作为 CLI 入口。
   - 增加接收 Cookie 字符串的内部函数，便于测试。
2. 仅添加账号操作允许初始化缺失配置。
   - 删除账号和编辑配置仍要求目标文件已存在。
   - `run`、`checkin`、`validate-config` 等读取型命令不得因为配置缺失而静默创建文件。
   - 不要让通用 `mutate_raw` 默认创建文件，避免隐藏路径错误。
3. 读取配置时区分错误类型。
   - `ErrorKind::NotFound`：进入初始化流程。
   - 权限不足、目标为目录、无效 YAML 等：保持失败，不重建或覆盖。
4. 初始化配置应在内存中完成。
   - 创建 `version: CURRENT_CONFIG_VERSION`。
   - 使用默认 `runtime`、`captcha`、`notifications`。
   - 初始 `accounts` 可以在内存中为空，但必须先加入本次账号再执行完整校验。
   - 不得先写入一个 `accounts: []` 的无效配置，因为当前校验要求至少一个账号。
5. Cookie 和账号名称校验全部成功后再创建目录。
6. 对非空父路径调用 `create_dir_all`。
7. 通过临时文件完成序列化和校验后，使用安全的新文件创建逻辑落盘。

建议抽取或复用以下内部能力：

```text
add_account(path, name, cookie)
load_raw_or_initialize(path)
write_new_validated_config(path, value)
write_replacement_validated_config(path, value)
```

具体函数名可按现有代码风格调整，但“新建”和“替换”必须有明确不同的覆盖语义。

### 4.3 边界条件

- 无效 Cookie、缺少 `stoken`、缺少 UID 且未提供备注时，不创建目录或文件。
- 已存在的空文件、损坏 YAML 或缺少 `accounts` 的配置不得自动重置。
- 父路径存在但实际为普通文件时，应返回写入错误。
- 仅传文件名时，其父路径为空，不应调用 `create_dir_all("")`。
- 两个进程同时首次创建配置时，只允许一个成功，另一个不得覆盖已创建配置。
- 修改已有配置时必须保留其他账号、未知普通字段和字段顺序策略。
- 任意失败路径不得残留 `*.editing` 临时文件。

## 5. 修改项二：运行功能选择参数

### 5.1 推荐命令

```text
MihoyoBBSToolsRS run --task china-checkin,hoyolab-checkin,bbs
MihoyoBBSToolsRS run --task bbs
MihoyoBBSToolsRS checkin --region china
MihoyoBBSToolsRS checkin --region hoyolab
MihoyoBBSToolsRS checkin --region all
```

### 5.2 参数语义

- `run --task` 使用 Clap `ValueEnum`，支持多值或逗号分隔。
- 可选值首期只包含当前已有运行器：
  - `china-checkin`
  - `hoyolab-checkin`
  - `bbs`
- 当前尚未实现运行器的 `china_cloud_game`、`overseas_cloud_game`、`web_activity` 不应暴露为可执行参数。
- 未提供 `--task` 时保持当前 `run` 行为。
- `checkin --region` 可选值为 `china`、`hoyolab`、`all`，默认 `all`，与当前实际行为兼容。
- CLI 选择只用于缩小本次执行范围，不能绕过 YAML 中账号、任务或游戏的禁用状态。
- 空选择、重复值和未知值由 Clap 给出明确错误。

### 5.3 代码组织

修改 `src/cli.rs`：

- 增加运行任务枚举和签到区域枚举。
- 为 `Run` 增加可选任务列表。
- 为 `Checkin` 增加区域选项。

修改 `src/main.rs`：

- 根据解析后的任务集合决定是否调用各运行器。
- 保持现有运行顺序：国内签到、HoYoLAB 签到、米游社任务。
- 未执行的任务不得出现在失败报告中。

如分派逻辑开始膨胀，可增加轻量的 `ExecutionSelection`/`RunSelection` 类型，但不应为了三个布尔选择引入复杂框架。

## 6. 修改项三：迁移相对路径

### 6.1 根因

`src/config/mod.rs` 的 `ensure_distinct_new_output` 当前使用：

```text
output.parent().unwrap_or_else(|| Path::new("."))
```

对 `new_config.yaml` 这类路径，`parent()` 可能返回空路径而不是 `None`，因此后续 `canonicalize` 仍然失败。

### 6.2 目标行为

以下形式均应工作：

```text
MihoyoBBSToolsRS migrate-config --input config.yaml --output new_config.yaml
MihoyoBBSToolsRS migrate-config --input .\config.yaml --output .\new_config.yaml
MihoyoBBSToolsRS migrate-config --input config\old.yaml --output output\new.yaml
```

### 6.3 修改方案

- 将 `None` 或空父路径统一解释为当前目录 `.`。
- 相对输入和显式相对输出均相对于进程当前工作目录。
- 不应相对于可执行文件所在目录解析路径。
- 显式输出也不应自动改为相对于输入文件目录。
- 继续要求显式输出的父目录已存在；本项只修复相对路径，不隐式创建任意迁移目录。
- 保留规范化后判断输入输出是否为同一文件的逻辑。

## 7. 修改项四：迁移位置参数和默认输出

### 7.1 支持的语法

```text
MihoyoBBSToolsRS migrate-config SOURCE
MihoyoBBSToolsRS migrate-config SOURCE TARGET
MihoyoBBSToolsRS migrate-config -i SOURCE
MihoyoBBSToolsRS migrate-config -i SOURCE -o TARGET
MihoyoBBSToolsRS migrate-config --input SOURCE
MihoyoBBSToolsRS migrate-config --input SOURCE --output TARGET
```

### 7.2 参数归一化

Clap 层可以分别保留位置参数字段和兼容命名选项字段，随后归一化为：

```text
ResolvedMigrationArgs {
    input: PathBuf,
    output: PathBuf,
}
```

约束：

- 输入必填，输出可选。
- 位置参数与命名参数不能重复提供同一含义。
- 推荐拒绝位置参数和命名参数混合使用，避免目标位置产生歧义。
- 参数冲突应在执行迁移前返回 CLI 错误。
- `config::write_migrated_config(input, output)` 继续接收两个明确路径，不负责 CLI 默认值。

### 7.3 默认输出命名

省略输出时，在输入文件同目录生成：

```text
config.yaml         -> config.migrated.yaml
config.yml          -> config.migrated.yml
config              -> config.migrated.yaml
configs/legacy.yaml -> configs/legacy.migrated.yaml
```

规则：

- `.yaml` 和 `.yml` 保留原扩展名，在扩展名前加入 `.migrated`。
- 无扩展名时追加 `.migrated.yaml`。
- 不自动尝试 `-1`、`-2` 等备用名称。
- 默认目标已存在时返回 `OutputAlreadyExists`。
- 默认目标与输入冲突时仍返回 `OutputMatchesInput`。
- 成功提示必须打印最终解析出的输出路径。

## 8. 修改项五：数字设置菜单

### 8.1 命令入口

新增：

```text
MihoyoBBSToolsRS config setup --config config/config.yaml
```

不建议让 `MihoyoBBSToolsRS`、`run` 或 `config` 在缺少子命令时自动进入菜单。

### 8.2 一级菜单

```text
请选择操作：
1. 添加账号
2. 设置账号任务
3. 设置账号游戏
4. 删除账号
5. 编辑完整配置
0. 退出
```

### 8.3 任务选择菜单

选择账号后显示：

```text
请选择启用的任务，可输入 123 或 1,2,3：
1. 国内游戏签到
2. HoYoLAB 签到
3. 米游社任务
0. 取消
```

选择米游社任务后继续设置：

```text
1. 社区签到
2. 阅读
3. 点赞
4. 点赞后取消
5. 分享
```

上述选项分别映射到现有 `TaskConfig` 和 `BbsTaskConfig` 字段。

### 8.4 输入规则

- 支持单选：`1`。
- 支持连续多选：`123`。
- 支持分隔多选：`1,2,3`。
- 忽略分隔符周围空白。
- 重复编号去重。
- `0` 表示取消当前操作，不写配置。
- 空输入、越界编号、非数字字符应提示错误并允许重新输入。
- 负数、超长数字和整数溢出应作为无效输入处理，不能导致 panic。
- EOF 应安全退出且不写配置。
- `config setup` 检测到标准输入不可用或不是可交互终端时，应快速返回明确错误，不能无限等待。
- 交互模块不得输出 Cookie、Token 或代理认证信息。

### 8.5 代码组织

建议新增 `src/config/interactive.rs`：

- 菜单展示和标准输入读取。
- 纯函数形式的数字选择解析器。
- 账号选择、任务设置和游戏设置流程。
- 取消及 EOF 处理。

修改 `src/config/editor.rs`：

- 提供结构化的账号任务、游戏写回函数。
- 复用现有临时文件和完整配置校验流程。
- 只修改目标账号，保留其他账号和未知普通字段。

修改 `src/config/mod.rs`：

- 导出交互入口或结构化编辑 API。

可选增强但不作为首期必需项：

```text
MihoyoBBSToolsRS config set-tasks --account NAME --enable china-checkin,bbs
```

该命令可为交互菜单提供非交互等价能力，但应在五项核心需求完成后再评估。

## 9. 文件级修改清单

### `src/cli.rs`

- 增加迁移位置参数及旧选项兼容模型。
- 增加迁移参数归一化和默认输出路径计算所需结构。
- 增加 `RunTask`、`CheckinRegion` 等枚举。
- 增加 `config setup` 子命令。
- 增加 CLI 解析测试。

### `src/main.rs`

- 使用归一化后的迁移参数。
- 按运行选择调用任务运行器。
- 分派 `config setup`。
- 输出实际生成的迁移目标路径。

### `src/config/mod.rs`

- 修复空父路径的相对输出处理。
- 视实现需要提供默认配置构造和安全写入辅助能力。
- 保持迁移写入 API 的不覆盖规则。
- 增加相对路径和默认输出相关测试。

### `src/config/editor.rs`

- 拆分标准输入和账号添加逻辑。
- 增加缺失配置初始化。
- 区分安全新建与校验后替换。
- 增加账号任务、游戏设置 API。
- 补充临时文件清理和失败原子性测试。

### `src/config/interactive.rs`（新增）

- 实现数字菜单和输入解析。
- 不承担 YAML 序列化、安全写入或业务校验职责。

### 文档

- 更新 `README.md` 的命令列表和迁移示例。
- 更新 `docs/configuration.md` 的账号初始化、运行筛选和交互设置说明。
- 更新 `docs/config-migration.md` 的位置参数、默认输出和相对路径规则。

## 10. 测试计划

### 10.1 配置首次创建

- 配置文件不存在、父目录存在时成功创建。
- 多级父目录不存在时自动创建。
- 默认路径 `config/config.yaml` 端到端成功。
- 新配置只有本次账号，没有示例账号或环境变量占位符。
- 无效 Cookie 或缺少 `stoken` 时不产生文件和目录。
- 已有坏 YAML、目标为目录、父路径为文件时失败且不覆盖。
- 并发首次创建时拒绝覆盖。
- Unix 下文件权限为 `0600`。
- 失败后不残留临时文件。

### 10.2 CLI 运行选择

- `run` 无 `--task` 时保持现有行为。
- 单选、多选、逗号分隔和重复值解析正确。
- 未知枚举值由 Clap 拒绝。
- CLI 选择与 YAML 开关正确取交集。
- `checkin --region china|hoyolab|all` 分派正确。

### 10.3 配置迁移

- 单位置参数、双位置参数解析成功。
- 旧长选项和短选项保持可用。
- 仅 `--input` 时生成默认输出。
- 重复或混合参数按约定失败。
- `config.yaml` 作为输入、`new_config.yaml` 作为输出时成功。
- `.yaml`、`.yml`、无扩展名和嵌套相对路径的默认命名正确。
- 默认目标与输入位于相同目录。
- 已有输出和输入输出相同时拒绝覆盖。
- 显式输出父目录不存在时保持失败。

### 10.4 数字菜单

- `1`、`123`、`1,2,3` 解析正确。
- 空格、重复项、空输入、越界、非法字符处理正确。
- `0`、EOF 和用户取消不写文件。
- 只修改目标账号，其他账号和未知字段保持不变。
- 设置后必须重新通过 `config::load` 和完整校验。
- 非 TTY 环境不会因普通运行命令自动进入菜单。
- 直接执行 `config setup` 且无可用交互输入时会明确失败，不挂起进程。

## 11. 验收标准

完成实施后，以下命令应满足预期：

```text
# 配置和目录不存在时创建首个账号配置
MihoyoBBSToolsRS config add-account

# 相对路径迁移
MihoyoBBSToolsRS migrate-config --input config.yaml --output new_config.yaml

# 位置参数迁移
MihoyoBBSToolsRS migrate-config config.yaml new_config.yaml

# 自动生成 config.migrated.yaml
MihoyoBBSToolsRS migrate-config config.yaml

# 临时只运行指定功能
MihoyoBBSToolsRS run --task china-checkin,bbs

# 进入数字设置菜单
MihoyoBBSToolsRS config setup
```

同时满足：

- 旧命令和旧配置继续可用。
- 不覆盖已有配置或迁移输出。
- 所有新错误信息不包含 Secret。
- 新增参数出现在 `--help` 中，参数错误继续使用 Clap 的标准非零退出码；配置读写和校验错误保持现有退出码语义。
- Windows 和 Linux 至少各验证一次相对路径、首次创建和迁移写入行为。
- `cargo fmt --check`、`cargo clippy --all-targets --all-features -- -D warnings` 和 `cargo test --all-targets` 通过。
- README 和配置、迁移文档与实际命令保持一致。

## 12. 建议实施顺序

1. 修复迁移纯文件名相对路径，并补底层测试。
2. 实现迁移参数归一化、位置参数和默认输出。
3. 实现配置首次创建和安全写入测试。
4. 增加运行任务筛选参数。
5. 增加结构化配置编辑 API。
6. 实现 `config setup` 数字菜单。
7. 补充 CLI 集成测试和文档。
8. 执行格式化、静态检查和全量测试。

该顺序优先处理可独立验证的路径问题，再处理配置写入，最后接入交互层，可降低并行实施时的冲突范围。
