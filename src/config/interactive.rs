use super::{
    ConfigError, LogLevel, NotificationProvider, add_account_from_stdin, edit_file, load,
    remove_account, remove_notification_provider, replace_account_cookie, set_account_device,
    set_account_games, set_account_general, set_account_proxy, set_account_tasks,
    set_captcha_endpoint, set_logging, set_notification_options, set_notification_provider,
    set_runtime,
};
use std::{
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
};

pub fn setup(path: &Path) -> Result<(), ConfigError> {
    if !io::stdin().is_terminal() {
        return Err(ConfigError::Edit("config setup 需要交互式终端".into()));
    }
    loop {
        println!("配置节点：1.全局运行 2.验证码 3.账号 4.通知 5.校验 6.高级 YAML 编辑 0.退出");
        match read_number(6)? {
            None | Some(0) => return Ok(()),
            Some(1) => runtime(path)?,
            Some(2) => captcha(path)?,
            Some(3) => accounts(path)?,
            Some(4) => notifications(path)?,
            Some(5) => println!("配置有效：{} 个账号", load(path)?.config.accounts.len()),
            Some(6) => edit_file(path)?,
            _ => {}
        }
    }
}

fn runtime(path: &Path) -> Result<(), ConfigError> {
    println!("全局运行：1.请求与重试 2.文件日志 0.返回");
    match read_number(2)? {
        Some(1) => runtime_request(path),
        Some(2) => runtime_logging(path),
        _ => Ok(()),
    }
}

fn runtime_request(path: &Path) -> Result<(), ConfigError> {
    let c = load(path)?.config.runtime;
    let timezone = prompt_keep("时区", &c.timezone)?;
    let timeout = prompt_number_keep("请求超时秒数", c.request_timeout_seconds)?;
    let retry = prompt_u32_keep("重试次数", c.retry_count)?;
    let delay = prompt_number_keep("随机延迟秒数", c.random_delay_seconds)?;
    println!("日志级别：1.trace 2.debug 3.info 4.warn 5.error，留空保留当前值");
    let current_level = log_level_name(c.log_level);
    let level = match read_number(5)? {
        Some(1) => "trace",
        Some(2) => "debug",
        Some(3) => "info",
        Some(4) => "warn",
        Some(5) => "error",
        _ => current_level,
    };
    set_runtime(path, &timezone, timeout, retry, delay, level)
}

fn runtime_logging(path: &Path) -> Result<(), ConfigError> {
    let c = load(path)?.config.runtime.logging;
    let enabled = prompt_bool("启用文件日志", c.enabled)?;
    let directory = prompt_keep("日志目录", &c.directory.to_string_lossy())?;
    let prefix = prompt_keep("日志文件名前缀", &c.file_prefix)?;
    set_logging(path, enabled, &directory, &prefix)
}

fn captcha(path: &Path) -> Result<(), ConfigError> {
    let current = load(path)?
        .config
        .captcha
        .endpoint
        .map(|url| url.to_string());
    let shown = current.as_ref().map(|_| "<已配置>");
    let endpoint = prompt_optional("验证码端点", shown)?;
    if endpoint.as_deref() == Some("<已配置>") {
        return Ok(());
    }
    set_captcha_endpoint(path, endpoint.as_deref())
}

fn accounts(path: &Path) -> Result<(), ConfigError> {
    loop {
        println!("账号：1.添加 2.基本信息 3.更新 Cookie 4.设备 5.代理 6.任务 7.游戏 8.删除 0.返回");
        match read_number(8)? {
            None | Some(0) => return Ok(()),
            Some(1) => {
                let remark = prompt("可选备注(留空不设置)")?;
                let name =
                    add_account_from_stdin(path, (!remark.is_empty()).then_some(remark.as_str()))?;
                println!("已添加账号：{name}");
            }
            Some(2) => account_general(path)?,
            Some(3) => account_cookie(path)?,
            Some(4) => account_device(path)?,
            Some(5) => account_proxy(path)?,
            Some(6) => tasks(path)?,
            Some(7) => games(path)?,
            Some(8) => {
                if let Some(name) = choose(path)? {
                    remove_account(path, &name)?;
                }
            }
            _ => {}
        }
    }
}

