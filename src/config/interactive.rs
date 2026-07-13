use super::{
    ConfigError, add_account_from_stdin, edit_file, load, remove_account, set_account_games,
    set_account_tasks, set_captcha_endpoint, set_notification_options, set_runtime,
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
        println!("配置节点：1.全局运行 2.验证码 3.账号 4.通知 5.校验 6.完整编辑 0.退出");
        match read_choice(6)? {
            None => return Ok(()),
            Some(ref v) if v == &[0] => return Ok(()),
            Some(v) if v == [1] => runtime(path)?,
            Some(v) if v == [2] => captcha(path)?,
            Some(v) if v == [3] => accounts(path)?,
            Some(v) if v == [4] => notifications(path)?,
            Some(v) if v == [5] => {
                println!("配置有效：{} 个账号", load(path)?.config.accounts.len())
            }
            Some(v) if v == [6] => edit_file(path)?,
            _ => println!("请选择一个节点"),
        }
    }
}
fn runtime(path: &Path) -> Result<(), ConfigError> {
    let c = load(path)?.config;
    let tz = prompt(&format!("时区[{}]", c.runtime.timezone))?;
    let timeout = prompt(&format!("超时[{}]", c.runtime.request_timeout_seconds))?;
    let retry = prompt(&format!("重试[{}]", c.runtime.retry_count))?;
    let delay = prompt(&format!("延迟[{}]", c.runtime.random_delay_seconds))?;
    println!("日志：1.trace 2.debug 3.info 4.warn 5.error");
    let level = match read_choice(5)?
        .and_then(|v| v.first().copied())
        .unwrap_or(3)
    {
        1 => "trace",
        2 => "debug",
        4 => "warn",
        5 => "error",
        _ => "info",
    };
    set_runtime(
        path,
        if tz.is_empty() {
            &c.runtime.timezone
        } else {
            &tz
        },
        timeout.parse().unwrap_or(c.runtime.request_timeout_seconds),
        retry.parse().unwrap_or(c.runtime.retry_count),
        delay.parse().unwrap_or(c.runtime.random_delay_seconds),
        level,
    )
}
fn captcha(path: &Path) -> Result<(), ConfigError> {
    let v = prompt("验证码端点(留空清除)")?;
    set_captcha_endpoint(path, if v.is_empty() { None } else { Some(&v) })
}
fn accounts(path: &Path) -> Result<(), ConfigError> {
    println!("账号：1.添加 2.任务 3.游戏 4.删除 5.设备/代理/凭据完整编辑 0.返回");
    match read_choice(5)? {
        Some(v) if v == [1] => {
            add_account_from_stdin(path, None)?;
        }
        Some(v) if v == [2] => tasks(path)?,
        Some(v) if v == [3] => games(path)?,
        Some(v) if v == [4] => {
            if let Some(n) = choose(path)? {
                remove_account(path, &n)?
            }
        }
        Some(v) if v == [5] => edit_file(path)?,
        _ => {}
    }
    Ok(())
}
fn notifications(path: &Path) -> Result<(), ConfigError> {
    println!("通知：1.启用 2.仅错误推送(可多选，0关闭)");
    let v = read_choice(2)?.unwrap_or_default();
    let words = prompt("屏蔽关键词(逗号分隔)")?;
    set_notification_options(
        path,
        v.contains(&1),
        v.contains(&2),
        words
            .split(',')
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect(),
    )?;
    println!("渠道类型、Token及URL通过分层后的完整通知节点编辑；现在打开编辑器？1.是 0.否");
    if read_choice(1)?.is_some_and(|v| v == [1]) {
        edit_file(path)?
    }
    Ok(())
}
fn tasks(path: &Path) -> Result<(), ConfigError> {
    let Some(n) = choose(path)? else {
        return Ok(());
    };
    println!(
        "任务：1.国内签到 2.HoYoLAB 3.米游社 4.国内云游戏 5.海外云游戏 6.Web活动；当前未实现项也可配置开关"
    );
    let v = read_choice(6)?.unwrap_or_default();
    println!("米游社：1.签到 2.阅读 3.点赞 4.取消点赞 5.分享");
    let b = if v.contains(&3) {
        read_choice(5)?.unwrap_or_default()
    } else {
        Vec::new()
    };
    set_account_tasks(path, &n, &v, &b)
}
fn games(path: &Path) -> Result<(), ConfigError> {
    let Some(n) = choose(path)? else {
        return Ok(());
    };
    println!("游戏：1.原神 2.崩坏学园2 3.崩坏3 4.未定事件簿 5.星穹铁道 6.绝区零");
    set_account_games(path, &n, &read_choice(6)?.unwrap_or_default())
}
fn choose(path: &Path) -> Result<Option<String>, ConfigError> {
    let c = load(path)?.config;
    for (i, a) in c.accounts.iter().enumerate() {
        println!("{}. {}", i + 1, a.name)
    }
    let v = read_choice(c.accounts.len() as u8)?.unwrap_or_default();
    let Some(&n) = v.first() else { return Ok(None) };
    if n == 0 {
        return Ok(None);
    }
    Ok(c.accounts.get(n as usize - 1).map(|a| a.name.clone()))
}
fn prompt(s: &str) -> Result<String, ConfigError> {
    print!("{s}> ");
    io::stdout()
        .flush()
        .map_err(|_| ConfigError::Edit("输出失败".into()))?;
    let mut v = String::new();
    io::stdin()
        .lock()
        .read_line(&mut v)
        .map_err(|_| ConfigError::Edit("读取失败".into()))?;
    Ok(v.trim().into())
}
fn read_choice(max: u8) -> Result<Option<Vec<u8>>, ConfigError> {
    loop {
        let v = prompt("")?;
        if v.is_empty() {
            return Ok(None);
        }
        match parse_choices(&v, max) {
            Ok(v) => return Ok(Some(v)),
            Err(e) => println!("{e}"),
        }
    }
}
pub fn parse_choices(s: &str, max: u8) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    for c in s.chars().filter(|c| !c.is_whitespace() && *c != ',') {
        if !c.is_ascii_digit() {
            return Err("请输入数字");
        }
        let n = c.to_digit(10).unwrap() as u8;
        if n > max {
            return Err("编号超出范围");
        }
        if !out.contains(&n) {
            out.push(n)
        }
    }
    if out.is_empty() {
        return Err("输入不能为空");
    }
    if out.contains(&0) && out.len() > 1 {
        return Err("0不能与其他编号同时选择");
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices() {
        assert_eq!(parse_choices("1,2,2,3", 3).unwrap(), vec![1, 2, 3]);
        assert!(parse_choices("-1", 3).is_err())
    }
}
