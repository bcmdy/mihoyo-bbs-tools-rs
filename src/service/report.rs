use std::fmt::Write;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    AlreadyCompleted,
    Skipped,
    StateSyncTimeout,
    Failed,
    AuthenticationFailed,
    CaptchaRequired,
    NetworkFailed,
}

/// 稳定的任务标识，序列化为 snake_case，供外部脚本按标识匹配，不受中文文案调整影响。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskId {
    /// 国内游戏签到
    ChinaCheckin,
    /// HoYoLAB 游戏签到
    HoyolabCheckin,
    /// 米游社社区签到
    BbsCheckin,
    /// 米游社社区阅读任务
    BbsRead,
    /// 米游社社区点赞任务
    BbsLike,
    /// 米游社社区取消点赞任务
    BbsUnlike,
    /// 米游社社区分享任务
    BbsShare,
    /// 米游币汇总（含任务状态、完成确认、实时汇总）
    BbsCoins,
    /// 米游币「任务状态」汇总（成功时不进入成功列表）
    BbsCoinStatus,
    /// 国内云游戏签到
    ChinaCloudGame,
    /// 国际服云游戏签到
    OverseasCloudGame,
    /// Web 活动
    WebActivity,
    /// Cookie / SToken 自动刷新
    CredentialRefresh,
    /// 通知发送
    Notification,
    /// 配置相关问题
    Config,
    /// 未归类的米游社动作
    BbsOther,
}

/// 记录类别：任务结果、诊断信息、汇总信息。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// 真正执行的任务结果，参与首屏任务计数。
    #[default]
    Task,
    /// 诊断或配置类提示，不参与任务计数。
    Diagnostic,
    /// 汇总记录（米游币合计等），不参与任务计数。
    Summary,
}

impl RecordKind {
    /// 是否计入任务数量统计。
    pub const fn counts_as_task(self) -> bool {
        matches!(self, Self::Task)
    }
}

/// 结构化业务数据；缺省时不写出字段，保持旧报告精简。
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RecordDetails {
    /// 奖励描述（例如「原石 ×20」）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward: Option<String>,
    /// 与米游币相关的数值（已领取 / 还可领取 / 总计）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coins: Option<CoinDetails>,
    /// 实际尝试次数。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts: Option<u32>,
    /// 是否为社区任务的横向汇总行（subject 为「米游币任务」）。
    #[serde(skip_serializing_if = "is_false")]
    pub task_rollup: bool,
}

