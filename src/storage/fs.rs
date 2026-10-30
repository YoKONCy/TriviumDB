//! 跨平台原子替换与目录同步辅助函数。
//!
//! Windows 下仅对共享冲突和访问拒绝执行有界退避，其他错误立即返回；Unix 下替换后
//! 同步父目录，保证 rename 的目录项持久化。该层只处理文件系统语义，不吞掉最终错误。

use std::path::Path;

/// 原子移动已同步的暂存文件；目标已存在时必须拒绝，不能退回覆盖或复制。
/// 调用方在记录目录项归属后同步源、目标目录，并最后发布提交标记。
pub(crate) fn rename_new(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = |value: &Path| {
            std::ffi::CString::new(value.as_os_str().as_bytes()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "重命名路径包含 NUL (Rename path contains NUL)",
                )
            })
        };
        let from = path(from)?;
        let to = path(to)?;
        // SAFETY: 两个 CString 的有效长度均包含终止 NUL，c_char 对齐为 1；
        // 缓冲在整个同步调用期间独占持有，系统只读且不保存指针或接管所有权。
        // 内核原子检查排他标志，防止并发覆盖目标目录项。
        let result = unsafe {
            #[cfg(target_os = "linux")]
            {
                // 直接使用系统调用，兼容没有 renameat2 动态符号的旧版 glibc。
                libc::syscall(
                    libc::SYS_renameat2,
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            }
            #[cfg(target_os = "macos")]
            {
                libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL)
            }
        };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let path = |value: &Path| {
            let mut wide: Vec<u16> = value.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "重命名路径包含 NUL (Rename path contains NUL)",
                ));
            }
            wide.push(0);
            Ok(wide)
        };
        let from = path(from)?;
        let to = path(to)?;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        // SAFETY: 两个 Vec<u16> 均满足 u16 对齐，有效长度包含终止 NUL，
        // 整个同步调用期间独占持有；系统只读、不保存指针且不接管缓冲所有权。
        // 不设置 REPLACE_EXISTING 或 COPY_ALLOWED，并发碰撞及跨卷移动均返回错误。
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } != 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "当前平台不支持排他原子重命名 (Exclusive atomic rename is unsupported)",
        ))
    }
}

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
                            "原子重命名第 {} 次失败，系统错误 {:?}，将在 {:?} 后重试 (Atomic rename attempt failed; retrying after delay)",
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

    #[test]
    fn 排他移动保留完整内容且不会覆盖已有目标() {
        let root =
            std::env::temp_dir().join(format!("triviumdb_rename_new_{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let first = root.join("第一份.tmp");
        let second = root.join("第二份.tmp");
        let target = root.join("目标.tdb");
        std::fs::write(&first, b"first complete file").unwrap();
        std::fs::write(&second, b"second complete file").unwrap();
        rename_new(&first, &target).unwrap();
        assert!(!first.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"first complete file");
        assert_eq!(
            rename_new(&second, &target).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"first complete file");
        assert_eq!(std::fs::read(&second).unwrap(), b"second complete file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(target.metadata().unwrap().nlink(), 1);
        }
        // 目录由本用例独占创建，只含以上三条路径。
        std::fs::remove_dir_all(root).unwrap();
    }

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
