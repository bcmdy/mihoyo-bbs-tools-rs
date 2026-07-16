use std::io::{self, BufRead, IsTerminal, Read, Write};

use super::ConfigError;

/// 读取普通文本。交互式终端显示提示，重定向输入时只读取一行。
pub fn prompt_text(label: &str) -> Result<String, ConfigError> {
    read_line(label, false)
}

/// 读取敏感文本。交互式终端关闭输入回显，重定向输入时不输出提示。
pub fn prompt_secret(label: &str) -> Result<String, ConfigError> {
    read_line(label, true)
}

/// 统一读取 yes/no 确认；空输入使用调用方提供的默认值。
pub fn confirm(label: &str, default: bool) -> Result<bool, ConfigError> {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    loop {
        let value = prompt_text(&format!("{label} {suffix}"))?;
        match value.to_ascii_lowercase().as_str() {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("请输入 y/yes 或 n/no"),
        }
    }
}

fn read_line(label: &str, secret: bool) -> Result<String, ConfigError> {
    let terminal = io::stdin().is_terminal();
    if terminal {
        print!("{label}> ");
        io::stdout()
            .flush()
            .map_err(|_| ConfigError::Edit("无法输出交互提示".to_owned()))?;
    }

    if terminal && secret {
        let _echo = EchoGuard::disable()?;
        let result = read_secret_terminal();
        drop(_echo);
        println!();
        return result;
    }
    let mut value = String::new();
    let bytes = io::stdin()
        .lock()
        .read_line(&mut value)
        .map_err(|_| ConfigError::Edit("无法从标准输入读取内容".to_owned()))?;
    if bytes == 0 {
        return Err(ConfigError::Edit(
            "标准输入已结束，未读取任何内容".to_owned(),
        ));
    }
    Ok(value.trim().to_owned())
}

fn read_secret_terminal() -> Result<String, ConfigError> {
    let mut value = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        let read = io::stdin()
            .read(&mut byte)
            .map_err(|_| ConfigError::Edit("无法从终端读取敏感内容".to_owned()))?;
        if read == 0 {
            return Err(ConfigError::Edit(
                "终端输入已结束，未读取任何内容".to_owned(),
            ));
        }
        match byte[0] {
            b'\r' | b'\n' => break,
            3 | 4 | 26 | 28 => {
                return Err(ConfigError::Edit("已取消敏感信息输入".to_owned()));
            }
            8 | 127 => remove_last_utf8_character(&mut value),
            byte => value.push(byte),
        }
    }
    String::from_utf8(value)
        .map(|value| value.trim().to_owned())
        .map_err(|_| ConfigError::Edit("终端输入不是有效 UTF-8 文本".to_owned()))
}

fn remove_last_utf8_character(value: &mut Vec<u8>) {
    while let Some(byte) = value.pop() {
        if byte & 0b1100_0000 != 0b1000_0000 {
            break;
        }
    }
}

struct EchoGuard {
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    #[cfg(windows)]
    original_mode: u32,
    #[cfg(unix)]
    original: String,
}

impl EchoGuard {
    #[cfg(windows)]
    fn disable() -> Result<Self, ConfigError> {
        const STD_INPUT_HANDLE: u32 = -10_i32 as u32;
        const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
        const ENABLE_LINE_INPUT: u32 = 0x0002;
        const ENABLE_ECHO_INPUT: u32 = 0x0004;

        #[link(name = "Kernel32")]
        #[allow(non_snake_case)]
        unsafe extern "system" {
            fn GetStdHandle(std_handle: u32) -> *mut std::ffi::c_void;
            fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
            fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
        }

        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle.is_null() || handle as isize == -1 {
            return Err(ConfigError::Edit("无法访问终端输入句柄".to_owned()));
        }
        let mut original_mode = 0;
        if unsafe { GetConsoleMode(handle, &mut original_mode) } == 0 {
            return Err(ConfigError::Edit("无法读取终端输入模式".to_owned()));
        }
        let secret_mode =
            original_mode & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT);
        if unsafe { SetConsoleMode(handle, secret_mode) } == 0 {
            return Err(ConfigError::Edit("无法关闭终端输入回显".to_owned()));
        }
        Ok(Self {
            handle,
            original_mode,
        })
    }

    #[cfg(unix)]
    fn disable() -> Result<Self, ConfigError> {
        let original = std::process::Command::new("stty")
            .arg("-g")
            .output()
            .map_err(|_| ConfigError::Edit("无法读取终端输入模式".to_owned()))?;
        if !original.status.success() {
            return Err(ConfigError::Edit("无法读取终端输入模式".to_owned()));
        }
        let original = String::from_utf8_lossy(&original.stdout).trim().to_owned();
        let status = std::process::Command::new("stty")
            .args(["-echo", "-icanon", "-isig", "min", "1", "time", "0"])
            .status()
            .map_err(|_| ConfigError::Edit("无法启动 stty 关闭终端回显".to_owned()))?;
        if !status.success() {
            return Err(ConfigError::Edit("无法关闭终端输入回显".to_owned()));
        }
        Ok(Self { original })
    }
}

impl Drop for EchoGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            #[link(name = "Kernel32")]
            #[allow(non_snake_case)]
            unsafe extern "system" {
                fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
            }
            let _ = unsafe { SetConsoleMode(self.handle, self.original_mode) };
        }
        #[cfg(unix)]
        if !self.original.is_empty() {
            let _ = std::process::Command::new("stty")
                .arg(&self.original)
                .status();
        }
    }
}
