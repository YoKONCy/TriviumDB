//! 从已经固定代际的文件句柄查询Direct I/O约束。
use super::{IoAlignment, WINDOW_LIMIT};
use std::{fs::File, io};

impl IoAlignment {
    fn conservative() -> Self {
        Self {
            offset: WINDOW_LIMIT,
            memory: WINDOW_LIMIT,
            detected: false,
        }
    }

    fn detected(offset: usize, memory: usize) -> io::Result<Self> {
        if !offset.is_power_of_two()
            || !memory.is_power_of_two()
            || offset > WINDOW_LIMIT
            || memory > WINDOW_LIMIT
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "文件对齐超出有界I/O工作区支持范围 (File alignment exceeds bounded I/O workspace support)",
            ));
        }
        Ok(Self {
            offset,
            memory,
            detected: true,
        })
    }
}

#[cfg(windows)]
pub(super) fn query(file: &File) -> io::Result<IoAlignment> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::*;
    // SAFETY: 两个Win32 POD结构均允许零初始化，查询仅写入对应结构大小，文件句柄在调用期间存活。
    let mut storage: FILE_STORAGE_INFO = unsafe { std::mem::zeroed() };
    let mut alignment: FILE_ALIGNMENT_INFO = unsafe { std::mem::zeroed() };
    let storage_ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileStorageInfo,
            (&mut storage as *mut FILE_STORAGE_INFO).cast(),
            std::mem::size_of_val(&storage) as u32,
        )
    };
    if storage_ok == 0 {
        let error = io::Error::last_os_error();
        return if super::unsupported(&error) {
            Ok(IoAlignment::conservative())
        } else {
            Err(error)
        };
    }
    let alignment_ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAlignmentInfo,
            (&mut alignment as *mut FILE_ALIGNMENT_INFO).cast(),
            std::mem::size_of_val(&alignment) as u32,
        )
    };
    if alignment_ok == 0 {
        let error = io::Error::last_os_error();
        return if super::unsupported(&error) {
            Ok(IoAlignment::conservative())
        } else {
            Err(error)
        };
    }
    if storage.LogicalBytesPerSector == 0 {
        return Ok(IoAlignment::conservative());
    }
    // Windows对齐要求采用掩码表示；物理扇区大小同时用于缓冲和读取窗口粒度。
    let memory = (alignment.AlignmentRequirement as usize)
        .checked_add(1)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let offset = [
        storage.LogicalBytesPerSector,
        storage.PhysicalBytesPerSectorForAtomicity,
        storage.PhysicalBytesPerSectorForPerformance,
        storage.FileSystemEffectivePhysicalBytesPerSectorForAtomicity,
    ]
    .into_iter()
    .max()
    .unwrap_or(0) as usize;
    IoAlignment::detected(offset, memory.max(offset))
}

#[cfg(target_os = "linux")]
pub(super) fn query(file: &File) -> io::Result<IoAlignment> {
    use std::os::fd::AsRawFd;
    // SAFETY: statx为POD，空路径与AT_EMPTY_PATH定位仍存活的同一文件描述符，结构独占可写。
    let mut info: libc::statx = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::syscall(
            libc::SYS_statx,
            file.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_DIOALIGN | libc::STATX_BASIC_STATS,
            &mut info,
        )
    };
    if result != 0 {
        let error = io::Error::last_os_error();
        return if super::unsupported(&error) {
            Ok(IoAlignment::conservative())
        } else {
            Err(error)
        };
    }
    if info.stx_mask & libc::STATX_DIOALIGN == 0 {
        return Ok(IoAlignment::conservative());
    }
    // 使用通用DIO约束兼容提供更宽松只读约束的新内核；按文件系统块大小减少零碎读取。
    let offset = info.stx_dio_offset_align as usize;
    let memory = info.stx_dio_mem_align as usize;
    IoAlignment::detected(offset, memory)?;
    let block = info.stx_blksize as usize;
    let grain = if block.is_power_of_two() && block <= WINDOW_LIMIT {
        offset.max(block)
    } else {
        offset
    };
    IoAlignment::detected(grain, memory.max(grain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_requires_bounded_power_of_two_geometry() {
        let geometry = IoAlignment::detected(4096, 512).unwrap();
        assert_eq!(
            (geometry.offset, geometry.memory, geometry.detected),
            (4096, 512, true)
        );
        for (offset, memory) in [
            (0, 512),
            (512, 0),
            (3072, 4096),
            (4096, 768),
            (131072, 4096),
            (4096, 131072),
        ] {
            assert_eq!(
                IoAlignment::detected(offset, memory).err().unwrap().kind(),
                io::ErrorKind::Unsupported
            );
        }
        let fallback = IoAlignment::conservative();
        assert_eq!(
            (fallback.offset, fallback.memory, fallback.detected),
            (65536, 65536, false)
        );
    }
}