fn account_general(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let loaded = load(path)?.config;
    let account = loaded
        .accounts
        .iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在");
    let enabled = prompt_bool("启用账号", account.enabled)?;
    let remark = prompt_optional("备注", account.remark.as_deref())?;
    set_account_general(path, &name, enabled, remark.as_deref())
}

fn account_cookie(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    eprintln!("请输入新的完整 Cookie（输入内容不会写入日志，留空取消）：");
    let cookie = prompt("")?;
    if cookie.is_empty() {
        return Ok(());
    }
    let new_name = replace_account_cookie(path, &name, &cookie)?;
    println!("Cookie、SToken 与米游社昵称已更新：{new_name}");
    Ok(())
}

fn account_device(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let loaded = load(path)?.config;
    let device = &loaded
        .accounts
        .iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在")
        .device;
    let device_name = prompt_keep("设备名称", &device.name)?;
    let model = prompt_keep("设备型号", &device.model)?;
    let id = prompt_clearable("设备 ID", &device.id)?;
    let fp = prompt_clearable("设备 FP", &device.fp)?;
    set_account_device(path, &name, &device_name, &model, &id, &fp)
}

fn account_proxy(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let loaded = load(path)?.config;
    let account = loaded
        .accounts
        .iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在");
    let current = account.proxy.url.as_ref().map(|_| "<已配置>");
    println!("代理支持 http、https、socks5、socks5h；敏感代理不会回显");
    let proxy = prompt_optional("代理 URL", current)?;
    if proxy.as_deref() == Some("<已配置>") {
        return Ok(());
    }
    set_account_proxy(path, &name, proxy.as_deref())
}

fn notifications(path: &Path) -> Result<(), ConfigError> {
    loop {
        println!("通知：1.通用选项 2.添加渠道 3.编辑渠道 4.删除渠道 0.返回");
        match read_number(4)? {
            None | Some(0) => return Ok(()),
            Some(1) => notification_options(path)?,
            Some(2) => add_notification_provider(path)?,
            Some(3) => edit_notification_provider(path)?,
            Some(4) => delete_notification_provider(path)?,
            _ => {}
        }
    }
}

fn notification_options(path: &Path) -> Result<(), ConfigError> {
    let current = load(path)?.config.notifications;
    let mut enabled = prompt_bool("启用通知", current.enabled)?;
    if enabled && current.providers.is_empty() {
        println!("请先添加至少一个通知渠道；本次保持通知关闭");
        enabled = false;
    }
    let error_only = prompt_bool("仅错误时推送", current.error_only)?;
    let words = prompt(&format!(
        "屏蔽关键词(逗号分隔，留空保留，- 清空)[{}]",
        current.block_keywords.join(",")
    ))?;
    let keywords = if words.is_empty() {
        current.block_keywords
    } else if words == "-" {
        Vec::new()
    } else {
        words
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    };
    set_notification_options(path, enabled, error_only, keywords)
}

fn add_notification_provider(path: &Path) -> Result<(), ConfigError> {
    let Some(kind) = choose_provider_type()? else {
        return Ok(());
    };
    let fields = prompt_provider_fields(kind, false)?;
    set_notification_provider(path, None, kind, &fields)
}

fn edit_notification_provider(path: &Path) -> Result<(), ConfigError> {
    let providers = load(path)?.config.notifications.providers;
    let Some(index) = choose_provider(&providers)? else {
        return Ok(());
    };
    let kind = provider_type(&providers[index]);
    println!("编辑 {kind}：留空保留原值，输入 - 清空可选字段；敏感值不会回显");
    let fields = prompt_provider_fields(kind, true)?;
    set_notification_provider(path, Some(index), kind, &fields)
}

fn delete_notification_provider(path: &Path) -> Result<(), ConfigError> {
    let providers = load(path)?.config.notifications.providers;
    if let Some(index) = choose_provider(&providers)? {
        remove_notification_provider(path, index)?;
    }
    Ok(())
}