/// `skip_serializing_if` 谓词：`false` 为默认值时不写出字段。
fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CoinDetails {
    pub received: u32,
    pub remaining: u32,
    pub total: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskRecord {
    pub account: String,
    pub task: String,
    pub subject: String,
    pub outcome: TaskOutcome,
    pub message: String,
    /// 稳定任务标识；无法归类时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// 记录类别，缺省表示任务结果。
    #[serde(default)]
    pub record_kind: RecordKind,
    /// 稳定的错误码（如 `cookie_invalid`、`retcode:-100`），便于外部聚合。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// 结构化业务数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<RecordDetails>,
}

impl TaskRecord {
    /// 构造任务结果记录；任务标识与结构化数据留空，由调用方按需补充。
    pub fn new(
        account: impl Into<String>,
        task: impl Into<String>,
        subject: impl Into<String>,
        outcome: TaskOutcome,
        message: impl Into<String>,
    ) -> Self {
        Self {
            account: account.into(),
            task: task.into(),
            subject: subject.into(),
            outcome,
            message: message.into(),
            task_id: None,
            record_kind: RecordKind::Task,
            error_code: None,
            details: None,
        }
    }

    pub fn with_task_id(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    pub fn with_kind(mut self, record_kind: RecordKind) -> Self {
        self.record_kind = record_kind;
        self
    }

    pub fn with_error_code(mut self, error_code: impl Into<String>) -> Self {
        self.error_code = Some(error_code.into());
        self
    }

    pub fn with_details(mut self, details: RecordDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// 是否为米游币汇总记录。
    pub fn is_coin_summary(&self) -> bool {
        matches!(self.task_id, Some(TaskId::BbsCoins | TaskId::BbsCoinStatus))
            && self.record_kind == RecordKind::Summary
    }

    /// 是否为需要在成功列表中折叠的隐藏汇总记录。
    fn is_hidden_summary(&self) -> bool {
        self.task_id == Some(TaskId::BbsCoinStatus) && self.outcome == TaskOutcome::Success
    }

    /// 是否为可折叠的社区重复明细（阅读 / 点赞 / 取消点赞 / 分享）。
    ///
    /// 横向汇总行用 `task_rollup` 标记，不再比较中文 subject。
    fn is_repeated_community_detail(&self) -> bool {
        matches!(
            self.task_id,
            Some(TaskId::BbsRead | TaskId::BbsLike | TaskId::BbsUnlike | TaskId::BbsShare)
        ) && !self
            .details
            .as_ref()
            .is_some_and(|details| details.task_rollup)
    }

    /// 是否为通知相关的失败记录。
    fn is_notification_failure(&self) -> bool {
        self.task_id == Some(TaskId::Notification)
    }
}

/// 首屏任务计数；只统计 `record_kind` 为任务类别的记录。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct TaskCounts {
    pub success: usize,
    pub already_completed: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RunReport {
    pub records: Vec<TaskRecord>,
}

impl RunReport {
    pub fn push(&mut self, record: TaskRecord) {
        self.records.push(record);
    }

    pub fn extend(&mut self, other: Self) {
        self.records.extend(other.records);
    }

    /// 统计首屏任务数量；诊断与汇总记录不计入。
    pub fn task_counts(&self) -> TaskCounts {
        let mut counts = TaskCounts::default();
        for record in self
            .records
            .iter()
            .filter(|record| record.record_kind.counts_as_task())
        {
            match record.outcome {
                TaskOutcome::Success => counts.success += 1,
                TaskOutcome::AlreadyCompleted => counts.already_completed += 1,
                TaskOutcome::Skipped => counts.skipped += 1,
                _ => counts.failed += 1,
            }
        }
        counts
    }

    pub fn exit_code(&self) -> u8 {
        if self
            .records
            .iter()
            .any(|record| record.outcome == TaskOutcome::AuthenticationFailed)
        {
            3
        } else if self
            .records
            .iter()
            .any(|record| record.outcome == TaskOutcome::CaptchaRequired)
        {
            4
        } else if self
            .records
            .iter()
            .any(|record| record.outcome == TaskOutcome::NetworkFailed)
        {
            5
        } else if self.records.iter().any(|record| {
            matches!(
                record.outcome,
                TaskOutcome::StateSyncTimeout | TaskOutcome::Failed
            )
        }) {
            1
        } else {
            0
        }
    }

    pub fn render_text(&self) -> String {
        let mut output = String::new();
        let counts = self.task_counts();
        let TaskCounts {
            success,
            already_completed: completed,
            skipped,
            failed,
        } = counts;
        let _ = writeln!(
            output,
            "运行完成：成功 {success}，今日已完成 {completed}，跳过 {skipped}，失败 {failed}"
        );

        self.write_successes(&mut output);
        self.write_grouped_records(
            &mut output,
            "跳过",
            |outcome| outcome == TaskOutcome::Skipped,
            false,
        );

        let problems = self
            .records
            .iter()
            .filter(|record| is_problem(record.outcome))
            .collect::<Vec<_>>();
        if problems.is_empty() {
            let _ = writeln!(output, "\n本次没有需要处理的问题。");
        } else {
            let _ = writeln!(output, "\n本次需要处理的问题：");
            self.write_records_by_account(&mut output, &problems, true);
        }
        let _ = writeln!(output, "\n完整逐项记录可使用 `run --verbose` 查看。");
        output
    }

    pub fn render_verbose_text(&self) -> String {
        let mut output = self.render_text();
        let _ = writeln!(output, "\n详细记录：");
        if self.records.is_empty() {
            let _ = writeln!(output, "（无任务记录）");
            return output;
        }
        for record in &self.records {
            let _ = writeln!(
                output,
                "[{}] {} / {} / {}：{}",
                outcome_name(record.outcome),
                record.account,
                record.task,
                record.subject,
                record.message
            );
        }
        output
    }

    fn write_successes(&self, output: &mut String) {
        let records = self
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.outcome,
                    TaskOutcome::Success | TaskOutcome::AlreadyCompleted
                ) && !record.is_hidden_summary()
            })
            .collect::<Vec<_>>();
        if records.is_empty() {
            return;
        }

        let _ = writeln!(output, "\n成功：");
        for account in account_order(&records) {
            let _ = writeln!(output, "- {account}");
            let coin_summary = records
                .iter()
                .copied()
                .rev()
                .find(|record| record.account == account && record.is_coin_summary());
            let mut coin_summary_written = false;
            for record in records.iter().copied().filter(|record| {
                record.account == account
                    && !record.is_repeated_community_detail()
                    && !record.is_coin_summary()
            }) {
                if record.task_id == Some(TaskId::BbsCheckin)
                    && !coin_summary_written
                    && let Some(summary) = coin_summary
                {
                    let _ = writeln!(
                        output,
                        "  / {} / {}：{}；{}",
                        record.task,
                        record.subject,
                        record.message,
                        summary.message.trim_start_matches("复查后")
                    );
                    coin_summary_written = true;
                    continue;
                }
                write_success_record(output, record);
            }
            if !coin_summary_written && let Some(summary) = coin_summary {
                write_success_record(output, summary);
            }
            for (task_id, task) in COMMUNITY_DETAIL_TASKS {
                let count = records
                    .iter()
                    .filter(|record| {
                        record.account == account
                            && record.task_id == Some(*task_id)
                            && record.is_repeated_community_detail()
                    })
                    .count();
                if count > 0 {
                    let _ = writeln!(output, "  / {task}：已完成 {count} 项；任务状态复查已确认");
                }
            }
        }
    }

    fn write_grouped_records(
        &self,
        output: &mut String,
        title: &str,
        predicate: impl Fn(TaskOutcome) -> bool,
        remediation_required: bool,
    ) {
        let records = self
            .records
            .iter()
            .filter(|record| predicate(record.outcome))
            .collect::<Vec<_>>();
        if records.is_empty() {
            return;
        }
        let _ = writeln!(output, "\n{title}：");
        self.write_records_by_account(output, &records, remediation_required);
    }

    fn write_records_by_account(
        &self,
        output: &mut String,
        records: &[&TaskRecord],
        remediation_required: bool,
    ) {
        for account in account_order(records) {
            let _ = writeln!(output, "- {account}");
            for record in records
                .iter()
                .copied()
                .filter(|record| record.account == account)
            {
                let _ = writeln!(
                    output,
                    "  / [{}] {} / {}：{}",
                    outcome_name(record.outcome),
                    record.task,
                    record.subject,
                    record.message
                );
                if remediation_required {
                    let _ = writeln!(output, "    处理方式：{}", remediation(record));
                }
            }
        }
    }
}

