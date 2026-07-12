use std::fmt::Write;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Success,
    AlreadyCompleted,
    Skipped,
    Failed,
    AuthenticationFailed,
    CaptchaRequired,
    NetworkFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRecord {
    pub account: String,
    pub task: String,
    pub subject: String,
    pub outcome: TaskOutcome,
    pub message: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunReport {
    pub records: Vec<TaskRecord>,
}

impl RunReport {
    pub fn push(&mut self, record: TaskRecord) {
        self.records.push(record);
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
        for record in &self.records {
            let _ = writeln!(
                output,
                "[{:?}] {} / {} / {}：{}",
                record.outcome, record.account, record.task, record.subject, record.message
            );
        }
        output
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
}