fn choose_provider(providers: &[NotificationProvider]) -> Result<Option<usize>, ConfigError> {
    if providers.is_empty() {
        println!("尚未配置通知渠道");
        return Ok(None);
    }
    for (index, provider) in providers.iter().enumerate() {
        println!("{}. {}", index + 1, provider_type(provider));
    }
    Ok(read_number(providers.len())?.and_then(
        |number| {
            if number == 0 { None } else { Some(number - 1) }
        },
    ))
}

fn choose_provider_type() -> Result<Option<&'static str>, ConfigError> {
    let types = provider_types();
    for (index, kind) in types.iter().enumerate() {
        println!("{}. {kind}", index + 1);
    }
    Ok(read_number(types.len())?.and_then(|number| {
        if number == 0 {
            None
        } else {
            Some(types[number - 1])
        }
    }))
}

#[derive(Clone, Copy)]
struct ProviderField {
    name: &'static str,
    required: bool,
    default: Option<&'static str>,
}

fn prompt_provider_fields(
    kind: &str,
    editing: bool,
) -> Result<Vec<(String, Option<String>)>, ConfigError> {
    let mut values = Vec::new();
    for field in provider_fields(kind) {
        loop {
            let default = field
                .default
                .map(|value| format!("，默认 {value}"))
                .unwrap_or_default();
            let value = prompt(&format!("{}{}", field.name, default))?;
            if editing && value.is_empty() {
                values.push((field.name.to_owned(), None));
                break;
            }
            if value == "-" && field.required {
                println!("{} 是必填字段，不能清空", field.name);
                continue;
            }
            let value = if value.is_empty() {
                field.default.unwrap_or("").to_owned()
            } else if value == "-" {
                String::new()
            } else {
                value
            };
            if !editing && field.required && value.is_empty() {
                println!("{} 不能为空", field.name);
                continue;
            }
            values.push((field.name.to_owned(), Some(value)));
            break;
        }
    }
    Ok(values)
}

fn provider_types() -> &'static [&'static str] {
    &[
        "telegram",
        "webhook",
        "pushplus",
        "ftqq",
        "pushme",
        "cqhttp",
        "wecom",
        "wecomrobot",
        "pushdeer",
        "dingrobot",
        "feishubot",
        "bark",
        "gotify",
        "ifttt",
        "qmsg",
        "discord",
        "wxpusher",
        "serverchan3",
    ]
}

fn provider_fields(kind: &str) -> &'static [ProviderField] {
    const TELEGRAM: &[ProviderField] = &[
        field("bot_token", true, None),
        field("chat_id", true, None),
        field("api_url", true, Some("https://api.telegram.org")),
        field("proxy", false, None),
    ];
    const WEBHOOK: &[ProviderField] = &[field("url", true, None)];
    const PUSHPLUS: &[ProviderField] = &[field("token", true, None), field("topic", false, None)];
    const FTQQ: &[ProviderField] = &[field("sendkey", true, None), field("api_url", false, None)];
    const TOKEN_URL: &[ProviderField] =
        &[field("token", true, None), field("api_url", false, None)];
    const CQHTTP: &[ProviderField] = &[
        field("url", true, None),
        field("qq", false, None),
        field("group", false, None),
    ];
    const WECOM: &[ProviderField] = &[
        field("corp_id", true, None),
        field("agent_id", true, None),
        field("secret", true, None),
        field("to_user", true, Some("@all")),
        field("api_url", false, None),
    ];
    const WECOM_ROBOT: &[ProviderField] = &[field("url", true, None), field("mobile", false, None)];
    const DING: &[ProviderField] = &[field("webhook", true, None), field("secret", false, None)];
    const WEBHOOK_ONLY: &[ProviderField] = &[field("webhook", true, None)];
    const BARK: &[ProviderField] = &[
        field("token", true, None),
        field("api_url", false, None),
        field("icon", false, None),
    ];
    const GOTIFY: &[ProviderField] = &[
        field("token", true, None),
        field("api_url", true, None),
        field("priority", true, Some("0")),
    ];
    const IFTTT: &[ProviderField] = &[
        field("event", true, None),
        field("key", true, None),
        field("api_url", false, None),
    ];
    const QMSG: &[ProviderField] = &[field("key", true, None), field("api_url", false, None)];
    const WXPUSHER: &[ProviderField] = &[
        field("app_token", true, None),
        field("uids", false, None),
        field("topic_ids", false, None),
        field("api_url", false, None),
    ];
    const SERVERCHAN3: &[ProviderField] =
        &[field("sendkey", true, None), field("tags", false, None)];
    match kind {
        "telegram" => TELEGRAM,
        "webhook" => WEBHOOK,
        "pushplus" => PUSHPLUS,
        "ftqq" => FTQQ,
        "pushme" | "pushdeer" => TOKEN_URL,
        "cqhttp" => CQHTTP,
        "wecom" => WECOM,
        "wecomrobot" => WECOM_ROBOT,
        "dingrobot" => DING,
        "feishubot" | "discord" => WEBHOOK_ONLY,
        "bark" => BARK,
        "gotify" => GOTIFY,
        "ifttt" => IFTTT,
        "qmsg" => QMSG,
        "wxpusher" => WXPUSHER,
        "serverchan3" => SERVERCHAN3,
        _ => &[],
    }
}

