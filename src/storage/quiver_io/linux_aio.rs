//! Linux内核AIO。每个工作线程独占上下文，异常路径通过io_destroy同步排空。
use std::io;

#[repr(C)]
#[derive(Default)]
struct ControlBlock {
    data: u64,
    key_flags: [u32; 2],
    opcode: u16,
    priority: i16,
    fd: u32,
    buffer: u64,
    bytes: u64,
    offset: i64,
    reserved: u64,
    flags: u32,
    event_fd: u32,
}

#[repr(C)]
#[derive(Default)]
struct Event {
    data: u64,
    object: u64,
    result: i64,
    result2: i64,
}

const _: () = assert!(std::mem::size_of::<ControlBlock>() == 64);
const _: () = assert!(std::mem::size_of::<Event>() == 32);

pub(super) struct NativeAio {
    context: libc::c_ulong,
}
impl NativeAio {
    pub fn new() -> io::Result<Self> {
        let mut context: libc::c_ulong = 0;
        // SAFETY: 内核ABI要求指向可写aio_context_t，初值为零；每个上下文仅供一个工作线程使用。
        if unsafe { libc::syscall(libc::SYS_io_setup, 1u32, &mut context) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { context })
    }

    /// 调用方保证缓冲及文件存活至返回，缓冲满足O_DIRECT对齐要求且保持独占。
    pub unsafe fn read(
        &mut self,
        fd: libc::c_int,
        offset: u64,
        buffer: *mut u8,
        bytes: usize,
    ) -> io::Result<usize> {
        if self.context == 0 {
            return Err(io::Error::other("AIO上下文已关闭 (AIO context closed)"));
        }
        let offset =
            i64::try_from(offset).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut control = ControlBlock {
            data: 1,
            fd: fd as u32,
            buffer: buffer as usize as u64,
            bytes: bytes as u64,
            offset,
            ..Default::default()
        };
        let mut pointer = &mut control as *mut ControlBlock;
        // SAFETY: 单个完整内核ABI控制块，操作码0表示PREAD，指针数组和对齐缓冲在完成前保持有效。
        let submitted = unsafe {
            libc::syscall(
                libc::SYS_io_submit,
                self.context,
                1 as libc::c_long,
                &mut pointer,
            )
        };
        if submitted < 0 {
            return Err(io::Error::last_os_error());
        }
        if submitted != 1 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let mut pending = Pending {
            context: self,
            completed: false,
        };
        let mut event = Event::default();
        loop {
            // SAFETY: 独占上下文仅有此请求，事件缓冲大小与ABI一致，空超时表示等到实际完成。
            let count = unsafe {
                libc::syscall(
                    libc::SYS_io_getevents,
                    pending.context.context,
                    1 as libc::c_long,
                    1 as libc::c_long,
                    &mut event,
                    std::ptr::null::<libc::timespec>(),
                )
            };
            if count == 1 {
                break;
            }
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
        }
        if event.data != 1 || event.object != pointer as usize as u64 {
            return Err(io::Error::other(
                "AIO完成项身份无效 (Invalid AIO completion identity)",
            ));
        }
        pending.completed = true;
        if event.result < 0 {
            return Err(io::Error::from_raw_os_error((-event.result) as i32));
        }
        if event.result2 != 0 || event.result as u64 > bytes as u64 {
            return Err(io::Error::other("AIO完成项无效 (Invalid AIO completion)"));
        }
        Ok(event.result as usize)
    }

    fn destroy(&mut self) {
        if self.context == 0 {
            return;
        }
        loop {
            // SAFETY: 独占有效上下文，io_destroy取消请求并等待全部未取消I/O完成后才归还缓冲所有权。
            let result = unsafe { libc::syscall(libc::SYS_io_destroy, self.context) };
            if result >= 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        self.context = 0;
    }
}
struct Pending<'a> {
    context: &'a mut NativeAio,
    completed: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.context.destroy();
        }
    }
}
impl Drop for NativeAio {
    fn drop(&mut self) {
        self.destroy();
    }
}
