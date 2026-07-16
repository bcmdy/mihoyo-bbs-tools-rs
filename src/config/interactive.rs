use super::{
    ConfigError, EditSession, HoyolabConfig, LogLevel, NotificationProvider, RoleBlacklistConfig,
    add_account_from_stdin, edit_file, load, remove_account, remove_notification_provider,
    replace_account_cookie, set_account_china_checkin, set_account_cloud_games, set_account_device,
    set_account_games, set_account_general, set_account_hoyolab, set_account_proxy,
    set_account_tasks, set_captcha_endpoint, set_logging, set_notification_options,
    set_notification_provider, set_runtime, set_schedule,
};
use std::{
    io::{self, IsTerminal},
    path::Path,
};

pub async fn setup(path: &Path) -> Result<(), ConfigError> {
    if !io::stdin().is_terminal() {
        return Err(ConfigError::Edit("config setup 需要交互式终端".into()));
    }
    let session = EditSession::begin(path)?;
    let working = session.path().to_path_buf();
    loop {
        println!(
            "配置节点：1.全局运行 2.验证码 3.账号 4.通知 5.校验暂存配置 6.高级 YAML 编辑 0.保存并退出"
        );
        match read_number(6)? {
            None | Some(0) => break,
            Some(1) => runtime(&working)?,
            Some(2) => captcha(&working)?,
            Some(3) => accounts(&working).await?,
            Some(4) => notifications(&working)?,
            Some(5) => println!(
                "暂存配置有效：{} 个账号",
                load(&working)?.config.accounts.len()
            ),
            Some(6) => edit_file(&working)?,
            _ => {}
        }
    }
    if !session.has_changes()? {
        println!("配置没有变化，未写入文件");
        return Ok(());
    }
    println!("\n待保存变更：");
    for change in session.summary()? {
        println!("- {change}");
    }
    if super::input::confirm("保存这些变更？", true)? {
        session.commit()?;
        println!("配置已保存：{}", path.display());
    } else {
        println!("已放弃全部暂存变更，原配置未修改");
    }
    Ok(())
}

fn runtime(path: &Path) -> Result<(), ConfigError> {
    println!("全局运行：1.请求与重试 2.文件日志 3.定时运行 0.返回");
    match read_number(3)? {
        Some(1) => runtime_request(path),
        Some(2) => runtime_logging(path),
        Some(3) => runtime_schedule(path),
        _ => Ok(()),
    }
}

fn runtime_request(path: &Path) -> Result<(), ConfigError> {
    let c = load(path)?.config.runtime;
    let timezone = prompt_keep("时区", &c.timezone)?;
    let timeout = prompt_number_keep("请求超时秒数", c.request_timeout_seconds)?;
    let retry = prompt_u32_keep("重试次数", c.retry_count)?;
    let game_checkin_max_attempts =
        prompt_u32_keep("游戏签到最大尝试次数", c.game_checkin_max_attempts)?;
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
    set_runtime(
        path,
        &timezone,
        timeout,
        retry,
        game_checkin_max_attempts,
        delay,
        level,
    )
}

fn runtime_logging(path: &Path) -> Result<(), ConfigError> {
    let c = load(path)?.config.runtime.logging;
    let enabled = prompt_bool("启用文件日志", c.enabled)?;
    let directory = prompt_keep("日志目录", &c.directory.to_string_lossy())?;
    let prefix = prompt_keep("日志文件名前缀", &c.file_prefix)?;
    set_logging(path, enabled, &directory, &prefix)
}

fn runtime_schedule(path: &Path) -> Result<(), ConfigError> {
    let schedule = load(path)?.config.runtime.schedule;
    let enabled = prompt_bool("启用 schedule 常驻定时运行", schedule.enabled)?;
    let interval = prompt_number_keep("执行间隔分钟数", schedule.interval_minutes)?;
    let run_on_start = prompt_bool("启动后立即执行第一轮", schedule.run_on_start)?;
    set_schedule(path, enabled, interval, run_on_start)
}

