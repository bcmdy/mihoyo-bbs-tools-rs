//! 配置文件的统一持久化：内容校验、文件同步、原子替换与并发冲突保护。
//!
//! 设计目标：**目标路径在任何时刻都能读到完整的旧配置或完整的新配置**。
//! 为此替换必须是单次原子重命名，不能像旧实现那样先 `rename(目标 → 瞬时备份)`
//! 再 `rename(临时文件 → 目标)`，后者在两次重命名之间会留下目标缺失的窗口。
//!
//! 具体步骤：
//!
//! 1. 在目标同目录写临时文件（`open_secure_new` 保持既有权限逻辑）；
//! 2. 由调用方提供校验回调，校验失败立即删除临时文件，目标保持原样；
//! 3. `sync_all` 临时文件，确保数据落盘后再替换；
//! 4. 可选的受控时间戳备份（`create_backup`）；
//! 5. 比较目标当前内容与预期基线，不一致则保留 `*.conflict.<pid>.yaml` 并报冲突；
//! 6. 原子替换（Unix 用 `rename(2)`，Windows 用 `MoveFileExW(MOVEFILE_REPLACE_EXISTING
//!    | MOVEFILE_WRITE_THROUGH)`）；
//! 7. 同步父目录（Unix 用 `File::open(dir)` + `sync_all`；Windows 由
//!    `MOVEFILE_WRITE_THROUGH` 保证落盘）。
//!
//! 并发控制：进程内按规范化路径加 `Mutex` 互斥，跨进程用步骤 5 的版本比较。
//! 比较与替换处于同一把锁内，因此不会出现「检查通过后又被覆盖」的窗口。
//!
//! 测试可通过 [`FileOps`] 注入失败，覆盖写入、同步、替换各阶段的异常路径。

use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use super::{ConfigError, create_backup, open_secure_new};

/// 可注入的文件系统原语，便于在单元测试中模拟各阶段失败。
///
/// 生产实现是 [`RealFileOps`]；测试实现只覆盖必要的失败注入，其余转发到真实文件系统。
pub(crate) trait FileOps {
    /// 写临时文件（`create_new` 语义，Unix 上保持 0o600）。
    fn write_new(&self, path: &Path, content: &[u8]) -> std::io::Result<()>;
    /// 把文件内容刷入持久存储。
    fn sync_file(&self, path: &Path) -> std::io::Result<()>;
    /// 原子替换：`source` 覆盖 `target`。
    fn replace(&self, source: &Path, target: &Path) -> std::io::Result<()>;
    /// 同步目录项，使重命名结果持久化。
    fn sync_directory(&self, directory: &Path) -> std::io::Result<()>;
    /// 删除文件（尽力而为，调用方忽略错误时用于清理）。
    fn remove(&self, path: &Path) -> std::io::Result<()>;
}

/// 生产用文件系统实现。
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RealFileOps;

impl FileOps for RealFileOps {
    fn write_new(&self, path: &Path, content: &[u8]) -> std::io::Result<()> {
        let mut file = open_secure_new(path)?;
        file.write_all(content)?;
        file.flush()
    }

    fn sync_file(&self, path: &Path) -> std::io::Result<()> {
        // Windows 上只读句柄无法调用 FlushFileBuffers，因此必须以写权限打开。
        fs::OpenOptions::new().write(true).open(path)?.sync_all()
    }

    fn replace(&self, source: &Path, target: &Path) -> std::io::Result<()> {
        atomic_replace(source, target)
    }

    fn sync_directory(&self, directory: &Path) -> std::io::Result<()> {
        sync_directory(directory)
    }

    fn remove(&self, path: &Path) -> std::io::Result<()> {
        fs::remove_file(path)
    }
}

/// `source` 原子覆盖 `target`。
///
/// Windows 的 `std::fs::rename` 不能覆盖已存在目标，因此必须走平台原语；
/// Unix 的 `rename(2)` 本身就是原子替换，直接使用 `std::fs::rename`。
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        windows::move_file_replace(source, target)
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, target)
    }
}

/// 同步目录项。Unix 上通过打开目录并 `sync_all` 完成；Windows 上尽力而为。
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        fs::File::open(directory)?.sync_all()
    }
    #[cfg(windows)]
    {
        // Windows 的 `MoveFileExW(MOVEFILE_WRITE_THROUGH)` 已在返回前完成落盘，
        // 额外刷新目录项需要 `CreateFileW`（引入 Win32_Security 特性），收益有限，故跳过。
        let _ = directory;
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = directory;
        Ok(())
    }
}

