use std::{env, ffi::OsString, path::PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QinglongSettings {
    pub multi: bool,
    pub directory: PathBuf,
    pub prefix: Option<String>,
    pub single_config: PathBuf,
    pub project_notifications: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum QinglongError {
    #[error("环境变量 {0} 不是有效的 Unicode 文本")]
    InvalidUnicode(&'static str),
    #[error("AutoMihoyoBBS_config_multi 只支持 0 或 1")]
    InvalidMultiMode,
    #[error("AutoMihoyoBBS_config_path 不能为空")]
    EmptyConfigPath,
    #[error("AutoMihoyoBBS_config_prefix 只能是文件名前缀，不能包含路径或控制字符")]
    InvalidPrefix,
}

pub fn qinglong_settings() -> Result<QinglongSettings, QinglongError> {
    resolve_with(env::var_os)
}

fn resolve_with(
    mut lookup: impl FnMut(&str) -> Option<OsString>,
) -> Result<QinglongSettings, QinglongError> {
    let directory = lookup("AutoMihoyoBBS_config_path")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config"));
    if directory.as_os_str().is_empty() {
        return Err(QinglongError::EmptyConfigPath);
    }
    let raw_prefix = lookup("AutoMihoyoBBS_config_prefix");
    let prefix_was_set = raw_prefix.is_some();
    let raw_prefix = raw_prefix
        .map(|value| {
            value
                .into_string()
                .map_err(|_| QinglongError::InvalidUnicode("AutoMihoyoBBS_config_prefix"))
        })
        .transpose()?;
    if raw_prefix.as_ref().is_some_and(|prefix| {
        prefix.contains('/') || prefix.contains('\\') || prefix.chars().any(char::is_control)
    }) {
        return Err(QinglongError::InvalidPrefix);
    }
    let multi = match lookup("AutoMihoyoBBS_config_multi") {
        None => false,
        Some(value) if value == "0" => false,
        Some(value) if value == "1" => true,
        Some(_) => return Err(QinglongError::InvalidMultiMode),
    };
    let qinglong_detected = lookup("QL_DIR").is_some();
    let prefix = if multi && qinglong_detected && !prefix_was_set {
        Some("mhy_".to_owned())
    } else {
        raw_prefix.clone().filter(|value| !value.is_empty())
    };
    let single_name = format!("{}config.yaml", raw_prefix.unwrap_or_default());
    let project_notifications =
        lookup("AutoMihoyoBBS_push_project").is_some_and(|value| value == "1");
    Ok(QinglongSettings {
        multi,
        single_config: directory.join(single_name),
        directory,
        prefix,
        project_notifications,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn resolve(values: &[(&str, &str)]) -> Result<QinglongSettings, QinglongError> {
        let values = values
            .iter()
            .map(|(key, value)| ((*key).to_owned(), OsString::from(value)))
            .collect::<HashMap<_, _>>();
        resolve_with(|name| values.get(name).cloned())
    }

    #[test]
    fn defaults_to_single_config() {
        let settings = resolve(&[]).unwrap();
        assert!(!settings.multi);
        assert_eq!(settings.directory, PathBuf::from("config"));
        assert_eq!(settings.single_config, PathBuf::from("config/config.yaml"));
        assert_eq!(settings.prefix, None);
        assert!(!settings.project_notifications);
    }

    #[test]
    fn qinglong_multi_defaults_to_mhy_prefix_only_when_prefix_is_absent() {
        let settings = resolve(&[("AutoMihoyoBBS_config_multi", "1"), ("QL_DIR", "/ql")]).unwrap();
        assert!(settings.multi);
        assert_eq!(settings.prefix.as_deref(), Some("mhy_"));

        let explicit_empty = resolve(&[
            ("AutoMihoyoBBS_config_multi", "1"),
            ("AutoMihoyoBBS_config_prefix", ""),
            ("QL_DIR", "/ql"),
        ])
        .unwrap();
        assert_eq!(explicit_empty.prefix, None);
    }

    #[test]
    fn custom_path_prefix_and_project_notifications_are_preserved() {
        let settings = resolve(&[
            ("AutoMihoyoBBS_config_path", "custom-configs"),
            ("AutoMihoyoBBS_config_prefix", "daily_"),
            ("AutoMihoyoBBS_push_project", "1"),
        ])
        .unwrap();
        assert_eq!(settings.directory, PathBuf::from("custom-configs"));
        assert_eq!(
            settings.single_config,
            PathBuf::from("custom-configs/daily_config.yaml")
        );
        assert_eq!(settings.prefix.as_deref(), Some("daily_"));
        assert!(settings.project_notifications);
    }

    #[test]
    fn invalid_multi_mode_is_rejected() {
        assert_eq!(
            resolve(&[("AutoMihoyoBBS_config_multi", "true")]),
            Err(QinglongError::InvalidMultiMode)
        );
    }

    #[test]
    fn empty_path_and_path_like_prefix_are_rejected() {
        assert_eq!(
            resolve(&[("AutoMihoyoBBS_config_path", "")]),
            Err(QinglongError::EmptyConfigPath)
        );
        assert_eq!(
            resolve(&[("AutoMihoyoBBS_config_prefix", "nested/")]),
            Err(QinglongError::InvalidPrefix)
        );
    }
}
