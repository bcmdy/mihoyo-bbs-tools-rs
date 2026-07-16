use std::io::{self, BufRead, IsTerminal, Write};

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

    let _echo = if terminal && secret {
        Some(EchoGuard::disable()?)
    } else {
        None
    };
    let mut value = String::new();
    let bytes = io::stdin()
        .lock()
        .read_line(&mut value)
        .map_err(|_| ConfigError::Edit("无法从标准输入读取内容".to_owned()))?;
    if terminal && secret {
        println!();
    }
    if bytes == 0 {
        return Err(ConfigError::Edit(
            "标准输入已结束，未读取任何内容".to_owned(),
        ));
    }
    Ok(value.trim().to_owned())
}

struct EchoGuard {
    #[cfg(windows)]
    handle: *mut std::ffi::c_void,
    #[cfg(windows)]
    original_mode: u32,
    #[cfg(unix)]
    active: bool,
}

impl EchoGuard {
    #[cfg(windows)]
    fn disable() -> Result<Self, ConfigError> {
        const STD_INPUT_HANDLE: u32 = -10_i32 as u32;
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
        if unsafe { SetConsoleMode(handle, original_mode & !ENABLE_ECHO_INPUT) } == 0 {
            return Err(ConfigError::Edit("无法关闭终端输入回显".to_owned()));
        }
        Ok(Self {
            handle,
            original_mode,
        })
    }

    #[cfg(unix)]
    fn disable() -> Result<Self, ConfigError> {
        let status = std::process::Command::new("stty")
            .arg("-echo")
            .status()
            .map_err(|_| ConfigError::Edit("无法启动 stty 关闭终端回显".to_owned()))?;
        if !status.success() {
            return Err(ConfigError::Edit("无法关闭终端输入回显".to_owned()));
        }
        Ok(Self { active: true })
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
        if self.active {
            let _ = std::process::Command::new("stty").arg("echo").status();
        }
    }
}
