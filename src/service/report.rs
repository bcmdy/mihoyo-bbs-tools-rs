use std::fmt::Write;

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskOutcome {
    Success,
    AlreadyCompleted,
    Skipped,
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
        } else if self
            .records
            .iter()
            .any(|record| record.outcome == TaskOutcome::Failed)
        {
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

        let problems = self
            .records
            .iter()
            .filter(|record| is_problem(record.outcome))
            .collect::<Vec<_>>();
        if problems.is_empty() {
            let _ = writeln!(output, "\n本次没有需要处理的问题。");
        } else {
            let _ = writeln!(output, "\n需要处理：");
            for record in problems {
                let _ = writeln!(
                    output,
                    "[{}] {} / {} / {}：{}",
                    outcome_name(record.outcome),
                    record.account,
                    record.task,
                    record.subject,
                    record.message
                );
                let _ = writeln!(output, "处理方式：{}", remediation(record));
            }
        }
        let _ = writeln!(output, "\n详细成功项可使用 `run --verbose` 查看。");
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
}

pub const fn outcome_name(outcome: TaskOutcome) -> &'static str {
    match outcome {
        TaskOutcome::Success => "成功",
        TaskOutcome::AlreadyCompleted => "今日已完成",
        TaskOutcome::Skipped => "已跳过",
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
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains(r#""outcome":"already_completed""#));
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
}
