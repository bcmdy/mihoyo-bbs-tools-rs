//! 离线 CLI 回归测试。
//!
//! 这些测试直接启动真实二进制，验证 `version`、`print-example-config`、
//! `validate-config` 与 `run --config -` 的 stdout、退出码与只读契约。
//! 全部用例不访问网络、不写用户目录、不依赖桌面通知或计划任务安装。

use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

/// 编译期由 Cargo 注入的真实二进制路径，避免测试依赖当前工作目录或 PATH。
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_MihoyoBBSToolsRS")
}

/// 最小合法配置：仅一个被禁用的账号，且关闭全部任务与文件日志。
///
/// 该配置用于验证 `run` 的只读 JSON 契约，不包含任何真实凭据或网络目标。
const DISABLED_CONFIG: &str = r#"
version: 1

runtime:
  random_delay_seconds: 0
  logging:
    enabled: false

accounts:
  - name: "ci-smoke-disabled"
    enabled: false
    credentials:
      cookie: ""
      stoken: ""
    tasks:
      china_game_checkin: false
      hoyolab_checkin: false
      bbs: false
      china_cloud_game: false
      overseas_cloud_game: false
      web_activity:
        enabled: false
        activities: []
"#;

fn run(args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("启动 MihoyoBBSToolsRS 失败")
}

/// 以指定标准输入运行子命令；配置只从标准输入传入，不落盘。
fn run_with_stdin(args: &[&str], stdin: &str, current_dir: &Path) -> Output {
    let mut child = Command::new(binary())
        .args(args)
        .current_dir(current_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("启动 MihoyoBBSToolsRS 失败");
    {
        let mut pipe = child.stdin.take().expect("标准输入应可写");
        pipe.write_all(stdin.as_bytes())
            .expect("写入标准输入配置失败");
        pipe.flush().expect("刷新标准输入失败");
    }
    child.wait_with_output().expect("等待子进程失败")
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_reports_commit_and_target() {
    let output = run(&["version"]);
    assert!(output.status.success(), "version 应成功退出");

    let stdout = stdout_text(&output);
    assert!(
        stdout.contains("MihoyoBBSToolsRS"),
        "version 输出应包含程序名：{stdout}"
    );
    assert!(
        stdout.contains("commit"),
        "version 输出应包含提交字段：{stdout}"
    );
    assert!(
        stdout.contains("target"),
        "version 输出应包含目标平台字段：{stdout}"
    );
}

#[test]
fn print_example_config_outputs_template() {
    let output = run(&["print-example-config"]);
    assert!(output.status.success(), "print-example-config 应成功退出");

    let stdout = stdout_text(&output);
    assert!(stdout.contains("version: 1"), "示例配置应包含版本号");
    assert!(stdout.contains("accounts:"), "示例配置应包含账号节点");
    assert!(stdout.contains("notifications:"), "示例配置应包含通知节点");
}

#[test]
fn example_config_output_is_accepted_by_validate_config() {
    let output = run(&["print-example-config"]);
    assert!(output.status.success());
    let example = stdout_text(&output);

    let directory = tempfile::tempdir().expect("创建临时目录失败");
    let config = directory.path().join("example.yaml");
    std::fs::write(&config, &example).expect("写入示例配置失败");

    // 示例配置只有一个 ${MIHOYO_COOKIE_1} 占位符需要展开；虚构凭据不含真实账号。
    // 以临时目录为工作目录运行，避免示例配置默认开启的文件日志写进仓库。
    let output = Command::new(binary())
        .arg("validate-config")
        .arg("--config")
        .arg(&config)
        .env(
            "MIHOYO_COOKIE_1",
            "account_id=123; account_mid_v2=mid; stoken=v2_example",
        )
        .current_dir(directory.path())
        .stdin(Stdio::null())
        .output()
        .expect("启动 validate-config 失败");

    assert!(
        output.status.success(),
        "示例配置应通过校验：{}",
        stderr_text(&output)
    );
    assert!(
        stdout_text(&output).contains("配置有效"),
        "validate-config 应输出校验结果"
    );
}

#[test]
fn validate_config_accepts_disabled_account_config() {
    let directory = tempfile::tempdir().expect("创建临时目录失败");
    let config = directory.path().join("disabled.yaml");
    std::fs::write(&config, DISABLED_CONFIG).expect("写入配置失败");

    let output = Command::new(binary())
        .arg("validate-config")
        .arg("--config")
        .arg(&config)
        .stdin(Stdio::null())
        .output()
        .expect("启动 validate-config 失败");

    assert!(
        output.status.success(),
        "禁用全部任务的配置应通过校验：{}",
        stderr_text(&output)
    );
}

#[test]
fn validate_config_rejects_invalid_config_with_exit_code_2() {
    let directory = tempfile::tempdir().expect("创建临时目录失败");

    // 结构合法但校验失败：accounts 为空，对应 AppError::Config 的退出码 2。
    let empty_accounts = directory.path().join("empty-accounts.yaml");
    std::fs::write(&empty_accounts, "version: 1\naccounts: []\n").expect("写入配置失败");
    let output = Command::new(binary())
        .arg("validate-config")
        .arg("--config")
        .arg(&empty_accounts)
        .stdin(Stdio::null())
        .output()
        .expect("启动 validate-config 失败");
    assert_eq!(
        output.status.code(),
        Some(2),
        "校验失败应返回退出码 2：{}",
        stderr_text(&output)
    );

    // YAML 语法错误同样归为配置错误。
    let malformed = directory.path().join("malformed.yaml");
    std::fs::write(&malformed, "version: 1\naccounts: [\n").expect("写入配置失败");
    let output = Command::new(binary())
        .arg("validate-config")
        .arg("--config")
        .arg(&malformed)
        .stdin(Stdio::null())
        .output()
        .expect("启动 validate-config 失败");
    assert_eq!(
        output.status.code(),
        Some(2),
        "YAML 解析失败应返回退出码 2：{}",
        stderr_text(&output)
    );
}

#[test]
fn run_reads_config_from_stdin_and_emits_json_schema() {
    let directory = tempfile::tempdir().expect("创建临时目录失败");
    let output = run_with_stdin(
        &[
            "run",
            "--config",
            "-",
            "--read-only",
            "--no-notify",
            "--output",
            "json",
        ],
        DISABLED_CONFIG,
        directory.path(),
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "禁用全部任务时 run 应返回退出码 0：{}",
        stderr_text(&output)
    );

    let stdout = stdout_text(&output);
    let json: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("run --output json 应输出单个 JSON 对象");

    let object = json.as_object().expect("JSON 顶层应为对象");
    for key in ["schema_version", "exit_code", "records", "notifications"] {
        assert!(
            object.contains_key(key),
            "JSON 顶层缺少字段 {key}：{stdout}"
        );
    }
    assert_eq!(object["exit_code"], serde_json::json!(0));
    assert!(
        object["records"]
            .as_array()
            .expect("records 应为数组")
            .is_empty(),
        "禁用全部任务时不应产生任务记录"
    );

    // 从标准输入读取时始终只读：临时目录不应出现配置副本、日志或凭据文件。
    let leftovers: Vec<_> = std::fs::read_dir(directory.path())
        .expect("读取临时目录失败")
        .map(|entry| entry.expect("读取目录项失败").file_name())
        .collect();
    assert!(
        leftovers.is_empty(),
        "只读运行不应写入任何文件，实际残留：{leftovers:?}"
    );
}