const fn field(name: &'static str, required: bool, default: Option<&'static str>) -> ProviderField {
    ProviderField {
        name,
        required,
        default,
    }
}

fn provider_type(provider: &NotificationProvider) -> &'static str {
    match provider {
        NotificationProvider::Telegram { .. } => "telegram",
        NotificationProvider::Webhook { .. } => "webhook",
        NotificationProvider::Pushplus { .. } => "pushplus",
        NotificationProvider::Ftqq { .. } => "ftqq",
        NotificationProvider::Pushme { .. } => "pushme",
        NotificationProvider::Cqhttp { .. } => "cqhttp",
        NotificationProvider::Wecom { .. } => "wecom",
        NotificationProvider::Wecomrobot { .. } => "wecomrobot",
        NotificationProvider::Pushdeer { .. } => "pushdeer",
        NotificationProvider::Dingrobot { .. } => "dingrobot",
        NotificationProvider::Feishubot { .. } => "feishubot",
        NotificationProvider::Bark { .. } => "bark",
        NotificationProvider::Gotify { .. } => "gotify",
        NotificationProvider::Ifttt { .. } => "ifttt",
        NotificationProvider::Qmsg { .. } => "qmsg",
        NotificationProvider::Discord { .. } => "discord",
        NotificationProvider::Wxpusher { .. } => "wxpusher",
        NotificationProvider::Serverchan3 { .. } => "serverchan3",
    }
}

fn tasks(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    println!("任务：1.国内签到 2.HoYoLAB 3.米游社 4.国内云游戏 5.海外云游戏 6.Web活动；留空取消");
    let Some(selected) = read_choice(6)? else {
        return Ok(());
    };
    if selected == [0] {
        return Ok(());
    }
    let bbs = if selected.contains(&3) {
        println!("米游社：1.签到 2.阅读 3.点赞 4.取消点赞 5.分享；留空取消");
        let Some(value) = read_choice(5)? else {
            return Ok(());
        };
        if value == [0] {
            return Ok(());
        }
        value
    } else {
        Vec::new()
    };
    set_account_tasks(path, &name, &selected, &bbs)
}

fn games(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    println!("游戏：1.原神 2.崩坏学园2 3.崩坏3 4.未定事件簿 5.星穹铁道 6.绝区零；留空取消");
    let Some(selected) = read_choice(6)? else {
        return Ok(());
    };
    if selected == [0] {
        return Ok(());
    }
    set_account_games(path, &name, &selected)
}

fn choose(path: &Path) -> Result<Option<String>, ConfigError> {
    let config = load(path)?.config;
    for (index, account) in config.accounts.iter().enumerate() {
        let remark = account
            .remark
            .as_deref()
            .map(|value| format!("（{value}）"))
            .unwrap_or_default();
        println!("{}. {}{}", index + 1, account.name, remark);
    }
    Ok(read_number(config.accounts.len())?.and_then(|number| {
        if number == 0 {
            None
        } else {
            Some(config.accounts[number - 1].name.clone())
        }
    }))
}

fn prompt_bool(label: &str, current: bool) -> Result<bool, ConfigError> {
    loop {
        let value = prompt(&format!("{label}[{}] (true/false，留空保留)", current))?;
        match value.to_ascii_lowercase().as_str() {
            "" => return Ok(current),
            "1" | "true" | "yes" | "y" => return Ok(true),
            "0" | "false" | "no" | "n" => return Ok(false),
            _ => println!("请输入 true/false、yes/no 或 1/0"),
        }
    }
}