/// 社区重复明细的（稳定标识，展示名）对照表；顺序决定文本输出顺序。
const COMMUNITY_DETAIL_TASKS: [(TaskId, &str); 4] = [
    (TaskId::BbsRead, "阅读"),
    (TaskId::BbsLike, "点赞"),
    (TaskId::BbsUnlike, "取消点赞"),
    (TaskId::BbsShare, "分享"),
];

fn account_order<'a>(records: &[&'a TaskRecord]) -> Vec<&'a str> {
    let mut accounts = Vec::new();
    for record in records {
        if !accounts.contains(&record.account.as_str()) {
            accounts.push(record.account.as_str());
        }
    }
    accounts
}

fn write_success_record(output: &mut String, record: &TaskRecord) {
    let completed_prefix =
        if record.is_coin_summary() && record.outcome == TaskOutcome::AlreadyCompleted {
            "今日已完成；"
        } else {
            ""
        };
    let _ = writeln!(
        output,
        "  / {} / {}：{}{}",
        record.task, record.subject, completed_prefix, record.message
    );
}

pub const fn outcome_name(outcome: TaskOutcome) -> &'static str {
    match outcome {
        TaskOutcome::Success => "成功",
        TaskOutcome::AlreadyCompleted => "今日已完成",
        TaskOutcome::Skipped => "已跳过",
        TaskOutcome::StateSyncTimeout => "状态同步超时",
        TaskOutcome::AuthenticationFailed => "认证失效",
        TaskOutcome::CaptchaRequired => "需要验证码",
        TaskOutcome::NetworkFailed => "网络失败",
        TaskOutcome::Failed => "执行失败",
    }
}

