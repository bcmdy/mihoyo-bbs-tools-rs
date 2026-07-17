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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskRecord {
    pub account: String,
    pub task: String,
    pub subject: String,
    pub outcome: TaskOutcome,
    pub message: String,
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
        let success = self.count(TaskOutcome::Success);
        let completed = self.count(TaskOutcome::AlreadyCompleted);
        let skipped = self.count(TaskOutcome::Skipped);
        let failed = self.records.len() - success - completed - skipped;
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

    fn count(&self, outcome: TaskOutcome) -> usize {
        self.records
            .iter()
            .filter(|record| record.outcome == outcome)
            .count()
    }

    fn write_successes(&self, output: &mut String) {
        let records = self
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.outcome,
                    TaskOutcome::Success | TaskOutcome::AlreadyCompleted
                ) && !is_hidden_summary_record(record)
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
                .find(|record| record.account == account && is_coin_summary_record(record));
            let mut coin_summary_written = false;
            for record in records.iter().copied().filter(|record| {
                record.account == account
                    && !is_repeated_community_detail(record)
                    && !is_coin_summary_record(record)
            }) {
                if record.task == "社区签到"
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
            for task in ["阅读", "点赞", "取消点赞", "分享"] {
                let count = records
                    .iter()
                    .filter(|record| {
                        record.account == account
                            && record.task == task
                            && is_repeated_community_detail(record)
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

fn account_order<'a>(records: &[&'a TaskRecord]) -> Vec<&'a str> {
    let mut accounts = Vec::new();
    for record in records {
        if !accounts.contains(&record.account.as_str()) {
            accounts.push(record.account.as_str());
        }
    }
    accounts
}

fn is_hidden_summary_record(record: &TaskRecord) -> bool {
    record.task == "米游币"
        && record.subject == "任务状态"
        && record.outcome == TaskOutcome::Success
}

fn is_coin_summary_record(record: &TaskRecord) -> bool {
    record.task == "米游币" && matches!(record.subject.as_str(), "任务状态" | "完成确认")
}

fn is_repeated_community_detail(record: &TaskRecord) -> bool {
    matches!(record.task.as_str(), "阅读" | "点赞" | "取消点赞" | "分享")
        && record.subject != "米游币任务"
}

fn write_success_record(output: &mut String, record: &TaskRecord) {
    let completed_prefix =
        if is_coin_summary_record(record) && record.outcome == TaskOutcome::AlreadyCompleted {
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
    let message = record.message.to_ascii_lowercase();
    if message.contains("stoken") || message.contains("mid") {
        "重新获取包含 UID、SToken 和 MID 的完整 Cookie，然后在 `config setup` 中更新账号 Cookie。"
    } else {
        match record.outcome {
            TaskOutcome::AuthenticationFailed => {
                "Cookie 已失效；运行 `config setup`，选择“账号 -> 更新 Cookie”。"
            }
            TaskOutcome::CaptchaRequired => "运行 `config setup` 检查验证码端点，或稍后重试。",
            TaskOutcome::NetworkFailed => {
                "运行 `doctor --online` 检查网络、TLS 和该账号的代理设置。"
            }
            TaskOutcome::StateSyncTimeout => {
                "稍后重新运行；若持续出现，请检查米游社任务状态和账号网络。"
            }
            TaskOutcome::Failed if record.task.contains("通知") => {
                "运行 `notification test` 单独检查通知渠道。"
            }
            TaskOutcome::Failed => "查看上方错误详情；可运行 `doctor --online` 继续诊断。",
            _ => "无需处理。",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(outcome: TaskOutcome) -> TaskRecord {
        TaskRecord {
            account: "example".to_owned(),
            task: "签到".to_owned(),
            subject: "原神".to_owned(),
            outcome,
            message: "安全消息".to_owned(),
        }
    }

    fn coin_summary_message(received: u32, remaining: u32, total: u32) -> String {
        format!("已领取 {received}，还可领取 {remaining}，当前共 {total} 米游币")
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
        report.push(TaskRecord {
            account: "账号甲".to_owned(),
            task: "国内游戏签到".to_owned(),
            subject: "原神 / ***1234".to_owned(),
            outcome: TaskOutcome::Success,
            message: "签到成功；今日奖励：原石 ×20；累计签到 7 天".to_owned(),
        });
        report.push(TaskRecord {
            account: "账号甲".to_owned(),
            task: "社区签到".to_owned(),
            subject: "原神".to_owned(),
            outcome: TaskOutcome::Success,
            message: "第 1 轮复查确认社区签到米游币领取".to_owned(),
        });
        report.push(TaskRecord {
            account: "账号甲".to_owned(),
            task: "米游币".to_owned(),
            subject: "完成确认".to_owned(),
            outcome: TaskOutcome::Success,
            message: format!("复查后{coin_summary}"),
        });
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
            report.push(TaskRecord {
                account: "账号甲".to_owned(),
                task: "阅读".to_owned(),
                subject: subject.to_owned(),
                outcome: TaskOutcome::Success,
                message: "复查已确认".to_owned(),
            });
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
        report.push(TaskRecord {
            account: "账号甲".to_owned(),
            task: "米游币".to_owned(),
            subject: "任务状态".to_owned(),
            outcome: TaskOutcome::AlreadyCompleted,
            message: coin_summary.clone(),
        });

        assert!(report.render_text().contains(&format!(
            "/ 米游币 / 任务状态：今日已完成；{coin_summary}"
        )));
    }
}
