use std::path::Path;

use crate::{
    auth::{
        AuthClient, BackfillOutcome, Credentials, PassportDevice, RefreshOnce, backfill_v2_tokens,
    },
    config::{Config, persist_refreshed_cookie},
    http::HttpClient,
};

use super::{RunReport, TaskOutcome, resolve_device_id};

/// 进程级 `x-rpc-lifecycle_id`。
///
/// 通行证接口要求同一进程内保持一致；不存在跨进程要求，因此进程启动时生成一次即可。
/// 这里用项目已有的 `rand` 生成 v4 格式 UUID，避免为 `uuid` 增加 `v4` 特性而改变
/// `Cargo.lock`（CI 使用 `--locked` 构建）。
fn process_lifecycle_id() -> String {
    use std::sync::OnceLock;

    static LIFECYCLE_ID: OnceLock<String> = OnceLock::new();
    LIFECYCLE_ID.get_or_init(random_uuid_v4).clone()
}

/// 生成随机 v4 UUID 字符串。
fn random_uuid_v4() -> String {
    use rand::Rng;

    let mut bytes = [0u8; 16];
    rand::rng().fill(&mut bytes);
    // RFC 4122：版本号为 4，variant 为 10xx。
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15],
    )
}

pub(super) fn has_authentication_failure(report: &RunReport) -> bool {
    report
        .records
        .iter()
        .any(|record| record.outcome == TaskOutcome::AuthenticationFailed)
}

pub(super) async fn refresh_account_cookie(
    config: &mut Config,
    account_index: usize,
    http: HttpClient,
    config_path: Option<&Path>,
) -> Result<bool, String> {
    let account = config
        .accounts
        .get(account_index)
        .ok_or_else(|| "待刷新的账号不存在".to_owned())?;
    let account_name = account.name.clone();
    let mut credentials = Credentials::new(
        account.credentials.cookie.expose_secret(),
        account.credentials.stoken.expose_secret(),
    );
    let client = AuthClient::new(http).device(passport_device(account));
    let mut refresh = RefreshOnce::default();
    refresh
        .refresh_cookie_token(&client, &mut credentials)
        .await
        .map_err(|error| error.to_string())?;

    let persisted = if let Some(path) = config_path {
        persist_refreshed_cookie(path, &account_name, credentials.cookie.expose_secret())
            .map_err(|error| error.to_string())?
    } else {
        false
    };
    config.accounts[account_index].credentials.cookie = credentials.cookie;
    Ok(persisted)
}

/// 通行证换票所需的设备上下文。
fn passport_device(account: &crate::config::AccountConfig) -> PassportDevice {
    let device_id = resolve_device_id(
        &account.device.id,
        account.credentials.cookie.expose_secret(),
    );
    PassportDevice::new(
        device_id,
        account.device.name.clone(),
        account.device.model.clone(),
        account.device.fp.clone(),
    )
    .lifecycle_id(process_lifecycle_id())
}

/// 为缺少 `cookie_token` / `ltoken` 的 V2 账号补齐凭据，并把结果写回配置。
///
/// 只在字段确实缺失时发起请求；补齐失败不影响本次任务继续执行。
pub(super) async fn backfill_account_credentials(
    config: &mut Config,
    account_index: usize,
    http: HttpClient,
    config_path: Option<&Path>,
) -> Result<BackfillOutcome, String> {
    let account = config
        .accounts
        .get(account_index)
        .ok_or_else(|| "待补齐的账号不存在".to_owned())?;
    let account_name = account.name.clone();
    let mut credentials = Credentials::new(
        account.credentials.cookie.expose_secret(),
        account.credentials.stoken.expose_secret(),
    );
    let client = AuthClient::new(http).device(passport_device(account));
    let outcome = backfill_v2_tokens(&client, &mut credentials)
        .await
        .map_err(|error| error.to_string())?;
    if outcome != BackfillOutcome::Updated {
        return Ok(outcome);
    }

    if let Some(path) = config_path {
        persist_refreshed_cookie(path, &account_name, credentials.cookie.expose_secret())
            .map_err(|error| error.to_string())?;
    }
    config.accounts[account_index].credentials.cookie = credentials.cookie;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{TaskOutcome, TaskRecord};

    #[test]
    fn only_authentication_failures_trigger_refresh() {
        let mut report = RunReport::default();
        report.push(TaskRecord::new(
            "账号",
            "签到",
            "原神",
            TaskOutcome::NetworkFailed,
            "网络请求失败",
        ));
        assert!(!has_authentication_failure(&report));
        report.records[0].outcome = TaskOutcome::AuthenticationFailed;
        assert!(has_authentication_failure(&report));
    }
}
