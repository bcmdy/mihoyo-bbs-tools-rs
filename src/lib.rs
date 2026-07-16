pub mod auth;
pub mod automation;
pub mod bbs;
pub mod captcha;
pub mod checkin;
pub mod cli;
pub mod cloud_game;
pub mod config;
pub mod doctor;
pub mod error;
pub mod http;
pub mod launcher;
pub mod push;
pub mod service;
pub mod signing;
pub mod update;

pub const VERSION: &str = match option_env!("MIHOYO_BBS_TOOLS_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
