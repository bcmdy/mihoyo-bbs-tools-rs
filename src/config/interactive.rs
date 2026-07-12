use super::{
    ConfigError, add_account_from_stdin, edit_file, load, remove_account, set_account_games,
    set_account_tasks,
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
        println!(
            "请选择操作：\n1. 添加账号\n2. 设置账号任务\n3. 设置账号游戏\n4. 删除账号\n5. 编辑完整配置\n0. 退出"
        );
        match read_choice(5)? {
            None => return Ok(()),
            Some(v) if v == [0] => return Ok(()),
            Some(v) if v == [1] => {
                add_account_from_stdin(path, None)?;
            }
            Some(v) if v == [2] => configure_tasks(path)?,
            Some(v) if v == [3] => configure_games(path)?,
            Some(v) if v == [4] => {
                if let Some(name) = choose_account(path)? {
                    remove_account(path, &name)?;
                }
            }
            Some(v) if v == [5] => edit_file(path)?,
            _ => println!("一级菜单只能选择一个编号"),
        }
    }
}
fn configure_tasks(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose_account(path)? else {
        return Ok(());
    };
    println!("选择任务：1.国内签到 2.HoYoLAB 3.米游社；0取消");
    let Some(tasks) = read_choice(3)? else {
        return Ok(());
    };
    if tasks.contains(&0) {
        return Ok(());
    }
    let bbs = if tasks.contains(&3) {
        println!("米游社：1.签到 2.阅读 3.点赞 4.取消点赞 5.分享；0取消");
        let Some(v) = read_choice(5)? else {
            return Ok(());
        };
        if v.contains(&0) {
            return Ok(());
        }
        v
    } else {
        Vec::new()
    };
    set_account_tasks(path, &name, &tasks, &bbs)
}
fn configure_games(path: &Path) -> Result<(), ConfigError> {
    let Some(name) = choose_account(path)? else {
        return Ok(());
    };
    println!("选择游戏：1.原神 2.崩坏学园2 3.崩坏3 4.未定事件簿 5.星穹铁道 6.绝区零；0取消");
    let Some(v) = read_choice(6)? else {
        return Ok(());
    };
    if v.contains(&0) {
        return Ok(());
    }
    set_account_games(path, &name, &v)
}
fn choose_account(path: &Path) -> Result<Option<String>, ConfigError> {
    let loaded = load(path)?;
    for (i, a) in loaded.config.accounts.iter().enumerate() {
        println!("{}. {}", i + 1, a.name)
    }
    println!("0. 取消");
    let Some(v) = read_choice(loaded.config.accounts.len() as u8)? else {
        return Ok(None);
    };
    let n = v[0];
    if n == 0 {
        return Ok(None);
    }
    Ok(loaded
        .config
        .accounts
        .get(n as usize - 1)
        .map(|a| a.name.clone()))
}
fn read_choice(max: u8) -> Result<Option<Vec<u8>>, ConfigError> {
    loop {
        print!("> ");
        io::stdout()
            .flush()
            .map_err(|_| ConfigError::Edit("无法输出交互提示".into()))?;
        let mut line = String::new();
        if io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|_| ConfigError::Edit("无法读取输入".into()))?
            == 0
        {
            return Ok(None);
        }
        match parse_choices(&line, max) {
            Ok(v) => return Ok(Some(v)),
            Err(e) => println!("{e}"),
        }
    }
}
pub fn parse_choices(input: &str, max: u8) -> Result<Vec<u8>, &'static str> {
    let s = input.trim();
    if s.is_empty() {
        return Err("输入不能为空");
    }
    let chars: Vec<_> = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    if chars.is_empty() || chars.iter().any(|c| !c.is_ascii_digit()) {
        return Err("请输入有效数字");
    }
    let mut out = Vec::new();
    for c in chars {
        let n = c.to_digit(10).ok_or("请输入有效数字")? as u8;
        if n > max {
            return Err("编号超出范围");
        }
        if !out.contains(&n) {
            out.push(n)
        }
    }
    if out.contains(&0) && out.len() > 1 {
        return Err("0 不能与其他编号同时选择");
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices() {
        assert_eq!(parse_choices("1, 2,2,3", 3).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_choices("123", 3).unwrap(), vec![1, 2, 3]);
        assert!(parse_choices("-1", 3).is_err());
    }
}