/// 一次原子替换的参数。
pub(crate) struct ReplaceRequest<'a> {
    /// 目标配置文件路径。
    pub(crate) target: &'a Path,
    /// 待写入的新内容。
    pub(crate) content: &'a str,
    /// 预期基线：目标当前内容必须与之一致，否则视为并发冲突。
    ///
    /// `None` 表示不比较（仅在目标尚未存在或调用方已持有独占上下文时使用）。
    pub(crate) expected: Option<&'a str>,
    /// 替换前是否生成受控时间戳备份。
    pub(crate) create_backup: bool,
    /// 备份保留数量。
    pub(crate) backup_keep: usize,
}

/// 按顺序执行「写临时文件 → 校验 → 同步 → 备份 → 冲突检查 → 原子替换 → 同步目录」。
///
/// `validate` 在临时文件写完、任何真实配置被触碰之前调用，返回错误时目标保持原样。
pub(crate) fn replace_atomically(
    ops: &dyn FileOps,
    request: &ReplaceRequest<'_>,
    validate: impl FnOnce(&Path) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let target = request.target;
    let temporary = temporary_path(target);
    // 上一次异常退出的残留可能占用同名临时文件，先尽力清理。
    let _ = ops.remove(&temporary);
    ops.write_new(&temporary, request.content.as_bytes())
        .map_err(|source| write_error(target, source))?;

    let validate_result = validate(&temporary);
    if let Err(error) = validate_result {
        let _ = ops.remove(&temporary);
        return Err(error);
    }

    if let Err(source) = ops.sync_file(&temporary) {
        let _ = ops.remove(&temporary);
        return Err(write_error(&temporary, source));
    }

    if request.create_backup
        && let Err(error) = create_backup(target, request.backup_keep)
    {
        let _ = ops.remove(&temporary);
        return Err(error);
    }

    let expected = match request.expected {
        Some(expected) => expected,
        None => {
            // 无基线要求：直接替换，但仍需在同一把路径锁内完成（由调用方保证）。
            let replaced = ops.replace(&temporary, target);
            return finish_replace(ops, replaced, target, &temporary);
        }
    };

    let current = match fs::read_to_string(target) {
        Ok(current) => current,
        Err(source) => {
            let _ = ops.remove(&temporary);
            return Err(ConfigError::Read {
                path: target.to_path_buf(),
                source,
            });
        }
    };
    if current != expected {
        let conflict = conflict_path(target);
        let preserve = ops.write_new(&conflict, request.content.as_bytes());
        let _ = ops.remove(&temporary);
        preserve.map_err(|source| write_error(&conflict, source))?;
        return Err(ConfigError::Conflict {
            path: target.to_path_buf(),
            preserved: conflict,
        });
    }

    let replaced = ops.replace(&temporary, target);
    finish_replace(ops, replaced, target, &temporary)
}

/// 替换完成后的收尾：失败时清理临时文件，成功时同步父目录。
fn finish_replace(
    ops: &dyn FileOps,
    replaced: std::io::Result<()>,
    target: &Path,
    temporary: &Path,
) -> Result<(), ConfigError> {
    if let Err(source) = replaced {
        let _ = ops.remove(temporary);
        return Err(write_error(target, source));
    }
    // 目录同步失败不影响配置已经生效这一事实，但要记 warn 提示持久性风险。
    if let Some(directory) = target.parent().filter(|path| !path.as_os_str().is_empty())
        && let Err(error) = ops.sync_directory(directory)
    {
        tracing::warn!(
            directory = %directory.display(),
            error = %error,
            "配置已替换，但目录同步失败；异常掉电后重命名结果可能丢失"
        );
    }
    Ok(())
}

/// 进程内的路径锁表，保证同一配置路径的「比较 + 替换」不会交错。
fn path_lock(path: &Path) -> std::sync::Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, std::sync::Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let key = canonical_key(path);
    // 锁表本身只在短暂查找期间持有；即使中毒也可以继续使用内部数据，故直接回退。
    let mut locks = match locks.lock() {
        Ok(locks) => locks,
        Err(poisoned) => poisoned.into_inner(),
    };
    locks
        .entry(key)
        .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
        .clone()
}

fn canonical_key(path: &Path) -> PathBuf {
    match fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(_) => match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => match fs::canonicalize(parent) {
                Ok(parent) => parent.join(name),
                Err(_) => path.to_path_buf(),
            },
            _ => path.to_path_buf(),
        },
    }
}

/// 在路径锁内执行 `action`，保证同一配置路径的读写不交错。
pub(crate) fn with_path_lock<T>(
    path: &Path,
    action: impl FnOnce() -> Result<T, ConfigError>,
) -> Result<T, ConfigError> {
    let lock = path_lock(path);
    let guard = match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let result = action();
    drop(guard);
    result
}

