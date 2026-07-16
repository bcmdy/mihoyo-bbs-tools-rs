use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::http::{HttpClient, HttpError};

const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/bcmdy/mihoyo-bbs-tools-rs/releases/latest";

#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub release_url: String,
    pub config_compatibility: String,
}

pub async fn check() -> Result<UpdateInfo, HttpError> {
    let response: LatestRelease = HttpClient::builder()
        .build()?
        .get_json(Url::parse(LATEST_RELEASE_API).expect("valid GitHub release API URL"))
        .await?;
    let current = crate::VERSION.trim_start_matches('v');
    let latest = response.tag_name.trim_start_matches('v');
    Ok(UpdateInfo {
        current_version: crate::VERSION.to_owned(),
        latest_version: response.tag_name,
        update_available: version_is_newer(latest, current),
        release_url: response.html_url,
        config_compatibility: format!(
            "当前程序使用配置 version {}；升级前请查看发布说明中的兼容提示",
            crate::config::CURRENT_CONFIG_VERSION
        ),
    })
}

fn version_is_newer(candidate: &str, current: &str) -> bool {
    let candidate = version_parts(candidate);
    let current = version_parts(current);
    for index in 0..candidate.len().max(current.len()) {
        match candidate.get(index).copied().unwrap_or(0).cmp(
            &current.get(index).copied().unwrap_or(0),
        ) {
            std::cmp::Ordering::Greater => return true,
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    false
}

fn version_parts(value: &str) -> Vec<u64> {
    value
        .split(['.', '-', '+'])
        .map_while(|part| part.parse::<u64>().ok())
        .collect()
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}