fn captcha(path: &Path) -> Result<(), ConfigError> {
    let current = load(path)?
        .config
        .captcha
        .endpoint
        .map(|url| url.to_string());
    let shown = current.as_ref().map(|_| "<已配置>");
    let endpoint = prompt_optional_hidden("验证码端点", shown)?;
    if endpoint.as_deref() == Some("<已配置>") {
        return Ok(());
    }
    set_captcha_endpoint(path, endpoint.as_deref())
}

async fn accounts(path: &Path) -> Result<(), ConfigError> {
    loop {
        println!(
            "账号：1.添加 2.基本信息 3.更新 Cookie 4.设备 5.代理 6.国内签到 7.HoYoLAB 8.云游戏 9.任务 10.国内游戏 11.删除 0.返回"
        );
        match read_number(11)? {
            None | Some(0) => return Ok(()),
            Some(1) => {
                let remark = prompt("可选备注(留空不设置)")?;
                let name =
                    add_account_from_stdin(path, (!remark.is_empty()).then_some(remark.as_str()))
                        .await?;
                println!("已添加账号：{name}");
            }
            Some(2) => account_general(path)?,
            Some(3) => account_cookie(path).await?,
            Some(4) => account_device(path)?,
            Some(5) => account_proxy(path)?,
            Some(6) => account_china_checkin(path)?,
            Some(7) => account_hoyolab(path)?,
            Some(8) => account_cloud_games(path)?,
            Some(9) => tasks(path)?,
            Some(10) => games(path)?,
            Some(11) => {
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

async fn account_cookie(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let cookie = super::input::prompt_secret(
        "请输入新的完整 Cookie（输入内容不会显示，留空取消）",
    )?;
    if cookie.is_empty() {
        return Ok(());
    }
    let new_name = replace_account_cookie(path, &name, &cookie).await?;
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
    let proxy = prompt_optional_hidden("代理 URL", current)?;
    if proxy.as_deref() == Some("<已配置>") {
        return Ok(());
    }
    set_account_proxy(path, &name, proxy.as_deref())
}

fn account_cloud_games(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let cloud = load(path)?
        .config
        .accounts
        .into_iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在")
        .cloud_games;
    println!("已保存的云游戏 Token 不会回显；新输入同样隐藏，留空保留，输入 - 清空");
    let china_genshin_token =
        prompt_secret("国内云原神 Token", cloud.china.genshin.token.as_ref())?;
    let china_genshin_enabled = prompt_bool("启用国内云原神", cloud.china.genshin.enabled)?;
    let china_zzz_token = prompt_secret(
        "国内云绝区零 Token",
        cloud.china.zenless_zone_zero.token.as_ref(),
    )?;
    let china_zzz_enabled = prompt_bool("启用国内云绝区零", cloud.china.zenless_zone_zero.enabled)?;
    let overseas_language = prompt_keep(
        "国际服语言(zh-cn/en-us/ja-jp/ko-kr)",
        &cloud.overseas.language,
    )?;
    let overseas_genshin_token =
        prompt_secret("国际服云原神 Token", cloud.overseas.genshin.token.as_ref())?;
    let overseas_genshin_enabled = prompt_bool("启用国际服云原神", cloud.overseas.genshin.enabled)?;
    set_account_cloud_games(
        path,
        &name,
        china_genshin_enabled,
        china_genshin_token.as_deref(),
        china_zzz_enabled,
        china_zzz_token.as_deref(),
        &overseas_language,
        overseas_genshin_enabled,
        overseas_genshin_token.as_deref(),
    )
}

fn account_china_checkin(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let current = load(path)?
        .config
        .accounts
        .into_iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在")
        .china_checkin;
    let user_agent = prompt_keep("国内签到 User-Agent", &current.user_agent)?;
    println!("角色黑名单填写完整 UID，多个 UID 使用逗号分隔；留空保留，- 清空");
    let role_blacklist = RoleBlacklistConfig {
        genshin: prompt_list_keep("原神 UID 黑名单", &current.role_blacklist.genshin)?,
        honkai2: prompt_list_keep("崩坏学园2 UID 黑名单", &current.role_blacklist.honkai2)?,
        honkai3rd: prompt_list_keep("崩坏3 UID 黑名单", &current.role_blacklist.honkai3rd)?,
        tears_of_themis: prompt_list_keep(
            "未定事件簿 UID 黑名单",
            &current.role_blacklist.tears_of_themis,
        )?,
        star_rail: prompt_list_keep(
            "崩坏：星穹铁道 UID 黑名单",
            &current.role_blacklist.star_rail,
        )?,
        zenless_zone_zero: prompt_list_keep(
            "绝区零 UID 黑名单",
            &current.role_blacklist.zenless_zone_zero,
        )?,
    };
    set_account_china_checkin(path, &name, &user_agent, &role_blacklist)
}

fn account_hoyolab(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let account = load(path)?
        .config
        .accounts
        .into_iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在");
    let current = account.hoyolab.unwrap_or_else(|| HoyolabConfig {
        cookie: account.credentials.cookie,
        language: "en-us".to_owned(),
        games: account.games,
        ..HoyolabConfig::default()
    });
    println!("已保存的 HoYoLAB Cookie 不会回显；新输入同样隐藏");
    let current_cookie = (!current.cookie.is_empty()).then_some(&current.cookie);
    let cookie = prompt_secret("HoYoLAB 独立 Cookie", current_cookie)?.unwrap_or_default();
    let language = prompt_keep("HoYoLAB 语言(zh-cn/en-us/ja-jp/ko-kr)", &current.language)?;
    let user_agent = prompt_keep("HoYoLAB User-Agent", &current.user_agent)?;
    println!("HoYoLAB 游戏：1.原神 2.崩坏3 3.未定事件簿 4.星穹铁道 5.绝区零；留空取消");
    let Some(selected) = read_choice(5)? else {
        return Ok(());
    };
    if selected == [0] {
        return Ok(());
    }
    let games = selected
        .into_iter()
        .filter_map(|number| {
            [1, 3, 4, 5, 6]
                .get((number as usize).saturating_sub(1))
                .copied()
        })
        .collect::<Vec<_>>();
    set_account_hoyolab(path, &name, &cookie, &language, &user_agent, &games)
}

fn notifications(path: &Path) -> Result<(), ConfigError> {
    loop {
        let current = load(path)?.config.notifications;
        println!(
            "通知：[{}]，已配置 {} 个渠道\n1.通用选项 2.添加渠道 3.编辑渠道 4.删除渠道 0.返回",
            if current.enabled { "已启用" } else { "已关闭" },
            current.providers.len()
        );
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
        let kind = provider_type(provider);
        println!("{}. {}（{kind}）", index + 1, provider_display(kind));
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
        println!("{}. {}（{kind}）", index + 1, provider_display(kind));
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
    secret: bool,
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
            let label = format!("{}{}", field.name, default);
            let value = if field.secret {
                super::input::prompt_secret(&label)?
            } else {
                prompt(&label)?
            };
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
        "smtp",
        "windows_toast",
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
    const SMTP: &[ProviderField] = &[
        field("host", true, None),
        field("port", true, Some("465")),
        field("from", true, None),
        field("to", true, None),
        field("username", true, None),
        field("password", true, None),
        field("subject", true, Some("MihoyoBBSTools RS")),
        field("tls", true, Some("implicit")),
        field("timeout_seconds", false, None),
    ];
    const WINDOWS_TOAST: &[ProviderField] =
        &[field("title_prefix", false, Some("MihoyoBBSTools RS"))];
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
        "smtp" => SMTP,
        "windows_toast" => WINDOWS_TOAST,
        _ => &[],
    }
}

const fn field(name: &'static str, required: bool, default: Option<&'static str>) -> ProviderField {
    ProviderField {
        name,
        required,
        default,
        secret: matches!(
            name,
            "token"
                | "bot_token"
                | "app_token"
                | "sendkey"
                | "secret"
                | "key"
                | "password"
                | "webhook"
                | "url"
                | "api_url"
                | "proxy"
        ),
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
        NotificationProvider::Smtp { .. } => "smtp",
        NotificationProvider::WindowsToast { .. } => "windows_toast",
    }
}

pub(crate) fn provider_display(kind: &str) -> &'static str {
    match kind {
        "telegram" => "Telegram 机器人",
        "webhook" => "通用 Webhook",
        "pushplus" => "PushPlus",
        "ftqq" => "Server 酱 Turbo",
        "pushme" => "PushMe",
        "cqhttp" => "CQHTTP QQ 机器人",
        "wecom" => "企业微信应用",
        "wecomrobot" => "企业微信群机器人",
        "pushdeer" => "PushDeer",
        "dingrobot" => "钉钉群机器人",
        "feishubot" => "飞书群机器人",
        "bark" => "Bark",
        "gotify" => "Gotify",
        "ifttt" => "IFTTT Webhooks",
        "qmsg" => "Qmsg 酱",
        "discord" => "Discord Webhook",
        "wxpusher" => "WxPusher",
        "serverchan3" => "Server 酱 3",
        "smtp" => "SMTP 邮件",
        "windows_toast" => "Windows 本地通知",
        _ => "未知通知渠道",
    }
}

fn tasks(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let account = load(path)?
        .config
        .accounts
        .into_iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在");
    let current = &account.tasks;
    let selected = [
        current.china_game_checkin,
        current.hoyolab_checkin,
        current.bbs.enabled,
        current.china_cloud_game,
        current.overseas_cloud_game,
        current.web_activity.enabled,
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(index, enabled)| enabled.then_some(index as u8 + 1))
    .collect::<Vec<_>>();
    let Some(selected) = toggle_menu(
        "账号任务",
        &[
            "国内游戏签到",
            "HoYoLAB 签到",
            "米游社社区任务",
            "国内云游戏",
            "海外云游戏",
            "Web 活动",
        ],
        &selected,
    )? else {
        return Ok(());
    };
    let mut bbs = [
        current.bbs.sign,
        current.bbs.read,
        current.bbs.like,
        current.bbs.cancel_like,
        current.bbs.share,
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(index, enabled)| enabled.then_some(index as u8 + 1))
    .collect::<Vec<_>>();
    let mut forums = current.bbs.forums.clone();
    if selected.contains(&3) {
        let Some(value) = toggle_menu(
            "米游社社区操作",
            &["社区签到", "阅读帖子", "点赞", "取消点赞", "分享"],
            &bbs,
        )? else {
            return Ok(());
        };
        bbs = value;
        let labels = crate::bbs::SUPPORTED_FORUMS
            .iter()
            .map(|forum| forum.name)
            .collect::<Vec<_>>();
        let current_forums = crate::bbs::SUPPORTED_FORUMS
            .iter()
            .enumerate()
            .filter_map(|(index, forum)| forums.contains(&forum.id).then_some(index as u8 + 1))
            .collect::<Vec<_>>();
        let Some(selected_forums) = toggle_menu("社区签到板块（首项也用于获取帖子）", &labels, &current_forums)? else {
            return Ok(());
        };
        forums = selected_forums
            .iter()
            .filter_map(|number| {
                crate::bbs::SUPPORTED_FORUMS.get((*number as usize).saturating_sub(1))
            })
            .map(|forum| forum.id)
            .collect();
    }
    set_account_tasks(path, &name, &selected, &bbs, &forums)
}

fn games(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose(path)? else {
        return Ok(());
    };
    let current = load(path)?
        .config
        .accounts
        .into_iter()
        .find(|account| account.name == name)
        .expect("已选择的账号存在")
        .games
        .into_iter()
        .map(|game| match game {
            super::Game::Genshin => 1,
            super::Game::Honkai2 => 2,
            super::Game::Honkai3rd => 3,
            super::Game::TearsOfThemis => 4,
            super::Game::StarRail => 5,
            super::Game::ZenlessZoneZero => 6,
        })
        .collect::<Vec<_>>();
    let Some(selected) = toggle_menu(
        "国内签到游戏",
        &["原神", "崩坏学园2", "崩坏3", "未定事件簿", "星穹铁道", "绝区零"],
        &current,
    )? else {
        return Ok(());
    };
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
        println!(
            "{}. [{}] {}{}",
            index + 1,
            if account.enabled { "x" } else { " " },
            account.name,
            remark
        );
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
        let value = prompt(&format!(
            "{label}[{}] (启用/关闭，留空保留)",
            if current { "已启用" } else { "已关闭" }
        ))?;
        match value.to_ascii_lowercase().as_str() {
            "" => return Ok(current),
            "1" | "true" | "yes" | "y" => return Ok(true),
            "0" | "false" | "no" | "n" => return Ok(false),
            "启用" | "开启" => return Ok(true),
            "关闭" | "禁用" => return Ok(false),
            _ => println!("请输入启用/关闭、yes/no 或 1/0"),
        }
    }
}

fn toggle_menu(
    title: &str,
    labels: &[&str],
    current: &[u8],
) -> Result<Option<Vec<u8>>, ConfigError> {
    let mut selected = current.to_vec();
    loop {
        println!("\n{title}：");
        for (index, label) in labels.iter().enumerate() {
            let number = index as u8 + 1;
            println!(
                "{}. [{}] {label}",
                number,
                if selected.contains(&number) { "x" } else { " " }
            );
        }
        println!("输入编号切换，a 全选，n 全不选，s 保存，0 取消");
        let value = prompt("")?;
        match value.to_ascii_lowercase().as_str() {
            "s" => return Ok(Some(selected)),
            "0" | "q" | "" => return Ok(None),
            "a" => selected = (1..=labels.len() as u8).collect(),
            "n" => selected.clear(),
            _ => match parse_choices(&value, labels.len() as u8) {
                Ok(numbers) => {
                    for number in numbers {
                        if let Some(index) = selected.iter().position(|value| *value == number) {
                            selected.remove(index);
                        } else {
                            selected.push(number);
                        }
                    }
                    selected.sort_unstable();
                }
                Err(error) => println!("{error}"),
            },
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

fn prompt_optional_hidden(
    label: &str,
    current: Option<&str>,
) -> Result<Option<String>, ConfigError> {
    let value = super::input::prompt_secret(&format!(
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

fn prompt_secret(
    label: &str,
    current: Option<&crate::auth::SecretString>,
) -> Result<Option<String>, ConfigError> {
    let shown = if current.is_some() {
        "<已配置>"
    } else {
        "未设置"
    };
    let value = super::input::prompt_secret(&format!(
        "{label}[{shown}] (留空保留，- 清空)"
    ))?;
    if value.is_empty() {
        Ok(current.map(|value| value.expose_secret().to_owned()))
    } else if value == "-" {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

fn prompt_list_keep(label: &str, current: &[String]) -> Result<Vec<String>, ConfigError> {
    let value = prompt(&format!(
        "{label}[{}] (逗号分隔，留空保留，- 清空)",
        current.join(",")
    ))?;
    if value.is_empty() {
        Ok(current.to_vec())
    } else if value == "-" {
        Ok(Vec::new())
    } else {
        Ok(value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect())
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
    super::input::prompt_text(label)
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
        assert!(
            provider_fields("smtp")
                .iter()
                .any(|field| field.name == "timeout_seconds")
        );
    }
}
