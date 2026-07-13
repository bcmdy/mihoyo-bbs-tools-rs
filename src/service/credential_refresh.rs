use std::path::Path;

use crate::{
    auth::{AuthClient, Credentials, RefreshOnce},
    config::{Config, persist_refreshed_cookie},
    http::HttpClient,
};

use super::{RunReport, TaskOutcome};

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
    let client = AuthClient::new(http);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{TaskOutcome, TaskRecord};

    #[test]
    fn only_authentication_failures_trigger_refresh() {
        let mut report = RunReport::default();
        report.push(TaskRecord {
            account: "账号".to_owned(),
            task: "签到".to_owned(),
            subject: "原神".to_owned(),
            outcome: TaskOutcome::NetworkFailed,
            message: "网络请求失败".to_owned(),
        });
        assert!(!has_authentication_failure(&report));
        report.records[0].outcome = TaskOutcome::AuthenticationFailed;
        assert!(has_authentication_failure(&report));
    }
}
