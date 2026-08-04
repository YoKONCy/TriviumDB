use std::path::Path;

pub(crate) fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Ok(());
    };

    #[cfg(unix)]
    {
        std::fs::File::open(parent)?.sync_all()
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(parent)?;
        match directory.sync_all() {
            Ok(()) => Ok(()),
            // Windows 上对目录句柄调用 FlushFileBuffers 可能返回
            // InvalidInput / Unsupported / PermissionDenied，均视为不支持而静默跳过
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::InvalidInput
                        | std::io::ErrorKind::Unsupported
                        | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        Ok(())
    }
}

/// 稳健重命名并同步父目录：rename 后对父目录执行 sync_all，
/// 确保目录元数据（dentry）落盘，防止断电后出现新旧文件并存。
///
/// - Unix：`fsync` 父目录
/// - Windows：`FlushFileBuffers`（通过 `sync_all`），不支持时静默跳过
pub(crate) fn robust_rename_and_sync(from: &Path, to: &Path) -> std::io::Result<()> {
    robust_rename(from, to)?;
    sync_parent_directory(to)
}

/// 稳健重命名：Windows 上对杀毒软件瞬态锁定进行指数退避重试
pub(crate) fn robust_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }

    #[cfg(windows)]
    {
        let max_retries = 10;
        let mut delay = std::time::Duration::from_millis(1);
        for attempt in 0..max_retries {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(error) if attempt < max_retries - 1 => {
                    let os_error = error.raw_os_error();
                    if os_error == Some(5) || os_error == Some(32) {
                        tracing::debug!(
                            "原子重命名第 {} 次失败，系统错误 {:?}，将在 {:?} 后重试",
                            attempt + 1,
                            os_error,
                            delay
                        );
                        std::thread::sleep(delay);
                        delay = (delay * 2).min(std::time::Duration::from_millis(50));
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::other("原子重命名重试次数耗尽"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 回归测试：robust_rename_and_sync 正常重命名并同步父目录
    #[test]
    fn robust_rename_and_sync_正常重命名并同步父目录() {
        let dir = std::env::temp_dir().join("triviumdb_fs_test_basic");
        std::fs::create_dir_all(&dir).ok();
        let from = dir.join("from.tmp");
        let to = dir.join("to.txt");
        std::fs::write(&from, b"hello").unwrap();

        robust_rename_and_sync(&from, &to).expect("重命名并同步应成功");

        assert!(!from.exists(), "源文件应已重命名");
        assert!(to.exists(), "目标文件应存在");
        assert_eq!(std::fs::read(&to).unwrap(), b"hello", "内容应保持一致");

        std::fs::remove_file(&to).ok();
        std::fs::remove_dir(&dir).ok();
    }

    /// 回归测试：目标文件已存在时，robust_rename_and_sync 应原子覆盖
    #[test]
    fn robust_rename_and_sync_目标已存在时覆盖() {
        let dir = std::env::temp_dir().join("triviumdb_fs_test_overwrite");
        std::fs::create_dir_all(&dir).ok();
        let from = dir.join("from.tmp");
        let to = dir.join("to.txt");
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"old").unwrap();

        robust_rename_and_sync(&from, &to).expect("覆盖重命名应成功");

        assert!(!from.exists(), "源文件应已重命名");
        assert_eq!(
            std::fs::read(&to).unwrap(),
            b"new",
            "目标文件应被覆盖为新内容"
        );

        std::fs::remove_file(&to).ok();
        std::fs::remove_dir(&dir).ok();
    }
}
