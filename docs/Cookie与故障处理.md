# Cookie 与凭据故障处理

本页只面向程序使用者，说明 MihoyoBBSTools RS 需要哪些 Cookie 字段、如何安全录入，以及凭据错误时怎样处理。

## 最安全的录入方式

首次使用推荐运行：

```powershell
.\MihoyoBBSToolsRS.exe config init
```

只添加账号时运行：

```powershell
.\MihoyoBBSToolsRS.exe config add-account
```

看到 Cookie 提示后再粘贴并按回车。交互式终端不会显示输入内容。不要把 Cookie 放在命令参数、BAT、截图、聊天、Issue、日志或公开仓库中。

程序不会自动读取浏览器 Cookie。请只从自己信任的浏览器会话获取，不要安装来源不明的 Cookie 导出扩展、脚本或修改版程序。

## 国内账号需要的字段

字段名称可能随米游社登录版本变化，程序会识别常见等价名称。

| 用途 | 常见字段 | 说明 |
|---|---|---|
| 识别账号 UID | `account_id`、`account_id_v2`、`ltuid`、`ltuid_v2`、`stuid` | 至少存在一个有效数字 UID。 |
| 社区任务与刷新 | `stoken` | 添加国内账号时必须存在。 |
| V2 SToken 配套身份 | `account_mid_v2`、`ltmid_v2`、`mid` | `stoken` 以 `v2_` 开头时必须存在。 |
| 游戏签到与社区请求 | `cookie_token` | 失效时程序可尝试通过 SToken 刷新。 |
| 旧式换取 SToken | `login_ticket` | 仅部分旧登录流程使用，不是所有账号都有。 |

“游戏签到当前能用”不代表 Cookie 完整。只含 UID 和 `cookie_token` 的内容可能足以访问部分游戏接口，但不能稳定执行米游社社区任务、昵称查询或凭据刷新。

HoYoLAB 使用独立 Cookie，在 `config setup` 的账号 HoYoLAB 设置中录入；不要用国内 Cookie 替代。

## 程序如何检查

`config init` 和 `config add-account` 会在保存前完成以下检查：

1. Cookie 每一段是否符合 `名称=值` 格式。
2. 是否识别到 UID。
3. 是否包含 SToken。
4. V2 SToken 是否同时包含 MID。
5. 是否能通过只读接口取得米游社昵称。

成功时只显示 UID 尾号以及 SToken、MID 是否存在，不显示原值。配置中的账号名称使用 `mys用户:<米游社昵称>`；昵称重复时会追加 UID 尾号。

## 常见错误

### 已识别 UID，但缺少 SToken

当前复制内容通常只包含游戏签到字段。重新登录米游社并获取完整 Cookie，然后通过以下菜单更新：

```text
config setup -> 账号 -> 更新 Cookie
```

### V2 SToken 缺少 MID

重新获取同一登录会话中的完整 Cookie，确保 `account_mid_v2`、`ltmid_v2` 或 `mid` 与 V2 SToken 一起存在。不要从其他账号拼接 MID。

### 无法获取米游社昵称

依次检查：

1. Cookie 是否来自当前有效登录会话。
2. 网络是否能访问米游社。
3. 是否复制了换行、引号或其他不可见字符。
4. 运行 `doctor --online`，区分网络、代理和凭据问题。

添加新账号时尚未建立账号配置，因此昵称查询不会使用该账号之后才会配置的代理。需要代理的环境应先保证系统网络可访问米游社，或先使用模板创建配置并在设置菜单中配置账号代理。

### 运行报告显示“认证失效”

Cookie 可能过期，或远程接口已撤销对应登录会话。运行 `config setup` 更新该账号 Cookie。不要只替换配置中的单个 token；完整更新可同时校验 UID、SToken、MID 和昵称。

### 自动刷新后仍失败

自动刷新只在已有 SToken 和身份字段有效时工作。SToken 本身失效、V2 SToken 缺少 MID、账号被登出或接口拒绝刷新时，仍需重新获取完整 Cookie。

## 排障命令

```powershell
.\MihoyoBBSToolsRS.exe validate-config
.\MihoyoBBSToolsRS.exe doctor
.\MihoyoBBSToolsRS.exe doctor --online
.\MihoyoBBSToolsRS.exe run --verbose
```

- `validate-config`：只校验 YAML。
- `doctor`：离线检查配置和目录，不访问网络。
- `doctor --online`：执行只读连通与身份查询，不签到、不阅读、不点赞、不领取奖励，也不发送通知。
- `run --verbose`：显示全部任务记录；默认 `run` 只突出汇总和需要处理的问题。

反馈问题前请删除或遮盖 Cookie、SToken、MID、Webhook、代理认证和通知 Token。UID 也建议只保留末四位。