fn prompt_keep(label: &str, current: &str) -> Result<String, ConfigError> {
    let value = prompt(&format!("{label}[{current}] (留空保留)"))?;
    Ok(if value.is_empty() {
        current.to_owned()
    } else {
        value
    })
}

fn prompt_clearable(label: &str, current: &str) -> Result<String, ConfigError> {
    let value = prompt(&format!("{label}[{current}] (留空保留，- 清空)"))?;
    Ok(if value.is_empty() {
        current.to_owned()
    } else if value == "-" {
        String::new()
    } else {
        value
    })
}

fn prompt_optional(label: &str, current: Option<&str>) -> Result<Option<String>, ConfigError> {
    let value = prompt(&format!(
        "{label}[{}] (留空保留，- 清空)",
        current.unwrap_or("未设置")
    ))?;
    if value.is_empty() {
        Ok(current.map(str::to_owned))
    } else if value == "-" {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn prompt_number_keep(label: &str, current: u64) -> Result<u64, ConfigError> {
    loop {
        let value = prompt(&format!("{label}[{current}] (留空保留)"))?;
        if value.is_empty() {
            return Ok(current);
        }
        match value.parse() {
            Ok(value) => return Ok(value),
            Err(_) => println!("请输入非负整数"),
        }
    }
}

fn prompt_u32_keep(label: &str, current: u32) -> Result<u32, ConfigError> {
    loop {
        let value = prompt(&format!("{label}[{current}] (留空保留)"))?;
        if value.is_empty() {
            return Ok(current);
        }
        match value.parse() {
            Ok(value) => return Ok(value),
            Err(_) => println!("请输入不超过 {} 的非负整数", u32::MAX),
        }
    }
}

fn log_level_name(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Trace => "trace",
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    }
}

fn prompt(label: &str) -> Result<String, ConfigError> {
    print!("{label}> ");
    io::stdout()
        .flush()
        .map_err(|_| ConfigError::Edit("输出失败".into()))?;
    let mut value = String::new();
    io::stdin()
        .lock()
        .read_line(&mut value)
        .map_err(|_| ConfigError::Edit("读取失败".into()))?;
    Ok(value.trim().into())
}

fn read_number(max: usize) -> Result<Option<usize>, ConfigError> {
    loop {
        let value = prompt("")?;
        if value.is_empty() {
            return Ok(None);
        }
        match value.parse::<usize>() {
            Ok(number) if number <= max => return Ok(Some(number)),
            _ => println!("请输入 0 到 {max} 之间的编号"),
        }
    }
}

fn read_choice(max: u8) -> Result<Option<Vec<u8>>, ConfigError> {
    loop {
        let value = prompt("")?;
        if value.is_empty() {
            return Ok(None);
        }
        match parse_choices(&value, max) {
            Ok(value) => return Ok(Some(value)),
            Err(error) => println!("{error}"),
        }
    }
}

pub fn parse_choices(value: &str, max: u8) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::new();
    for character in value
        .chars()
        .filter(|character| !character.is_whitespace() && *character != ',')
    {
        if !character.is_ascii_digit() {
            return Err("请输入数字");
        }
        let number = character.to_digit(10).unwrap() as u8;
        if number > max {
            return Err("编号超出范围");
        }
        if !output.contains(&number) {
            output.push(number);
        }
    }
    if output.is_empty() {
        return Err("输入不能为空");
    }
    if output.contains(&0) && output.len() > 1 {
        return Err("0不能与其他编号同时选择");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices() {
        assert_eq!(parse_choices("1,2,2,3", 3).unwrap(), vec![1, 2, 3]);
        assert!(parse_choices("-1", 3).is_err());
    }

    #[test]
    fn every_notification_provider_has_fields() {
        for kind in provider_types() {
            assert!(!provider_fields(kind).is_empty(), "{kind} 缺少菜单字段");
        }
        assert!(
            provider_fields("telegram")
                .iter()
                .any(|field| field.name == "proxy")
        );
    }
}
