//! 单个用户线程拥有整个ring，Rust仅使用本地C桥接的窄接口。
use super::*;
use std::{ffi::c_void, ptr::NonNull};

pub(super) const DEPTH: usize = 256;
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Info {
    pub mapping_bytes: u64, pub registered_bytes: u64,
    pub sq_entries: u32, pub cq_entries: u32, pub features: u32,
    pub flags: u32, pub affinity_count: u32, pub reserved: u32,
}
#[repr(C)]
#[derive(Default)]
struct Stats {
    submitted: u64, completed: u64, enter_calls: u64, enter_eintr: u64,
    max_inflight: u64, completion_batches: u64, completion_batch_max: u64, submit_batches: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Event { token: u64, result: i32, flags: u32 }
#[repr(C)]
#[derive(Default)]
pub(super) struct Stamp { pub monotonic_ns: u64, pub cpu_ns: u64 }
unsafe extern "C" {
    fn qv_ring_open(file: i32, buffer: *mut c_void, bytes: usize, depth: u32,
                    ring: *mut *mut c_void, info: *mut Info) -> i32;
    fn qv_ring_push(ring: *mut c_void, token: u64, buffer: *mut c_void, length: u32, offset: u64) -> i32;
    fn qv_ring_submit(ring: *mut c_void) -> i32;
    fn qv_ring_reap(ring: *mut c_void, events: *mut Event, capacity: u32) -> i32;
    fn qv_ring_stats(ring: *mut c_void, stats: *mut Stats);
    fn qv_ring_quiesce(ring: *mut c_void) -> i32;
    fn qv_ring_close(ring: *mut c_void) -> i32;
    fn qv_clock_stamp(stamp: *mut Stamp) -> i32;
}
fn checked(result: i32, operation: &str) -> io::Result<i32> {
    if result < 0 {
        let error = io::Error::from_raw_os_error(-result);
        return Err(io::Error::new(error.kind(), format!("io_uring {operation}失败，系统错误{error}")));
    }
    Ok(result)
}
pub(super) fn stamp() -> io::Result<Stamp> {
    let mut result = Stamp::default();
    // SAFETY：C只写入有效且布局一致的两个u64字段，调用完成后再读取。
    checked(unsafe { qv_clock_stamp(&mut result) }, "读取原生时钟")?;
    Ok(result)
}

pub(super) struct Ring {
    raw: Option<NonNull<c_void>>, _buffers: Arc<BufferPool>, pub info: Info,
}
// SAFETY：Ring在创建后只移动给一个调度线程，所有后续C调用都由该线程串行执行。
// 持有池的Arc确保固定注册与所有内核DMA结束前匿名slab不会解除映射。
unsafe impl Send for Ring {}
impl Ring {
    pub fn new(file: i32, buffers: Arc<BufferPool>) -> io::Result<Self> {
        let mut raw = std::ptr::null_mut();
        let mut info = Info::default();
        // SAFETY：整块slab地址、长度有效且按页对齐。C成功后持有固定注册，
        // 本对象保存池的Arc直到C排空请求、注销资源并关闭ring之后。
        checked(unsafe { qv_ring_open(file, buffers.slab.address.as_ptr().cast(), buffers.slab.bytes,
                                      DEPTH as u32, &mut raw, &mut info) }, "创建及注册")?;
        let raw = NonNull::new(raw).ok_or_else(|| io::Error::other("io_uring桥接返回空对象"))?;
        Ok(Self { raw: Some(raw), _buffers: buffers, info })
    }
    fn raw(&self) -> *mut c_void { self.raw.unwrap().as_ptr() }
    fn push(&mut self, token: u64, lease: &mut PageLease, offset: u64) -> io::Result<()> {
        let pointer = self._buffers.slab.write_ptr(lease);
        // SAFETY：独占租约尚未发布给读者，且由Operation保存到对应CQE或排空结束。
        // C验证地址位于唯一注册缓冲内，当前线程是SQ的唯一生产者。
        checked(unsafe { qv_ring_push(self.raw(), token, pointer, PAGE as u32, offset) }, "准备读取")?;
        Ok(())
    }
    fn submit(&mut self) -> io::Result<()> {
        // SAFETY：当前线程独占有效ring对象，全部SQE引用仍持有租约的页。
        checked(unsafe { qv_ring_submit(self.raw()) }, "批量提交")?;
        Ok(())
    }
    fn reap(&mut self, events: &mut [Event; DEPTH]) -> io::Result<usize> {
        // SAFETY：输出数组覆盖capacity个ABI一致的事件；C只在CQE可见后复制并推进CQ。
        Ok(checked(unsafe { qv_ring_reap(self.raw(), events.as_mut_ptr(), DEPTH as u32) }, "批量收割")? as usize)
    }
    fn quiesce(&mut self) -> io::Result<()> {
        // SAFETY：本线程停止发布新SQE，C撤回尚未消费的SQE并等待所有已消费请求的CQE。
        // 返回前Operation仍保存全部租约，不会提前归还内核可能访问的内存。
        checked(unsafe { qv_ring_quiesce(self.raw()) }, "错误排空")?;
        Ok(())
    }
    fn publish(&self, engine: &Engine) {
        let mut stats = Stats::default();
        // SAFETY：ring只在当前调度线程访问，C复制计数到有效的同布局输出对象。
        unsafe { qv_ring_stats(self.raw(), &mut stats); }
        let c = &engine.counters;
        for (counter, number) in [
            (&c.uring_submitted,stats.submitted),(&c.uring_completed,stats.completed),
            (&c.uring_enter_calls,stats.enter_calls),(&c.uring_enter_eintr,stats.enter_eintr),
            (&c.uring_max_inflight,stats.max_inflight),(&c.uring_completion_batches,stats.completion_batches),
            (&c.uring_completion_batch_max,stats.completion_batch_max),(&c.uring_submit_batches,stats.submit_batches),
        ] { counter.store(number,Ordering::Relaxed); }
    }
}
impl Drop for Ring {
    fn drop(&mut self) {
        if let Some(raw) = self.raw.take() {
            // SAFETY：本对象仍持有池的Arc。C先排空、再注销固定缓冲与文件、最后关闭并释放。
            let result = unsafe { qv_ring_close(raw.as_ptr()) };
            if result < 0 { eprintln!("关闭io_uring失败，系统错误{}",io::Error::from_raw_os_error(-result)); }
        }
    }
}

struct Operation { page: u64, task: Arc<Task>, lease: PageLease }
impl Engine {
    fn try_ring_buffer(&self) -> Option<PageLease> {
        loop {
            if let Some(lease) = self.buffers.try_take() { return Some(lease); }
            let evicted = self.evict_cached_page();
            if evicted.is_none() { return None; }
            drop(evicted);
        }
    }
    fn run_ring(&self, ring: &mut Ring, active: &mut HashMap<u64,Operation>) -> io::Result<()> {
        let mut events = [Event::default();DEPTH];
        let mut token = 0u64;
        loop {
            self.check_error()?;
            // SQ物理容量不变，只限制本片已提交且尚未收割的实际操作数。
            while active.len() < self.active_limit {
                let work = self.state.lock().unwrap().queue.pop_front();
                let Some((page,task)) = work else { break; };
                let Some(mut lease) = self.try_ring_buffer() else {
                    self.state.lock().unwrap().queue.push_front((page,task));
                    break;
                };
                let offset = page.checked_mul(PAGE as u64).ok_or_else(|| invalid("异步页偏移溢出"))?;
                token = token.checked_add(1).ok_or_else(|| invalid("异步读取标识溢出"))?;
                if let Err(error) = ring.push(token,&mut lease,offset) {
                    self.buffers.put(lease);
                    self.complete(page,&task,Err(io::Error::other(error.to_string())));
                    return Err(error);
                }
                add(&self.counters.queue_wait_ns,task.queued_at.elapsed().as_nanos().min(u64::MAX as u128) as u64);
                add(&self.counters.read_calls,1);
                add(&self.counters.worker_read_calls,1);
                self.counters.worker_mask.fetch_or(1,Ordering::Relaxed);
                active.insert(token,Operation {page,task,lease});
            }
            ring.submit()?;
            if active.is_empty() {
                let mut state=self.state.lock().unwrap();
                while state.queue.is_empty() && !state.shutdown && state.failure.is_none() {
                    state=self.work.wait(state).unwrap();
                }
                if state.shutdown { return Ok(()); }
                let has_queue=!state.queue.is_empty();
                drop(state);
                if has_queue { self.buffers.wait(); }
                continue;
            }
            let count=ring.reap(&mut events)?;
            // 先发布完整批次计数，再唤醒最后一个等待者，保证响应快照不会漏计。
            ring.publish(self);
            for event in &events[..count] {
                let operation=active.remove(&event.token).ok_or_else(|| io::Error::other("io_uring完成标识无对应租约"))?;
                let Operation {page,task,lease}=operation;
                let offset=page as usize*PAGE;
                let expected=PAGE.min(self.file_bytes-offset);
                let result = if event.flags != 0 {
                    Err(io::Error::other("io_uring完成标志异常"))
                } else if event.result < 0 {
                    let error=io::Error::from_raw_os_error(-event.result);
                    Err(io::Error::new(error.kind(),format!("O_DIRECT异步读取页{page}失败，系统错误{error}")))
                } else {
                    add(&self.counters.read_bytes,event.result as u64);
                    if event.result as usize != expected {
                        Err(io::Error::new(io::ErrorKind::UnexpectedEof,format!("O_DIRECT页{page}短读，期望{expected}字节，实际{}字节",event.result)))
                    } else { Ok(event.result as usize) }
                };
                match result {
                    Ok(valid)=>self.complete(page,&task,Ok(Arc::new(PageData {buffer:Some(lease),valid,
                        pool:self.buffers.clone(),counters:self.counters.clone(),prefetched:task.prefetched,used:AtomicBool::new(false)}))),
                    Err(error)=>{
                        self.buffers.put(lease);
                        add(&self.counters.read_errors,1);
                        self.complete(page,&task,Err(io::Error::other(error.to_string())));
                        return Err(error);
                    }
                }
            }
        }
    }
    pub(super) fn uring_worker(&self, mut ring: Ring) {
        let mut active=HashMap::with_capacity(DEPTH);
        if let Err(error)=self.run_ring(&mut ring,&mut active) {
            add(&self.counters.ring_errors,1);
            let message=error.to_string();
            {
                let mut state=self.state.lock().unwrap();
                state.failure.get_or_insert_with(||message.clone());
                self.publish_cache_failure(&message);
            }
            let queued=self.state.lock().unwrap().queue.drain(..).collect::<Vec<_>>();
            for (page,task) in queued { self.complete(page,&task,Err(io::Error::other(message.clone()))); }
            for operation in active.values() {
                self.complete(operation.page,&operation.task,Err(io::Error::other(message.clone())));
            }
            // 等待者先获得错误，租约仍留在active中，直到所有可能的DMA结束。
            if let Err(drain)=ring.quiesce() { eprintln!("io_uring错误排空记录{}",drain); }
            ring.publish(self);
            for (_,operation) in active.drain() { self.buffers.put(operation.lease); }
            self.changed.notify_all();
        }
    }
}
