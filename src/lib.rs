pub mod auth;
pub mod bbs;
pub mod captcha;
pub mod checkin;
pub mod cli;
pub mod config;
pub mod error;
pub mod http;
pub mod push;
pub mod service;
pub mod signing;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