const fn is_problem(outcome: TaskOutcome) -> bool {
    matches!(
        outcome,
        TaskOutcome::AuthenticationFailed
            | TaskOutcome::CaptchaRequired
            | TaskOutcome::NetworkFailed
            | TaskOutcome::StateSyncTimeout
            | TaskOutcome::Failed
    )
}

fn remediation(record: &TaskRecord) -> &'static str {
    if let Some(code) = record.error_code.as_deref() {
        match code {
            "cookie_invalid" | "stoken_invalid" | "mid_missing" => {
                return "重新获取包含 UID、SToken 和 MID 的完整 Cookie，然后在 `config setup` 中更新账号 Cookie。";
            }
            "notification_failed" => return "运行 `notification test` 单独检查通知渠道。",
            _ => {}
        }
    }
    if record.is_notification_failure() && record.outcome == TaskOutcome::Failed {
        return "运行 `notification test` 单独检查通知渠道。";
    }
    let message = record.message.to_ascii_lowercase();
    if message.contains("stoken") || message.contains("mid") {
        return "重新获取包含 UID、SToken 和 MID 的完整 Cookie，然后在 `config setup` 中更新账号 Cookie。";
    }
    match record.outcome {
        TaskOutcome::AuthenticationFailed => {
            "Cookie 已失效；运行 `config setup`，选择“账号 -> 更新 Cookie”。"
        }
        TaskOutcome::CaptchaRequired => "运行 `config setup` 检查验证码端点，或稍后重试。",
        TaskOutcome::NetworkFailed => "运行 `doctor --online` 检查网络、TLS 和该账号的代理设置。",
        TaskOutcome::StateSyncTimeout => {
            "稍后重新运行；若持续出现，请检查米游社任务状态和账号网络。"
        }
        TaskOutcome::Failed if record.is_notification_failure() => {
            "运行 `notification test` 单独检查通知渠道。"
        }
        TaskOutcome::Failed => "查看上方错误详情；可运行 `doctor --online` 继续诊断。",
        _ => "无需处理。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(outcome: TaskOutcome) -> TaskRecord {
        TaskRecord::new("example", "签到", "原神", outcome, "安全消息")
    }

    fn coin_summary_message(received: u32, remaining: u32, total: u32) -> String {
        format!("已领取 {received}，还可领取 {remaining}，当前共 {total} 米游币")
    }

    fn coin_details(received: u32, remaining: u32, total: u32) -> RecordDetails {
        RecordDetails {
            coins: Some(CoinDetails {
                received,
                remaining,
                total,
            }),
            ..RecordDetails::default()
        }
    }

    #[test]
    fn exit_code_uses_documented_priority() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::NetworkFailed));
        report.push(record(TaskOutcome::CaptchaRequired));
        report.push(record(TaskOutcome::AuthenticationFailed));
        assert_eq!(report.exit_code(), 3);
    }

    #[test]
    fn successful_and_skipped_tasks_exit_zero() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::Success));
        report.push(record(TaskOutcome::AlreadyCompleted));
        report.push(record(TaskOutcome::Skipped));
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn json_report_has_stable_status_names_and_no_config_data() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::AlreadyCompleted));
        report.push(record(TaskOutcome::StateSyncTimeout));
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""outcome":"already_completed""#));
        assert!(json.contains(r#""outcome":"state_sync_timeout""#));
        assert!(!json.contains("cookie"));
        assert!(!json.contains("stoken"));
    }

    #[test]
    fn json_records_expose_stable_task_id_kind_error_code_and_details() {
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "国内游戏签到",
                "原神 / ***1234",
                TaskOutcome::AuthenticationFailed,
                "Cookie 无效或已过期",
            )
            .with_task_id(TaskId::ChinaCheckin)
            .with_error_code("cookie_invalid")
            .with_details(RecordDetails {
                reward: Some("原石 ×20".to_owned()),
                coins: None,
                attempts: Some(2),
            }),
        );
        report.push(
            TaskRecord::new(
                "账号甲",
                "米游币",
                "完成确认",
                TaskOutcome::Success,
                "已领取 30",
            )
            .with_task_id(TaskId::BbsCoins)
            .with_kind(RecordKind::Summary)
            .with_details(coin_details(30, 0, 4249)),
        );

        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""task_id":"china_checkin""#));
        assert!(json.contains(r#""record_kind":"task""#));
        assert!(json.contains(r#""error_code":"cookie_invalid""#));
        assert!(json.contains(r#""reward":"原石 ×20""#));
        assert!(json.contains(r#""attempts":2"#));
        assert!(json.contains(r#""task_id":"bbs_coins""#));
        assert!(json.contains(r#""record_kind":"summary""#));
        assert!(json.contains(r#""coins":{"received":30,"remaining":0,"total":4249}"#));
    }

    #[test]
    fn json_omits_optional_fields_when_absent() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::Success));
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("task_id"));
        assert!(!json.contains("error_code"));
        assert!(!json.contains("details"));
        assert!(json.contains(r#""record_kind":"task""#));
        assert!(!json.contains("stoken"));
        assert!(!json.contains("cookie"));
    }

    #[test]
    fn text_report_is_localized_and_contains_summary_and_remediation() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::Success));
        report.push(record(TaskOutcome::AuthenticationFailed));
        let text = report.render_text();
        assert!(text.contains("运行完成：成功 1，今日已完成 0，跳过 0，失败 1"));
        assert!(text.contains("[认证失效]"));
        assert!(text.contains("处理方式："));
        assert!(!text.contains("[AuthenticationFailed]"));
    }

    #[test]
    fn state_sync_timeout_is_a_problem_with_failed_exit_code() {
        let mut report = RunReport::default();
        report.push(record(TaskOutcome::StateSyncTimeout));

        assert_eq!(report.exit_code(), 1);
        let text = report.render_text();
        assert!(text.contains("[状态同步超时]"));
        assert!(text.contains("稍后重新运行"));
    }

    #[test]
    fn text_report_groups_accounts_and_keeps_rewards_and_coin_summary() {
        let coin_summary = coin_summary_message(17, 33, 2468);
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "国内游戏签到",
                "原神 / ***1234",
                TaskOutcome::Success,
                "签到成功；今日奖励：原石 ×20；累计签到 7 天",
            )
            .with_task_id(TaskId::ChinaCheckin),
        );
        report.push(
            TaskRecord::new(
                "账号甲",
                "社区签到",
                "原神",
                TaskOutcome::Success,
                "第 1 轮复查确认社区签到米游币领取",
            )
            .with_task_id(TaskId::BbsCheckin),
        );
        report.push(
            TaskRecord::new(
                "账号甲",
                "米游币",
                "完成确认",
                TaskOutcome::Success,
                format!("复查后{coin_summary}"),
            )
            .with_task_id(TaskId::BbsCoins)
            .with_kind(RecordKind::Summary),
        );
        report.push(TaskRecord {
            account: "账号乙".to_owned(),
            ..record(TaskOutcome::AlreadyCompleted)
        });

        let text = report.render_text();
        assert_eq!(text.matches("- 账号甲").count(), 1);
        assert_eq!(text.matches("- 账号乙").count(), 1);
        assert!(text.contains("/ 国内游戏签到 / 原神 / ***1234："));
        assert!(text.contains("今日奖励：原石 ×20"));
        assert!(text.contains("/ 社区签到 / 原神："));
        assert!(text.contains(&coin_summary));
        assert!(!text.contains("/ 米游币 / 完成确认："));
    }

    #[test]
    fn text_report_aggregates_repeated_community_post_details() {
        let mut report = RunReport::default();
        for subject in ["帖子一", "帖子二"] {
            report.push(
                TaskRecord::new(
                    "账号甲",
                    "阅读",
                    subject,
                    TaskOutcome::Success,
                    "复查已确认",
                )
                .with_task_id(TaskId::BbsRead),
            );
        }

        let text = report.render_text();
        assert!(text.contains("/ 阅读：已完成 2 项；任务状态复查已确认"));
        assert!(!text.contains("帖子一"));
        assert!(!text.contains("帖子二"));

        let verbose = report.render_verbose_text();
        assert!(verbose.contains("帖子一"));
        assert!(verbose.contains("帖子二"));
    }

    #[test]
    fn already_completed_coin_summary_remains_visible() {
        let coin_summary = coin_summary_message(23, 27, 1357);
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "米游币",
                "任务状态",
                TaskOutcome::AlreadyCompleted,
                coin_summary.clone(),
            )
            .with_task_id(TaskId::BbsCoinStatus)
            .with_kind(RecordKind::Summary),
        );

        assert!(
            report
                .render_text()
                .contains(&format!("/ 米游币 / 任务状态：今日已完成；{coin_summary}"))
        );
    }

    #[test]
    fn summary_and_diagnostic_records_are_excluded_from_task_counts() {
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "国内游戏签到",
                "原神",
                TaskOutcome::Success,
                "签到成功",
            )
            .with_task_id(TaskId::ChinaCheckin),
        );
        // 汇总记录不应计入「成功」计数，也不应让「失败」计数变成负数。
        report.push(
            TaskRecord::new(
                "账号甲",
                "米游币",
                "完成确认",
                TaskOutcome::Success,
                "已领取 30",
            )
            .with_task_id(TaskId::BbsCoins)
            .with_kind(RecordKind::Summary),
        );
        // 诊断记录同样不计入任务计数。
        report.push(
            TaskRecord::new(
                "账号甲",
                "凭据刷新",
                "cookie_token",
                TaskOutcome::Success,
                "已刷新",
            )
            .with_task_id(TaskId::CredentialRefresh)
            .with_kind(RecordKind::Diagnostic),
        );

        let text = report.render_text();
        assert!(
            text.contains("运行完成：成功 1，今日已完成 0，跳过 0，失败 0"),
            "汇总与诊断记录不应计入任务计数，实际输出：\n{text}"
        );
    }

    #[test]
    fn remediation_prefers_stable_error_code() {
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "国内游戏签到",
                "原神",
                TaskOutcome::AuthenticationFailed,
                "自定义文案不再依赖关键字",
            )
            .with_task_id(TaskId::ChinaCheckin)
            .with_error_code("cookie_invalid"),
        );

        let text = report.render_text();
        assert!(text.contains("重新获取包含 UID、SToken 和 MID 的完整 Cookie"));
    }

    #[test]
    fn notification_failure_remediation_does_not_depend_on_chinese_task_name() {
        let mut report = RunReport::default();
        report.push(
            TaskRecord::new(
                "账号甲",
                "通知",
                "Webhook",
                TaskOutcome::Failed,
                "通知发送失败",
            )
            .with_task_id(TaskId::Notification)
            .with_kind(RecordKind::Diagnostic),
        );

        let text = report.render_text();
        assert!(text.contains("运行 `notification test` 单独检查通知渠道。"));
    }

    #[test]
    fn task_id_serializes_as_snake_case_for_every_variant() {
        let ids = [
            (TaskId::ChinaCheckin, "china_checkin"),
            (TaskId::HoyolabCheckin, "hoyolab_checkin"),
            (TaskId::BbsCheckin, "bbs_checkin"),
            (TaskId::BbsRead, "bbs_read"),
            (TaskId::BbsLike, "bbs_like"),
            (TaskId::BbsUnlike, "bbs_unlike"),
            (TaskId::BbsShare, "bbs_share"),
            (TaskId::BbsCoins, "bbs_coins"),
            (TaskId::BbsCoinStatus, "bbs_coin_status"),
            (TaskId::ChinaCloudGame, "china_cloud_game"),
            (TaskId::OverseasCloudGame, "overseas_cloud_game"),
            (TaskId::WebActivity, "web_activity"),
            (TaskId::CredentialRefresh, "credential_refresh"),
            (TaskId::Notification, "notification"),
            (TaskId::Config, "config"),
            (TaskId::BbsOther, "bbs_other"),
        ];
        for (task_id, expected) in ids {
            let json = serde_json::to_string(&task_id).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
        }
    }
}