/// 冲突时保留的待保存副本路径：`{stem}.conflict.{pid}.yaml`。
fn conflict_path(target: &Path) -> PathBuf {
    let stem = target
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("config");
    let directory = target.parent().unwrap_or_else(|| Path::new("."));
    let mut candidate = directory.join(format!("{stem}.conflict.{}.yaml", std::process::id()));
    let mut suffix = 1;
    while candidate.exists() {
        candidate = directory.join(format!(
            "{stem}.conflict.{}.{suffix}.yaml",
            std::process::id()
        ));
        suffix += 1;
    }
    candidate
}

/// 替换过程的临时文件路径：`{stem}.yaml.{pid}.editing`。
///
/// 与 `editor.rs` 的 `temporary_path` 共享同一命名规则，便于启动兜底清理识别。
fn temporary_path(target: &Path) -> PathBuf {
    target.with_extension(format!("yaml.{}.editing", std::process::id()))
}

fn write_error(path: &Path, source: std::io::Error) -> ConfigError {
    ConfigError::Write {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        OsStr::new(path.as_os_str())
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// 用 `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` 原子覆盖目标。
    ///
    /// `MOVEFILE_WRITE_THROUGH` 保证函数返回前重命名结果已落盘，
    /// 因此 Windows 侧不再单独刷新父目录。
    pub(super) fn move_file_replace(source: &Path, target: &Path) -> std::io::Result<()> {
        let source = wide(source);
        let target = wide(target);
        // SAFETY: 两个指针都指向以 NUL 结尾的宽字符串，且在本调用期间一直有效。
        let replaced = unsafe {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, io::Error};

    use super::*;

    /// 记录调用顺序，并按配置注入失败。
    ///
    /// 内部计数用 `Cell`，因此模拟器本身不满足 `Sync`；每次测试在单线程内使用即可，
    /// 需要跨线程时会显式用 `Mutex` 包装。
    #[derive(Default)]
    struct FaultyOps {
        inner: RealFileOps,
        fail_write: bool,
        fail_sync: bool,
        fail_replace: bool,
        fail_directory: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl FaultyOps {
        fn record(&self, name: &'static str) {
            self.calls.borrow_mut().push(name);
        }
    }

    impl FileOps for FaultyOps {
        fn write_new(&self, path: &Path, content: &[u8]) -> std::io::Result<()> {
            self.record("write");
            if self.fail_write {
                return Err(Error::other("注入：写入失败"));
            }
            self.inner.write_new(path, content)
        }

        fn sync_file(&self, path: &Path) -> std::io::Result<()> {
            self.record("sync");
            if self.fail_sync {
                return Err(Error::other("注入：同步失败"));
            }
            self.inner.sync_file(path)
        }

        fn replace(&self, source: &Path, target: &Path) -> std::io::Result<()> {
            self.record("replace");
            if self.fail_replace {
                return Err(Error::other("注入：替换失败"));
            }
            self.inner.replace(source, target)
        }

        fn sync_directory(&self, directory: &Path) -> std::io::Result<()> {
            self.record("sync_directory");
            if self.fail_directory {
                return Err(Error::other("注入：目录同步失败"));
            }
            self.inner.sync_directory(directory)
        }

        fn remove(&self, path: &Path) -> std::io::Result<()> {
            self.inner.remove(path)
        }
    }

    fn request<'a>(
        target: &'a Path,
        content: &'a str,
        expected: Option<&'a str>,
    ) -> ReplaceRequest<'a> {
        ReplaceRequest {
            target,
            content,
            expected,
            create_backup: false,
            backup_keep: 5,
        }
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn replaces_target_atomically_and_syncs_directory() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");

        let ops = FaultyOps::default();
        replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 新内容\n", Some("version: 1\n")),
            |staged| {
                assert_eq!(
                    fs::read_to_string(staged).unwrap(),
                    "version: 1\n# 新内容\n"
                );
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "version: 1\n# 新内容\n"
        );
        assert_eq!(
            ops.calls.borrow().clone(),
            vec!["write", "sync", "replace", "sync_directory"]
        );
        // 临时文件必须清理干净。
        assert!(!temporary_path(&target).exists());
    }

    #[test]
    fn validation_failure_keeps_target_untouched() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");
        let ops = FaultyOps::default();

        let error = replace_atomically(
            &ops,
            &request(&target, "invalid: content\n", Some("version: 1\n")),
            |_| Err(ConfigError::Edit("校验失败".to_owned())),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Edit(_)));
        assert_eq!(fs::read_to_string(&target).unwrap(), "version: 1\n");
        assert!(!temporary_path(&target).exists());
        // 校验失败时不应该继续同步、替换或生成备份。
        assert_eq!(ops.calls.borrow().clone(), vec!["write"]);
    }

    #[test]
    fn replace_failure_keeps_old_content_readable() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");
        let ops = FaultyOps {
            fail_replace: true,
            ..Default::default()
        };

        let error = replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 新内容\n", Some("version: 1\n")),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Write { .. }));
        // 关键断言：替换失败后目标仍可读到完整的旧配置。
        assert_eq!(fs::read_to_string(&target).unwrap(), "version: 1\n");
        assert!(!temporary_path(&target).exists());
    }

    #[test]
    fn sync_failure_does_not_touch_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");
        let ops = FaultyOps {
            fail_sync: true,
            ..Default::default()
        };

        let error = replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 新内容\n", Some("version: 1\n")),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Write { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "version: 1\n");
        assert!(!temporary_path(&target).exists());
    }

    #[test]
    fn write_failure_reports_target_error() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");
        let ops = FaultyOps {
            fail_write: true,
            ..Default::default()
        };

        let error = replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 新内容\n", Some("version: 1\n")),
            |_| Ok(()),
        )
        .unwrap_err();

        assert!(matches!(error, ConfigError::Write { .. }));
        assert_eq!(fs::read_to_string(&target).unwrap(), "version: 1\n");
    }

    #[test]
    fn baseline_mismatch_preserves_conflict_copy_and_keeps_target() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n# 其它进程写入\n");
        let ops = FaultyOps::default();

        let error = replace_atomically(
            &ops,
            &request(
                &target,
                "version: 1\n# 本次会话内容\n",
                Some("version: 1\n"),
            ),
            |_| Ok(()),
        )
        .unwrap_err();

        let ConfigError::Conflict { path, preserved } = error else {
            panic!("应报告并发冲突");
        };
        assert_eq!(path, target);
        // 不覆盖其它进程的写入。
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "version: 1\n# 其它进程写入\n"
        );
        // 待保存内容被保留，且不含其它进程的内容。
        assert_eq!(
            fs::read_to_string(&preserved).unwrap(),
            "version: 1\n# 本次会话内容\n"
        );
        assert!(
            preserved
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("conflict")
        );
        assert!(!temporary_path(&target).exists());
        // 冲突时不应执行替换。
        assert!(!ops.calls.borrow().contains(&"replace"));
    }

    #[test]
    fn directory_sync_failure_is_not_fatal() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n");
        let ops = FaultyOps {
            fail_directory: true,
            ..Default::default()
        };

        replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 新内容\n", Some("version: 1\n")),
            |_| Ok(()),
        )
        .unwrap();

        // 新配置已经生效；目录同步失败只记 warn。
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "version: 1\n# 新内容\n"
        );
        assert!(ops.calls.borrow().contains(&"sync_directory"));
    }

    #[test]
    fn missing_target_without_baseline_creates_new_file() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("nested/config.yaml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let ops = FaultyOps::default();

        replace_atomically(
            &ops,
            &request(&target, "version: 1\n# 首次写入\n", None),
            |_| Ok(()),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "version: 1\n# 首次写入\n"
        );
    }

    #[test]
    fn path_lock_serialises_same_path_and_isolates_different_paths() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("config.yaml");
        let second = temp.path().join("other.yaml");
        write_file(&first, "version: 1\n");
        write_file(&second, "version: 1\n");

        let order = Mutex::new(Vec::new());
        with_path_lock(&first, || {
            order.lock().unwrap().push("first-outer");
            // 同一路径嵌套获取会死锁，这里只验证不同路径可以并行获得各自的锁。
            with_path_lock(&second, || {
                order.lock().unwrap().push("second-inner");
                Ok(())
            })?;
            order.lock().unwrap().push("first-outer-done");
            Ok(())
        })
        .unwrap();

        assert_eq!(
            order.lock().unwrap().clone(),
            vec!["first-outer", "second-inner", "first-outer-done"]
        );
    }

    #[test]
    fn conflict_copy_names_do_not_overwrite_previous_ones() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("config.yaml");
        write_file(&target, "version: 1\n# 进程 A\n");

        let first = conflict_path(&target);
        write_file(&first, "version: 1\n# 第一次冲突\n");
        let second = conflict_path(&target);
        assert_ne!(first, second, "冲突副本不能相互覆盖");
        assert!(second.to_string_lossy().ends_with(".yaml"));
    }
}
