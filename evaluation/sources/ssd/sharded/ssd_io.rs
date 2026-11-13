//! 共享的有界O_DIRECT页读取和研究JSONL入口。
use super::*;
mod uring;
mod int8_codec;
mod direct_int8;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    sync::{Arc, Condvar, Mutex, RwLock, atomic::{AtomicBool, AtomicU64, Ordering}},
    thread::{self, JoinHandle},
    time::Duration,
};
use triviumdb::index::quiver::corrected_sm::ColdVectorSource;

const PAGE: usize = if option_env!("QUIVER_IO_1K").is_some(){1024}else{4096};
const OSPAGE: usize = 4096;
const MAX_QUERY_PAGES: usize = 128;
const MAX_SEARCH_THREADS: usize = 32;
unsafe extern "C" {
    fn pread(fd: i32, buffer: *mut std::ffi::c_void, count: usize, offset: i64) -> isize;
    fn mmap(address: *mut std::ffi::c_void, length: usize, protection: i32, flags: i32,
            descriptor: i32, offset: i64) -> *mut std::ffi::c_void;
    fn munmap(address: *mut std::ffi::c_void, length: usize) -> i32;
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
fn field(c: &Value, key: &str, default: usize) -> usize {
    c[key].as_u64().map(|v| v as usize).unwrap_or(default)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend { Warm, Sync, Batch, Lookahead }
impl Backend {
    fn parse(c: &Value) -> io::Result<Self> {
        match c["backend"].as_str().unwrap_or("warm_mmap") {
            "warm_mmap" | "warm" => Ok(Self::Warm),
            "sync_direct" | "sync" => Ok(Self::Sync),
            "batch_final" | "batch" => Ok(Self::Batch),
            "lookahead" => Ok(Self::Lookahead),
            _ => Err(invalid("未知SSD后端")),
        }
    }
    fn name(self) -> &'static str {
        match self { Self::Warm => "warm_mmap", Self::Sync => "sync_direct", Self::Batch => "batch_final", Self::Lookahead => "lookahead" }
    }
}

#[derive(Clone,Copy,PartialEq,Eq)]
enum Driver { Pread,Uring }
impl Driver {
    fn parse(c: &Value)->io::Result<Self> {
        match c["io_driver"].as_str().unwrap_or("pread") {
            "pread"=>Ok(Self::Pread),"uring"=>Ok(Self::Uring),_=>Err(invalid("IO驱动须为pread或uring")),
        }
    }
    fn name(self)->&'static str {match self {Self::Pread=>"pread",Self::Uring=>"uring"}}
}

#[derive(Default)]
struct Counters {
    evicted_pages: AtomicU64,
    local_metrics: bool,
    read_calls: AtomicU64, read_bytes: AtomicU64, read_errors: AtomicU64,
    cache_hits: AtomicU64, cache_misses: AtomicU64, pending_hits: AtomicU64,
    query_page_hits: AtomicU64, prefetch_issued: AtomicU64, prefetch_used: AtomicU64,
    prefetch_wasted: AtomicU64, prefetch_skipped: AtomicU64,
    worker_read_calls: AtomicU64, inline_read_calls: AtomicU64,
    worker_mask: AtomicU64,
    queue_wait_ns: AtomicU64, ring_errors: AtomicU64,
    uring_submitted: AtomicU64, uring_completed: AtomicU64,
    uring_enter_calls: AtomicU64, uring_enter_eintr: AtomicU64,
    uring_max_inflight: AtomicU64, uring_completion_batches: AtomicU64,
    uring_completion_batch_max: AtomicU64, uring_submit_batches: AtomicU64,
}
fn add(counter: &AtomicU64, value: u64) { counter.fetch_add(value, Ordering::Relaxed); }
fn value(counter: &AtomicU64) -> u64 { counter.load(Ordering::Relaxed) }

/// 匿名映射只有一份，租约按编译时I/O块粒度划分，物理页由实际读取按需触及。
struct Slab { address: std::ptr::NonNull<u8>, bytes: usize }

// SAFETY：映射地址与长度在创建后保持不变。共享对象本身不暴露可写切片，
// 每个I/O块索引只能由BufferPool的锁保护分配一次；独占租约不能克隆。
// 写入仅发生在pread完成前，完成后PageData经Arc共享，所有访问均为只读。
unsafe impl Send for Slab {}
// SAFETY：不同独占租约覆盖互不重叠的页。最后一个PageData持有的池引用
// 释放前映射始终有效，因此跨线程读写不会重叠，也不会提前munmap。
unsafe impl Sync for Slab {}

impl Slab {
    fn new(pages: usize) -> io::Result<Self> {
        let bytes = pages.checked_mul(PAGE).ok_or_else(|| invalid("匿名页池长度溢出"))?;
        if bytes == 0 { return Err(invalid("匿名页池不能为空")); }
        // SAFETY：Linux匿名私有映射使用有效长度、读写权限、匿名文件描述符-1
        // 和偏移0。未设置预触页标志，mmap成功仅预留地址，后续pread按需触页。
        let address = unsafe { mmap(std::ptr::null_mut(), bytes, 1 | 2, 2 | 0x20, -1, 0) };
        if address as isize == -1 { return Err(io::Error::last_os_error()); }
        let Some(address) = std::ptr::NonNull::new(address.cast::<u8>()) else {
            // SAFETY：该长度对应刚创建成功但不适合Rust切片要求的零地址映射。
            unsafe { munmap(address, bytes); }
            return Err(io::Error::other("匿名页池返回了零地址"));
        };
        if address.as_ptr() as usize % PAGE != 0 {
            // SAFETY：撤销刚创建且尚未借出的完整映射。
            unsafe { munmap(address.as_ptr().cast(), bytes); }
            return Err(io::Error::other("匿名页池地址未按4096字节对齐"));
        }
        Ok(Self { address, bytes })
    }

    fn write_ptr(&self, lease: &mut PageLease) -> *mut std::ffi::c_void {
        // SAFETY：租约只能由同一池分配，索引严格小于页数。调用方独占租约，
        // 其页尚未发布给Arc读者；返回指针用于持有租约期间的同步或异步读取。
        unsafe { self.address.as_ptr().add(lease.index * PAGE).cast() }
    }

    fn bytes<'a>(&'a self, lease: &'a PageLease) -> &'a [u8] {
        // SAFETY：租约有效期间整页始终属于当前PageData，且pread已完成。
        // PageData的Arc保证返回切片存在时不会回收租约；池的Arc保证映射存活。
        // 页内只有一个写入者，发布后只有共享只读访问，因此不存在可变别名。
        unsafe { std::slice::from_raw_parts(self.address.as_ptr().add(lease.index * PAGE), PAGE) }
    }
}
impl Drop for Slab {
    fn drop(&mut self) {
        // SAFETY：Slab只由BufferPool拥有。每个已借出PageData都持有池的Arc，
        // 因而运行到这里时全部租约与读者已释放，撤销整段原始映射不会留下悬垂访问。
        if unsafe { munmap(self.address.as_ptr().cast(), self.bytes) } != 0 {
            eprintln!("释放匿名页池失败，系统错误{}", io::Error::last_os_error());
        }
    }
}

/// 不实现Clone，索引在回收前只能属于一个读取任务或PageData。
struct PageLease { index: usize }
struct PoolState { free: Vec<usize>, allocated: usize, live: usize, peak_live: usize }
struct BufferPool { limit: usize, slab: Slab, state: Mutex<PoolState>, changed: Condvar }
impl BufferPool {
    fn new(limit: usize) -> io::Result<Self> {
        Ok(Self {
            limit, slab: Slab::new(limit)?,
            state: Mutex::new(PoolState { free: Vec::new(), allocated: 0, live: 0, peak_live: 0 }),
            changed: Condvar::new(),
        })
    }
    fn try_take(&self) -> Option<PageLease> {
        let mut state = self.state.lock().unwrap();
        let index = if let Some(index) = state.free.pop() {
            index
        } else {
            if state.allocated >= self.limit { return None; }
            let index = state.allocated;
            state.allocated += 1;
            index
        };
        state.live += 1;
        state.peak_live = state.peak_live.max(state.live);
        Some(PageLease { index })
    }
    fn put(&self, lease: PageLease) {
        let mut state = self.state.lock().unwrap();
        state.live -= 1;
        state.free.push(lease.index);
        self.changed.notify_all();
    }
    fn wait(&self) {
        let state = self.state.lock().unwrap();
        if state.free.is_empty() && state.allocated == self.limit {
            let _ = self.changed.wait_timeout(state, Duration::from_millis(10)).unwrap();
        }
    }
    fn snapshot(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap();
        // 保留v1统计语义，allocated计已分配过租约的页，live计当前持有页。
        (state.allocated * PAGE, state.live * PAGE, state.peak_live * PAGE)
    }
    fn free_list_bytes(&self) -> usize {
        self.state.lock().unwrap().free.capacity() * std::mem::size_of::<usize>()
    }
}
struct PageData {
    buffer: Option<PageLease>, valid: usize, pool: Arc<BufferPool>,
    counters: Arc<Counters>, prefetched: bool, used: AtomicBool,
}
impl PageData {
    fn consume(&self) {
        if self.prefetched {
            // 已标记的页面只读访问，首次使用仍由原子交换唯一计数。
            let first_use=if self.counters.local_metrics {
                !self.used.load(Ordering::Relaxed) && !self.used.swap(true,Ordering::Relaxed)
            }else{!self.used.swap(true,Ordering::Relaxed)};
            if first_use {add(&self.counters.prefetch_used,1);}
        }
    }
}
impl Drop for PageData {
    fn drop(&mut self) {
        if self.prefetched && !self.used.load(Ordering::Relaxed) { add(&self.counters.prefetch_wasted, 1); }
        self.pool.put(self.buffer.take().unwrap());
    }
}
type PageResult = Result<Arc<PageData>, String>;
struct Task { result: Mutex<Option<PageResult>>, ready: Condvar, prefetched: bool, queued_at: Instant }
impl Task {
    fn new(prefetched: bool) -> Self { Self { result: Mutex::new(None), ready: Condvar::new(), prefetched, queued_at: Instant::now() } }
    fn wait(&self) -> io::Result<Arc<PageData>> {
        let mut result = self.result.lock().unwrap();
        while result.is_none() { result = self.ready.wait(result).unwrap(); }
        result.as_ref().unwrap().clone().map_err(io::Error::other)
    }
}
#[derive(Clone)]
enum Handle { Ready(Arc<PageData>), Pending(Arc<Task>) }
impl Handle {
    fn wait(&self) -> io::Result<Arc<PageData>> {
        match self { Self::Ready(page) => Ok(page.clone()), Self::Pending(task) => task.wait() }
    }
}
/// 仅索引主cache已持有的同一批不可变页，不分配向量副本。
/// 所有写入必须持有state后再取得本索引写锁，读锁不得升级或调用state路径。
#[derive(Default)]
struct CacheReadIndex {
    pages: HashMap<u64, Arc<PageData>>,
    failure: Option<String>,
}

struct State {
    cache: HashMap<u64, Arc<PageData>>, fifo: VecDeque<u64>,
    pending: HashMap<u64, Arc<Task>>, queue: VecDeque<(u64, Arc<Task>)>,
    shutdown: bool, cache_peak: usize, pending_peak: usize, failure: Option<String>,
}
struct Engine {
    file: fs::File, file_bytes: usize, cache_pages: usize, inflight_pages: usize,
    workers: usize, active_limit: usize, driver: Driver, ring_info: uring::Info, buffers: Arc<BufferPool>, counters: Arc<Counters>,
    state: Mutex<State>, work: Condvar, changed: Condvar,
    cache_read_hits: bool, cache_read_index: RwLock<CacheReadIndex>,
    router_failure:Option<Arc<RouterFailure>>,worker_identities:Mutex<Vec<Value>>,
}
struct Runtime { engine: Arc<Engine>, threads: Vec<JoinHandle<()>> }
impl Runtime {
    fn open(path: &Path, bytes: usize, cache_bytes: usize, inflight_bytes: usize, workers: usize, driver: Driver, local_metrics: bool, cache_read_hits: bool, router_failure:Option<Arc<RouterFailure>>, active_limit:usize) -> io::Result<Self> {
        if !(1..=uring::DEPTH).contains(&active_limit) {return Err(invalid("每片实际active-I/O上限必须在1到256之间"));}
        if cache_bytes % PAGE != 0 || inflight_bytes % PAGE != 0 || inflight_bytes < MAX_SEARCH_THREADS*MAX_QUERY_PAGES*PAGE {
            return Err(invalid("缓存与在途预算必须按4096对齐，在途预算至少16MiB"));
        }
        if workers == 0 || workers > 32 || (driver==Driver::Uring && workers!=1) {
            return Err(invalid("pread工作线程须1到32，uring调度线程须1"));
        }
        let file=fs::OpenOptions::new().read(true).custom_flags(0x4000).open(path)?;
        if file.metadata()?.len()!=bytes as u64 { return Err(invalid("FP32原始文件长度与n及dim不符")); }
        let buffers=Arc::new(BufferPool::new((cache_bytes+inflight_bytes)/PAGE)?);
        let mut ring_info=uring::Info::default();
        let mut scheduler=None;
        let mut engine_sender=None;
        if driver==Driver::Uring {
            // Linux5.15的IOWQ亲和性注册只作用于current的io_wq。
            // 创建ring、注册预算与后续enter全部留在同一个最终调度线程中。
            let (ready_sender,ready_receiver)=std::sync::mpsc::sync_channel::<Result<uring::Info,String>>(1);
            let (sender,receiver)=std::sync::mpsc::sync_channel::<Arc<Engine>>(1);
            let file_fd=file.as_raw_fd();
            let ring_buffers=buffers.clone();
            let handle=thread::Builder::new().name("SSD异步调度".into()).spawn(move|| {
                let ring=match uring::Ring::new(file_fd,ring_buffers) {
                    Ok(ring)=>ring,
                    Err(error)=>{
                        let _=ready_sender.send(Err(error.to_string()));
                        return;
                    }
                };
                if ready_sender.send(Ok(ring.info)).is_err() { return; }
                // 主线程先取得实际注册Info再构造Engine，第二次握手后才允许启动读取。
                // Ring始终持有池的Arc，握手失败时析构会先注销资源再释放slab。
                if let Ok(engine)=receiver.recv() {
                    engine.worker_identities.lock().unwrap().push(io_thread_identity());
                    engine.uring_worker(ring);
                }
            })?;
            match ready_receiver.recv() {
                Ok(Ok(info))=>ring_info=info,
                Ok(Err(error))=>{
                    let _=handle.join();
                    return Err(io::Error::other(error));
                }
                Err(error)=>{
                    let _=handle.join();
                    return Err(io::Error::other(format!("io_uring创建线程握手失败，{error}")));
                }
            }
            scheduler=Some(handle);
            engine_sender=Some(sender);
        }
        let engine=Arc::new(Engine {
            file,file_bytes:bytes,cache_pages:cache_bytes/PAGE,inflight_pages:inflight_bytes/PAGE,
            workers,active_limit,driver,ring_info,buffers,counters:Arc::new(Counters {local_metrics, ..Counters::default()}),
            state:Mutex::new(State {cache:HashMap::new(),fifo:VecDeque::new(),pending:HashMap::new(),
                queue:VecDeque::new(),shutdown:false,cache_peak:0,pending_peak:0,failure:None}),
            work:Condvar::new(),changed:Condvar::new(),
            cache_read_hits,cache_read_index:RwLock::new(CacheReadIndex::default()),
            router_failure,worker_identities:Mutex::new(Vec::new()),
        });
        let mut runtime=Self {engine,threads:Vec::new()};
        if let Some(scheduler)=scheduler {
            runtime.threads.push(scheduler);
            engine_sender.unwrap().send(runtime.engine.clone())
                .map_err(|_|io::Error::other("io_uring调度线程接收Engine失败"))?;
            let probe=runtime.engine.request(0,false,true,false)?.unwrap().wait()?;
            drop(probe);
            runtime.engine.reset_cache();
            runtime.engine.check_error()?;
        } else {
            let probe=runtime.engine.read_page(0,false,false)?;
            drop(probe);
            for id in 0..workers {
                let worker=runtime.engine.clone();
                runtime.threads.push(thread::Builder::new().name(format!("SSD读取{id}")).spawn(move||{worker.worker_identities.lock().unwrap().push(io_thread_identity());worker.worker(id)})?);
            }
        }
        Ok(runtime)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.engine.wait_idle();
        self.engine.state.lock().unwrap().shutdown = true;
        self.engine.work.notify_all();
        for worker in self.threads.drain(..) { let _ = worker.join(); }
    }
}
impl Engine {

    /// 调用方必须已持有state；错误与主状态在同一写事务内发布。
    fn publish_cache_failure(&self, message: &str) {
        if let Some(failure)=&self.router_failure {failure.publish(message);}
        if self.cache_read_hits {
            let mut index = self.cache_read_index.write().unwrap();
            index.failure.get_or_insert_with(|| message.to_owned());
        }
    }

    fn evict_cached_page(&self) -> Option<Arc<PageData>> {
        let mut state = self.state.lock().unwrap();
        let page = state.fifo.pop_front()?;
        if self.cache_read_hits {
            // 固定锁序为state再index.write，索引必须在主cache移除前同步删除。
            self.cache_read_index.write().unwrap().pages.remove(&page);
        }
        // 返回最后一个拥有者，由调用方在所有cache锁释放后销毁页或归还租约。
        let evicted=state.cache.remove(&page);
        if evicted.is_some() {add(&self.counters.evicted_pages,1);}
        evicted
    }
    fn buffer(&self) -> PageLease {
        loop {
            if let Some(buffer) = self.buffers.try_take() { return buffer; }
            let evicted = self.evict_cached_page();
            if evicted.is_some() { drop(evicted); } else { self.buffers.wait(); }
        }
    }
    fn read_page(&self, page: u64, prefetched: bool, worker: bool) -> io::Result<Arc<PageData>> {
        let offset = (page as usize).checked_mul(PAGE).ok_or_else(|| invalid("SSD页偏移溢出"))?;
        if offset >= self.file_bytes { return Err(invalid("SSD页超出FP32文件")); }
        let mut buffer = self.buffer();
        let result = loop {
            add(&self.counters.read_calls, 1);
            if worker { add(&self.counters.worker_read_calls, 1); } else { add(&self.counters.inline_read_calls, 1); }
            // SAFETY：当前读取任务独占该页租约，指针按编译时I/O粒度对齐且块长充足。
            // 同步pread返回前不发布PageData，其余租约对应互不重叠的页。
            let size = unsafe { pread(self.file.as_raw_fd(), self.buffers.slab.write_ptr(&mut buffer), PAGE, offset as i64) };
            if size >= 0 { break Ok(size as usize); }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted { break Err(error); }
        };
        let size = match result {
            Ok(size) => size,
            Err(error) => {
                self.buffers.put(buffer); add(&self.counters.read_errors, 1);
                return Err(io::Error::new(error.kind(), format!("O_DIRECT读取页{page}失败，系统错误{error}")));
            }
        };
        add(&self.counters.read_bytes, size as u64);
        let expected = PAGE.min(self.file_bytes - offset);
        if size != expected {
            self.buffers.put(buffer); add(&self.counters.read_errors, 1);
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("O_DIRECT页{page}短读，期望{expected}字节，实际{size}字节")));
        }
        Ok(Arc::new(PageData { buffer: Some(buffer), valid: size, pool: self.buffers.clone(), counters: self.counters.clone(), prefetched, used: AtomicBool::new(false) }))
    }
    fn complete(&self, page: u64, task: &Arc<Task>, result: io::Result<Arc<PageData>>) {
        let result = result.map_err(|error| error.to_string());
        *task.result.lock().unwrap() = Some(result.clone());
        task.ready.notify_all();
        let mut state = self.state.lock().unwrap();
        if let Err(error) = &result {
            state.failure.get_or_insert_with(|| error.clone());
            self.publish_cache_failure(error);
        }
        if let Ok(data) = result {
            if self.cache_pages > 0 {
                // 开关关闭时不获取索引锁，主cache、FIFO与pending沿用原有状态机。
                let mut index = self.cache_read_hits.then(|| self.cache_read_index.write().unwrap());
                while state.cache.len() >= self.cache_pages {
                    if let Some(old) = state.fifo.pop_front() {
                        if let Some(index) = index.as_mut() { index.pages.remove(&old); }
                        if state.cache.remove(&old).is_some() {add(&self.counters.evicted_pages,1);}
                    }
                }
                if let Some(index) = index.as_mut() { index.pages.insert(page, data.clone()); }
                state.cache.insert(page, data);
                state.fifo.push_back(page);
                state.cache_peak = state.cache_peak.max(state.cache.len());
            }
        }
        state.pending.remove(&page);
        self.changed.notify_all();
    }
    fn worker(&self, id: usize) {
        loop {
            let work = {
                let mut state = self.state.lock().unwrap();
                while state.queue.is_empty() && !state.shutdown { state = self.work.wait(state).unwrap(); }
                if state.shutdown { return; }
                state.queue.pop_front().unwrap()
            };
            let (page, task) = work;
            add(&self.counters.queue_wait_ns,task.queued_at.elapsed().as_nanos().min(u64::MAX as u128) as u64);
            self.counters.worker_mask.fetch_or(1u64 << id, Ordering::Relaxed);
            let result = self.read_page(page, task.prefetched, true);
            self.complete(page, &task, result);
        }
    }
    fn request(&self, page: u64, prefetch: bool, required: bool, inline: bool) -> io::Result<Option<Handle>> {

        if self.cache_read_hits {
            // 本作用域结束前只访问读索引，禁止在持有读锁时尝试取得state。
            let cached = {
                let index = self.cache_read_index.read().unwrap();
                if let Some(error) = &index.failure {
                    return Err(io::Error::other(error.clone()));
                }
                index.pages.get(&page).cloned()
            };
            if let Some(data) = cached {
                if !prefetch { add(&self.counters.cache_hits, 1); }
                return Ok(Some(Handle::Ready(data)));
            }
            // 读锁已释放，miss进入原状态机后重新检查cache和pending，防止重复提交。
        }
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(error) = &state.failure { return Err(io::Error::other(error.clone())); }
            if let Some(data) = state.cache.get(&page) {
                if !prefetch { add(&self.counters.cache_hits, 1); }
                return Ok(Some(Handle::Ready(data.clone())));
            }
            if let Some(task) = state.pending.get(&page) {
                if !prefetch { add(&self.counters.pending_hits, 1); }
                return Ok(Some(Handle::Pending(task.clone())));
            }
            if state.pending.len() < self.inflight_pages { break; }
            if !required { add(&self.counters.prefetch_skipped, 1); return Ok(None); }
            state = self.changed.wait(state).unwrap();
        }
        if prefetch { add(&self.counters.prefetch_issued, 1); } else { add(&self.counters.cache_misses, 1); }
        let task = Arc::new(Task::new(prefetch));
        state.pending.insert(page, task.clone());
        state.pending_peak = state.pending_peak.max(state.pending.len());
        if inline && self.driver==Driver::Pread {
            drop(state);
            let result = self.read_page(page, prefetch, false);
            self.complete(page, &task, result);
        } else {
            state.queue.push_back((page, task.clone()));
            self.work.notify_one();
        }
        Ok(Some(Handle::Pending(task)))
    }
    fn wait_idle(&self) {
        let mut state = self.state.lock().unwrap();
        while !state.pending.is_empty() { state = self.changed.wait(state).unwrap(); }
    }
    fn reset_cache(&self) {
        self.wait_idle();
        let mut state = self.state.lock().unwrap();
        // reset在所有pending排空后持有state，索引与主cache同时清空，错误状态保留。
        let mut index = self.cache_read_hits.then(|| self.cache_read_index.write().unwrap());
        if let Some(index) = index.as_mut() { index.pages.clear(); }
        state.cache.clear(); state.fifo.clear();
    }
    fn check_error(&self) -> io::Result<()> {
        match &self.state.lock().unwrap().failure {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }
    fn snapshot(&self) -> Value {
        let state = self.state.lock().unwrap();
        let (allocated, live, peak_live) = self.buffers.snapshot();
        let c = &self.counters;
        let allocator_snapshot = allocator();
        let (index_entries,index_capacity) = if self.cache_read_hits {
            let index = self.cache_read_index.read().unwrap();
            (index.pages.len(),index.pages.capacity())
        } else {(0,0)};
        let unused = state.cache.values().filter(|page| page.prefetched && !page.used.load(Ordering::Relaxed)).count();
        json!({
            "evicted_pages":value(&c.evicted_pages),
            "read_calls":value(&c.read_calls),"read_bytes":value(&c.read_bytes),"read_errors":value(&c.read_errors),
            "cache_hits":value(&c.cache_hits),"cache_misses":value(&c.cache_misses),"pending_hits":value(&c.pending_hits),
            "query_page_hits":value(&c.query_page_hits),"prefetch_issued":value(&c.prefetch_issued),
            "prefetch_used":value(&c.prefetch_used),"prefetch_wasted":value(&c.prefetch_wasted),"prefetch_skipped":value(&c.prefetch_skipped),
            "prefetch_unused_resident":unused,
            "worker_read_calls":value(&c.worker_read_calls),"inline_read_calls":value(&c.inline_read_calls),
            "cache_current_bytes":state.cache.len()*PAGE,"cache_peak_bytes":state.cache_peak*PAGE,
            "inflight_current_bytes":state.pending.len()*PAGE,"inflight_peak_bytes":state.pending_peak*PAGE,
            "buffers_allocated_bytes":allocated,"buffers_current_bytes":live,"buffers_peak_bytes":peak_live,
            "buffer_pool_limit_bytes":self.buffers.limit*PAGE,"io_workers":self.workers,
            "io_workers_used":value(&c.worker_mask).count_ones(),
            "io_driver":self.driver.name(),"driver":self.driver.name(),
            "queue_wait_ns":value(&c.queue_wait_ns),"ring_errors":value(&c.ring_errors),
            "uring_submitted":value(&c.uring_submitted),"uring_completed":value(&c.uring_completed),
            "uring_enter_calls":value(&c.uring_enter_calls),"uring_enter_eintr":value(&c.uring_enter_eintr),
            "uring_max_inflight":value(&c.uring_max_inflight),"uring_completion_batches":value(&c.uring_completion_batches),
            "uring_completion_batch_max":value(&c.uring_completion_batch_max),"uring_submit_batches":value(&c.uring_submit_batches),
            "registered_buffer_bytes":self.ring_info.registered_bytes,"pinned_buffer_bytes":self.ring_info.registered_bytes,
            "ring_mapping_bytes":self.ring_info.mapping_bytes,
            "cache_policy":"FIFO","prefetch_wasted_counted_on_page_release":true,
            "cache_read_hits":self.cache_read_hits,"cache_read_index_entries":index_entries,
            "cache_read_index_capacity_entries":index_capacity,"cache_read_index_payload_copies":0,
            "page_buffer_backend":"anonymous_mmap_slab_v2","slab_mapping_bytes":self.buffers.slab.bytes,
            "slab_free_list_capacity_bytes":self.buffers.free_list_bytes(),
            "slab_page_metadata_struct_bytes":live/PAGE*std::mem::size_of::<PageData>(),
            "allocator_arena_bytes":allocator_snapshot["arena_bytes"],
            "allocator_free_arena_bytes":allocator_snapshot["free_arena_bytes"],
            "allocator_in_use_bytes":allocator_snapshot["in_use_plus_allocator_mmap_bytes"],
            "allocator_scope":"进程分配器计数包括热索引、元数据与工作区；直接匿名mmap页池另列，结构体元数据字节不含Arc头与哈希表"
        })
    }
}


// 各子Engine只拥有自己的ring统计。跨片仅共享错误发布，不共享I/O计数器。
#[derive(Default)]
struct RouterFailure {failed:AtomicBool,message:Mutex<Option<String>>}
impl RouterFailure {
    fn publish(&self,message:&str) {
        self.message.lock().unwrap().get_or_insert_with(||message.to_owned());
        self.failed.store(true,Ordering::Release);
    }
    fn check(&self)->io::Result<()> {
        if self.failed.load(Ordering::Acquire) {
            let message=self.message.lock().unwrap().clone().unwrap_or_else(||"I/O分片已经失败".into());
            return Err(io::Error::other(message));
        }
        Ok(())
    }
}
fn io_thread_identity()->Value {
    let status=fs::read_to_string("/proc/thread-self/status").unwrap_or_default();
    let allowed=status.lines().find_map(|line|line.strip_prefix("Cpus_allowed_list:"))
        .unwrap_or("").trim().to_owned();
    let count=allowed.split(',').filter(|part|!part.is_empty()).map(|part| {
        let mut limits=part.split('-').filter_map(|item|item.parse::<usize>().ok());
        let low=limits.next().unwrap_or(0);let high=limits.next().unwrap_or(low);
        high.saturating_sub(low)+1
    }).sum::<usize>();
    let tid=fs::read_link("/proc/thread-self").ok().and_then(|path|path.file_name()
        .and_then(|name|name.to_str()).and_then(|name|name.parse::<u64>().ok()));
    json!({"tid":tid,"cpus_allowed_list":allowed,"cpus_count":count})
}
struct RouterEngine {
    children:Vec<Arc<Engine>>,counters:Arc<Counters>,ring_info:uring::Info,
    failure:Arc<RouterFailure>,workers_per_shard:usize,
}
struct RouterRuntime {engine:RouterEngine,_children:Vec<Runtime>}
impl RouterRuntime {
    fn open(path:&Path,bytes:usize,cache_bytes:usize,inflight_bytes:usize,workers:usize,
        driver:Driver,local_metrics:bool,cache_read_hits:bool,shards:usize,total_active_io:usize)->io::Result<Self> {
        if !matches!(shards,1|2|4) || cache_bytes%(shards*PAGE)!=0 || inflight_bytes%(shards*PAGE)!=0 {
            return Err(invalid("I/O分片只能为1、2、4，缓存和在途总预算必须按分片及I/O块整除"));
        }
        if shards>1 && PAGE!=1024 {return Err(invalid("多I/O分片研究只支持1KiB构建"));}
        if !matches!(total_active_io,0|256) {return Err(invalid("总active-I/O只支持256或0，0表示每片256"));}
        let active_limit=if total_active_io==0 {uring::DEPTH}else{total_active_io/shards};
        let failure=Arc::new(RouterFailure::default());
        let mut runtimes=Vec::with_capacity(shards);
        for _ in 0..shards {
            // 每个Runtime执行原同上下文ring创建、注册、进入和排空，不共用slab或pending。
            runtimes.push(Runtime::open(path,bytes,cache_bytes/shards,inflight_bytes/shards,workers,
                driver,local_metrics,cache_read_hits,if shards>1 {Some(failure.clone())}else{None},active_limit)?);
        }
        let children=runtimes.iter().map(|runtime|runtime.engine.clone()).collect::<Vec<_>>();
        let counters=if shards==1 {children[0].counters.clone()}else {
            Arc::new(Counters {local_metrics,..Counters::default()})
        };
        let mut ring_info=children[0].ring_info;
        for child in &children[1..] {
            ring_info.mapping_bytes+=child.ring_info.mapping_bytes;
            ring_info.registered_bytes+=child.ring_info.registered_bytes;
            ring_info.sq_entries+=child.ring_info.sq_entries;
            ring_info.cq_entries+=child.ring_info.cq_entries;
            if child.ring_info.affinity_count!=ring_info.affinity_count {
                return Err(io::Error::other("I/O分片的注册CPU预算不一致"));
            }
        }
        Ok(Self {engine:RouterEngine {children,counters,ring_info,failure,workers_per_shard:workers},_children:runtimes})
    }
}
impl RouterEngine {
    fn request(&self,page:u64,prefetch:bool,required:bool,inline:bool)->io::Result<Option<Handle>> {
        // 单片直接进入原实现；多片页号低位唯一映射，不复制同页cache和pending。
        if self.children.len()==1 {return self.children[0].request(page,prefetch,required,inline);}
        self.failure.check()?;
        self.children[(page as usize)&(self.children.len()-1)].request(page,prefetch,required,inline)
    }
    fn wait_idle(&self) {for child in &self.children {child.wait_idle();}}
    fn reset_cache(&self) {self.wait_idle();for child in &self.children {child.reset_cache();}}
    fn check_error(&self)->io::Result<()> {
        self.failure.check()?;
        for child in &self.children {child.check_error()?;}
        Ok(())
    }
    fn worker_threads(&self)->Vec<Value> {
        self.children.iter().enumerate().flat_map(|(shard,child)| {
            child.worker_identities.lock().unwrap().iter().cloned().map(|mut item| {
                item["shard"]=json!(shard);item
            }).collect::<Vec<_>>()
        }).collect()
    }
    fn configuration(&self)->Value {
        let child=&self.children[0];let count=self.children.len();
        json!({"io_shards":count,"routing":"page_low_bits","rings":if child.driver==Driver::Uring {count}else{0},
            "workers_per_shard":self.workers_per_shard,"worker_threads":self.worker_threads(),
            "cache_bytes_per_shard":child.cache_pages*PAGE,"inflight_bytes_per_shard":child.inflight_pages*PAGE,
            "slab_bytes_per_shard":child.buffers.slab.bytes,
            "queue_depth_per_shard":if child.driver==Driver::Uring {256}else{0},
            "queue_depth_total":if child.driver==Driver::Uring {256*count}else{0},
            "queue_capacity_total":if child.driver==Driver::Uring {256*count}else{0},
            "configured_active_io_total":if child.driver==Driver::Uring {child.active_limit*count}else{0},
            "configured_active_io_per_shard":if child.driver==Driver::Uring {child.active_limit}else{0},
            "cache_policy":"per_shard_FIFO","page_uniqueness":"one_page_one_child_cache_pending_ring",
            "peak_aggregation":"sum_of_child_lifetime_peaks_upper_bound",
            "snapshot_scope":"sequential_child_snapshots","shared_io_counters":false})
    }
    fn snapshot(&self)->Value {
        let shards=self.children.iter().enumerate().map(|(index,child)| {
            let mut snapshot=child.snapshot();
            snapshot["shard"]=json!(index);
            snapshot["cache_capacity_bytes"]=json!(child.cache_pages*PAGE);
            snapshot["inflight_capacity_bytes"]=json!(child.inflight_pages*PAGE);
            snapshot["queue_depth"]=json!(if child.driver==Driver::Uring {256}else{0});
            snapshot["queue_capacity"]=json!(if child.driver==Driver::Uring {256}else{0});
            snapshot["configured_active_io_limit"]=json!(if child.driver==Driver::Uring {child.active_limit}else{0});
            snapshot["registered_file_count"]=json!(if child.driver==Driver::Uring {1}else{0});
            snapshot["worker_threads"]=json!(child.worker_identities.lock().unwrap().clone());
            snapshot
        }).collect::<Vec<_>>();
        let mut total=shards[0].clone();
        if shards.len()>1 {
            for key in ["evicted_pages","read_calls","read_bytes","read_errors","cache_hits","cache_misses",
                "pending_hits","query_page_hits","prefetch_issued","prefetch_used","prefetch_wasted","prefetch_skipped",
                "prefetch_unused_resident","worker_read_calls","inline_read_calls","cache_current_bytes","cache_peak_bytes",
                "inflight_current_bytes","inflight_peak_bytes","buffers_allocated_bytes","buffers_current_bytes",
                "buffers_peak_bytes","buffer_pool_limit_bytes","io_workers","io_workers_used","queue_wait_ns",
                "ring_errors","uring_submitted","uring_completed","uring_enter_calls","uring_enter_eintr",
                "uring_max_inflight","uring_completion_batches","uring_submit_batches","registered_buffer_bytes",
                "pinned_buffer_bytes","ring_mapping_bytes","cache_read_index_entries","cache_read_index_capacity_entries",
                "slab_mapping_bytes","slab_free_list_capacity_bytes","slab_page_metadata_struct_bytes"] {
                total[key]=json!(shards.iter().map(|child|child[key].as_u64().unwrap_or(0)).sum::<u64>());
            }
            total["uring_completion_batch_max"]=json!(shards.iter().map(|child|child["uring_completion_batch_max"].as_u64().unwrap_or(0)).max().unwrap_or(0));
            let hits=value(&self.counters.query_page_hits);let skipped=value(&self.counters.prefetch_skipped);
            total["query_page_hits"]=json!(total["query_page_hits"].as_u64().unwrap()+hits);
            total["prefetch_skipped"]=json!(total["prefetch_skipped"].as_u64().unwrap()+skipped);
            total["router_query_page_hits"]=json!(hits);total["router_prefetch_skipped"]=json!(skipped);
            total["cache_policy"]=json!("per_shard_FIFO");
        }else{
            total["router_query_page_hits"]=json!(0);total["router_prefetch_skipped"]=json!(0);
        }
        // 分配器字段是进程全局快照，保留首片观察，不能逐片累加。
        let child=&self.children[0];
        total["queue_capacity_total"]=json!(if child.driver==Driver::Uring {uring::DEPTH*shards.len()}else{0});
        total["configured_active_io_total"]=json!(if child.driver==Driver::Uring {child.active_limit*shards.len()}else{0});
        total["configured_active_io_per_shard"]=json!(if child.driver==Driver::Uring {child.active_limit}else{0});
        total["io_shards"]=json!(shards.len());
        total["sharding"]=self.configuration();total["shards"]=json!(shards);
        total["peak_aggregation"]=json!("sum_of_child_lifetime_peaks_upper_bound");
        total["allocator_aggregation"]=json!("process_global_observed_once_not_summed");
        total
    }
}

struct QuerySource<'a> {
    engine: &'a RouterEngine, backend: Backend, n: usize, dim: usize, row_stride: usize,
    encoding:int8_codec::Encoding,slots:Option<&'a mut Vec<usize>>,
    mode:direct_int8::Mode,query:Option<direct_int8::Query>,stats:&'a direct_int8::Stats,
    layout:direct_int8::Layout,
    depth: usize, every: usize, handles: HashMap<u64, Handle>, local_page_hits:u64, stream_hint_counts:bool,
}
impl Drop for QuerySource<'_> {
    fn drop(&mut self) {
        // 析构包含失败返回路径，保证已经完成的页命中不会漏记。
        if self.local_page_hits>0 {add(&self.engine.counters.query_page_hits,self.local_page_hits);}
    }
}
impl QuerySource<'_> {

    fn int8_row(&mut self,slot:usize)->io::Result<[u8;868]> {
        if self.encoding!=int8_codec::Encoding::Int8 {return Err(invalid("直接INT8要求INT8冷文件"));}
        if let Some(slots)=self.slots.as_mut() {slots.push(slot);}
        let pages=self.pages(slot)?;
        let start=slot*self.row_stride;
        let end=start+self.layout.payload();
        let mut row=[0u8;868];
        let mut copied=0;
        for page in pages {
            let handle=if let Some(handle)=self.handles.get(&page) {
                if self.engine.counters.local_metrics {self.local_page_hits+=1;}else{add(&self.engine.counters.query_page_hits,1);}handle.clone()
            }else{self.engine.request(page,false,true,self.backend==Backend::Sync)?.unwrap()};
            let data=handle.wait()?;
            data.consume();
            let page_start=page as usize*PAGE;
            let lo=start.saturating_sub(page_start);
            let hi=(end-page_start).min(PAGE);
            if hi>data.valid {return Err(io::Error::new(io::ErrorKind::UnexpectedEof,"INT8冷行跨块读取不足"));}
            let bytes=&data.pool.slab.bytes(data.buffer.as_ref().unwrap())[lo..hi];
            row[copied..copied+bytes.len()].copy_from_slice(bytes);
            copied+=bytes.len();
        }
        if copied!=self.layout.payload() {return Err(invalid("分组INT8行有效载荷长度错误"));}
        Ok(row)
    }
    fn pages(&self, slot: usize) -> io::Result<std::ops::RangeInclusive<u64>> {
        if slot >= self.n { return Err(invalid("冷向量slot越界")); }
        let start = slot * self.row_stride;
        Ok((start / PAGE) as u64..=((start + self.encoding.payload(self.dim,self.layout) - 1) / PAGE) as u64)
    }
}
impl ColdVectorSource for QuerySource<'_> {

    fn prepare_query(&mut self,q:&[f32])->io::Result<()> {
        if self.mode==direct_int8::Mode::Direct {self.query=Some(direct_int8::Query::new(q,self.stats,self.layout)?);}
        Ok(())
    }
    fn direct_one(&mut self,slot:usize)->io::Result<Option<f32>> {
        if self.mode!=direct_int8::Mode::Direct {return Ok(None);}
        let row=self.int8_row(slot)?;
        let query=self.query.as_ref().ok_or_else(||invalid("直接INT8 query尚未准备"))?;
        Ok(Some(direct_int8::score(query,[row.as_slice()],self.stats,self.layout)?[0]))
    }
    fn direct_four(&mut self,slots:[usize;4])->io::Result<Option<[f32;4]>> {
        if self.mode!=direct_int8::Mode::Direct {return Ok(None);}
        let mut rows=[[0u8;868];4];
        for k in 0..4 {rows[k]=self.int8_row(slots[k])?;}
        let query=self.query.as_ref().ok_or_else(||invalid("直接INT8 query尚未准备"))?;
        Ok(Some(direct_int8::score(query,std::array::from_fn(|k|rows[k].as_slice()),self.stats,self.layout)?))
    }
    fn fill(&mut self,_slot:usize,_buffer:&mut Vec<f32>)->io::Result<bool> {
        Err(invalid("分组INT8禁止解码候选为FP32"))
    }
    fn hint(&mut self, slots: &[usize], final_batch: bool) -> io::Result<()> {
        if self.backend == Backend::Sync { return Ok(()); }
        if !final_batch && self.backend != Backend::Lookahead { return Ok(()); }
        let mut hint_returns=[0u64;2];
        let mut pages = Vec::new();
        let mut unique = HashSet::new();
        for &slot in slots {
            for page in self.pages(slot)? { if unique.insert(page) { pages.push(page); } }
        }
        if final_batch {
            if pages.len() > MAX_QUERY_PAGES { return Err(invalid("最终候选页超过研究入口上限")); }
            self.handles.retain(|page, _| unique.contains(page));
        }
        for page in pages {
            if self.handles.contains_key(&page) { continue; }
            if self.handles.len() >= MAX_QUERY_PAGES { add(&self.engine.counters.prefetch_skipped, 1); continue; }
            if let Some(handle) = self.engine.request(page, true, final_batch, false)? {
                if self.stream_hint_counts {
                    match &handle {Handle::Ready(_)=>hint_returns[0]+=1,Handle::Pending(_)=>hint_returns[1]+=1}
                }
                self.handles.insert(page, handle);
            }
        }
        if self.stream_hint_counts {
            STREAM_HINT_RETURNS.with(|counts| {
                let previous=counts.get();
                counts.set([previous[0]+hint_returns[0],previous[1]+hint_returns[1]]);
            });
        }
        Ok(())
    }
    fn hint_depth(&self) -> usize { if self.backend == Backend::Lookahead { self.depth } else { 0 } }
    fn hint_every(&self) -> usize { self.every }
}

fn delta(before: &Value, after: &Value) -> Value {
    let mut result = after.clone();
    for key in ["read_calls","read_bytes","read_errors","cache_hits","cache_misses","pending_hits","query_page_hits","prefetch_issued","prefetch_used","prefetch_wasted","prefetch_skipped","worker_read_calls","inline_read_calls","queue_wait_ns","ring_errors","uring_submitted","uring_completed","uring_enter_calls","uring_enter_eintr","uring_completion_batches","uring_submit_batches","evicted_pages"] {
        result[key] = json!(after[key].as_u64().unwrap().saturating_sub(before[key].as_u64().unwrap()));
    }
    if let (Some(old),Some(new))=(before["shards"].as_array(),after["shards"].as_array()) {
        result["shards"]=json!(old.iter().zip(new.iter()).map(|(old,new)|delta(old,new)).collect::<Vec<_>>());
    }
    for key in ["router_query_page_hits","router_prefetch_skipped"] {
        if let (Some(old),Some(new))=(before[key].as_u64(),after[key].as_u64()) {
            result[key]=json!(new.saturating_sub(old));
        }
    }
    result["counters_scope"] = json!("当前请求，吞吐预热不计入；当前值与峰值属于进程累计状态");
    result
}

fn file_residency(path: &Path) -> io::Result<Value> {
    let file = fs::File::open(path)?;
    let mapping = unsafe { memmap2::MmapOptions::new().map(&file)? };
    let mut pages = vec![0u8; mapping.len().div_ceil(OSPAGE)];
    let rc = unsafe { mincore(mapping.as_ptr() as *mut std::ffi::c_void, mapping.len(), pages.as_mut_ptr()) };
    if rc != 0 { return Err(io::Error::last_os_error()); }
    let resident = pages.iter().filter(|&&page| page & 1 != 0).count();
    Ok(json!({"pages":pages.len(),"resident_pages":resident,"resident_fraction":resident as f64/pages.len() as f64,
        "note":"临时只读映射仅查询mincore，未读取FP32，返回前已解除映射"}))
}

unsafe extern "C" {
    #[link_name = "posix_fadvise"]
    fn layout_fadvise(fd: i32, offset: i64, length: i64, advice: i32) -> i32;
}
fn release_layout_pages(path: &Path) -> io::Result<()> {
    let file = fs::File::open(path)?;
    // SAFETY：只读文件描述符在调用期间有效，0长度覆盖整个文件，4为Linux的DONTNEED提示。
    let result = unsafe { layout_fadvise(file.as_raw_fd(), 0, 0, 4) };
    if result != 0 { return Err(io::Error::from_raw_os_error(result)); }
    Ok(())
}

// stream只运行输入一次，窗口观测保留在顶层原生计时内。
const STREAM_QUERY_COUNT: usize = 20_000;
const STREAM_WINDOW_QUERIES: usize = 1_000;
thread_local! {
    static STREAM_HINT_RETURNS: std::cell::Cell<[u64;2]> = const { std::cell::Cell::new([0,0]) };
}
fn take_stream_hint_returns(pool:&rayon::ThreadPool)->[u64;2] {
    pool.broadcast(|_|STREAM_HINT_RETURNS.with(|counts|counts.replace([0,0])))
        .into_iter().fold([0u64;2],|mut total,counts| {
            total[0]+=counts[0];total[1]+=counts[1];total
        })
}
struct StreamOutput {
    neighbors:Vec<Vec<(u64,f32)>>,
    latencies:Vec<u64>,
    windows:Vec<Value>,
}
fn run_stream<F>(pool:&rayon::ThreadPool,queries:&[f32],dim:usize,one:&F,
                 engine:Option<&RouterEngine>,score_stats:&direct_int8::Stats)->io::Result<StreamOutput>
where F:Fn(&[f32])->io::Result<Vec<(u64,f32)>>+Send+Sync {
    if queries.len()!=STREAM_QUERY_COUNT*dim {
        return Err(invalid("stream输入必须恰好20000条query，禁止循环重复凑时长"));
    }
    let mut output=StreamOutput {
        neighbors:Vec::with_capacity(STREAM_QUERY_COUNT),latencies:Vec::with_capacity(STREAM_QUERY_COUNT),
        windows:Vec::with_capacity(STREAM_QUERY_COUNT/STREAM_WINDOW_QUERIES),
    };
    // 清除本请求各worker的TLS，之后每个窗口只取回本窗口新返回的句柄计数。
    take_stream_hint_returns(pool);
    for first in (0..STREAM_QUERY_COUNT).step_by(STREAM_WINDOW_QUERIES) {
        let end=first+STREAM_WINDOW_QUERIES;
        let before=engine.map(|engine|engine.snapshot());
        let score_before=score_stats.snapshot();
        let start=uring::stamp()?;
        let rows=pool.install(||queries[first*dim..end*dim].par_chunks_exact(dim).map(|q| {
            let begin=Instant::now();
            one(q).map(|neighbors|(neighbors,begin.elapsed().as_nanos() as u64))
        }).collect::<io::Result<Vec<_>>>())?;
        if let Some(engine)=engine {engine.wait_idle();engine.check_error()?;}
        let finish=uring::stamp()?;
        // 以下广播、快照与JSON组装不计窗口native，但仍位于顶层native区间内。
        let returns=take_stream_hint_returns(pool);
        let score_after=score_stats.snapshot();
        let after=engine.map(|engine|engine.snapshot());
        let io_stats=before.as_ref().zip(after.as_ref()).map(|(before,after)|delta(before,after));
        let mut window_latencies=Vec::with_capacity(STREAM_WINDOW_QUERIES);
        for (neighbors,latency) in rows {
            output.neighbors.push(neighbors);
            output.latencies.push(latency);
            window_latencies.push(latency);
        }
        let total_returns=returns[0]+returns[1];
        let hit_rate=if total_returns==0 {None}else{Some(returns[0] as f64/total_returns as f64)};
        let current=after.as_ref().and_then(|stats|stats["cache_current_bytes"].as_u64()).unwrap_or(0);
        let peak=after.as_ref().and_then(|stats|stats["cache_peak_bytes"].as_u64()).unwrap_or(0);
        let index_entries=after.as_ref().and_then(|stats|stats["cache_read_index_entries"].as_u64()).unwrap_or(0);
        let evicted=io_stats.as_ref().and_then(|stats|stats["evicted_pages"].as_u64()).unwrap_or(0);
        output.windows.push(json!({
            "window_index":first/STREAM_WINDOW_QUERIES,"query_start_index":first,"query_end_index_exclusive":end,
            "queries_executed":STREAM_WINDOW_QUERIES,"seconds":(finish.monotonic_ns-start.monotonic_ns) as f64/1e9,
            "process_cpu_ns":finish.cpu_ns-start.cpu_ns,
            "native_interval":{"monotonic_start_ns":start.monotonic_ns,"monotonic_finish_ns":finish.monotonic_ns,
                "process_cpu_start_ns":start.cpu_ns,"process_cpu_finish_ns":finish.cpu_ns},
            "latency_ns":window_latencies,"io_stats_before":before,"io_stats_after":after,"io_stats":io_stats,
            "cache_current_bytes":current,"cache_peak_bytes":peak,"cache_read_index_entries":index_entries,
            "evicted_pages":evicted,"hint_ready_returns":returns[0],"hint_pending_returns":returns[1],
            "hint_return_cache_hit_rate":hit_rate,"pending_returns_include_new_and_merged":true,
            "decoded_vector_count":score_after[0]-score_before[0],"direct_dot_count":score_after[1]-score_before[1],
            "query_quantization_count":score_after[2]-score_before[2]
        }));
    }
    if output.neighbors.len()!=STREAM_QUERY_COUNT || output.latencies.len()!=STREAM_QUERY_COUNT {
        return Err(io::Error::other("stream执行总量或延迟长度不完整"));
    }
    Ok(output)
}

pub(super) fn serve(c: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let dim = field(c, "dim", 0); let n = field(c, "n", 0);
    if dim != 768 || n == 0 || c["route"] != "l2" { return Err(invalid("SSD首轮要求非空768维SM-L2配置").into()); }
    let total_active_io=field(c,"total_active_io",256);
    if !matches!(total_active_io,0|256) {return Err(invalid("总active-I/O只支持256或0").into());}
    let io_shards=field(c,"io_shards",1);
    if !matches!(io_shards,1|2|4) {return Err(invalid("I/O分片只支持1、2、4").into());}
    let backend = Backend::parse(c)?;
    if backend==Backend::Warm && io_shards!=1 {return Err(invalid("暖基线不创建I/O分片，请设置io_shards=1").into());}
    let driver = Driver::parse(c)?;
    if driver==Driver::Uring && field(c,"queue_depth",256)!=256 {return Err(invalid("本轮uring深度固定为256").into());}
    let cache_bytes = field(c, "cache_bytes", 128*1024*1024);
    let inflight_bytes = field(c, "inflight_bytes", 32*1024*1024);
    let workers = field(c, "io_workers", if driver==Driver::Uring {1}else{16});
    let depth = match c["prefetch_mode"].as_str().unwrap_or("top32") {
        "top32" | "32" => 32, "top64" | "64" => 64, "none" => 0,
        _ => return Err(invalid("预取模式必须为top32、top64或none").into()),
    };
    let every = field(c, "prefetch_every", 16);
    if every == 0 { return Err(invalid("预取提示间隔必须大于零").into()); }
    let basepath = Path::new(c["base"].as_str().ok_or_else(|| invalid("缺少FP32路径"))?);
    let encoding=int8_codec::Encoding::parse(c)?;
    let layout=direct_int8::Layout::parse(c)?;
    let score_mode=direct_int8::Mode::parse(c)?;
    if field(c,"io_block_bytes",PAGE)!=PAGE {return Err(invalid("配置I/O块大小与编译二进制不符").into());}
    if PAGE==1024 && backend!=Backend::Warm && encoding!=int8_codec::Encoding::Int8 {
        return Err(invalid("1KiB读取二进制的冷后端只支持INT8").into());
    }
    if score_mode==direct_int8::Mode::Direct {
        if encoding!=int8_codec::Encoding::Int8 {return Err(invalid("直接INT8评分要求INT8冷编码").into());}
        direct_int8::self_check()?;
    }
    let local_metrics=c["local_metrics"].as_bool().unwrap_or(false);
    let cache_read_hits=c["cache_read_hits"].as_bool().unwrap_or(false);
    let scoring_stats=Arc::new(direct_int8::Stats::default());
    if encoding==int8_codec::Encoding::Int8 && c["cold_base"].as_str().is_none() {
        return Err(invalid("INT8必须提供独立cold_base，禁止FP32回退").into());
    }
    let coldpath = Path::new(c["cold_base"].as_str().unwrap_or_else(|| basepath.to_str().unwrap()));
    let default_stride=if encoding==int8_codec::Encoding::Int8 {1024}else if c.get("cold_encoding").is_some(){OSPAGE}else{dim*4};
    let row_stride=field(c,"cold_row_stride_bytes",default_stride);
    if encoding==int8_codec::Encoding::Int8 {
        if row_stride!=1024 || coldpath.canonicalize()?==basepath.canonicalize()? {
            return Err(invalid("INT8要求独立冷文件和1024字节行步长").into());
        }
    } else if row_stride!=dim*4 && row_stride!=OSPAGE {
        return Err(invalid("冷FP32行步长必须为逻辑维度字节数或4096").into());
    }
    let logical_fp32_bytes = n.checked_mul(dim*4).ok_or_else(|| invalid("逻辑FP32长度溢出"))?;
    let cold_storage_bytes = n.checked_mul(row_stride).ok_or_else(|| invalid("冷FP32存储长度溢出"))?;
    if fs::metadata(coldpath)?.len() != cold_storage_bytes as u64 {
        return Err(invalid("冷FP32文件长度与节点数及行步长不符").into());
    }

    let begin = Instant::now();
    let mut index = QuIVer::load_from_file(Path::new(c["index"].as_str().ok_or_else(|| invalid("缺少图路径"))?))?;
    let load_s = begin.elapsed().as_secs_f64();
    if index.total_count() != n { return Err(invalid("图节点数与配置不符").into()); }
    let loaded_status = fs::read_to_string("/proc/self/status")?;
    let loaded_allocator = allocator();
    let before = index.stats().hot_bytes;
    let vectors = VecPool::<f32>::open(basepath, dim, n)?;
    vectors.advise_random();
    let begin = Instant::now();
    index.enable_sm_l2_with_vectors(|slot, buffer| {
        buffer.clear(); buffer.extend_from_slice(vectors.get(slot).unwrap()); true
    })?;
    let install_s = begin.elapsed().as_secs_f64();
    let installation_status = fs::read_to_string("/proc/self/status")?;
    let installation_residency = residency(&vectors, n, dim);
    let warm_vectors = if backend == Backend::Warm && encoding==int8_codec::Encoding::Fp32 { Some(vectors) } else {
        // 只操作本实验只读原始FP32映射，释放安装时的PTE与清洁文件页。
        evict(&vectors, basepath);
        drop(vectors);
        None
    };
    let (warm_int8,int8_validation_residency)=if encoding==int8_codec::Encoding::Int8 {
        let (mapping,resident)=int8_codec::checked_map(coldpath,n,layout)?;
        if backend==Backend::Warm {(Some(mapping),Some(resident))}else{drop(mapping);(None,Some(resident))}
    }else{(None,None)};
    // 冻结文件哈希会暖化整个派生文件，READY前单独释放其清洁页缓存。
    if backend != Backend::Warm { release_layout_pages(coldpath)?; }
    unsafe { malloc_trim(0); }
    let runtime = if backend == Backend::Warm { None } else {
        Some(RouterRuntime::open(coldpath, cold_storage_bytes, cache_bytes, inflight_bytes, workers, driver, local_metrics, cache_read_hits, io_shards, total_active_io)?)
    };
    let engine = runtime.as_ref().map(|runtime| &runtime.engine);
    let io_config = json!({"backend":backend.name(),"direct_io":backend!=Backend::Warm,
        "cache_bytes":if engine.is_some(){cache_bytes}else{0},"inflight_bytes":if engine.is_some(){inflight_bytes}else{0},
        "cache_read_hits":cache_read_hits,"local_metrics":local_metrics,"io_workers":if engine.is_some(){workers*io_shards}else{0},"io_workers_per_shard":if engine.is_some(){workers}else{0},"prefetch_depth":depth,"prefetch_every":every,
        "buffer_pool_limit_bytes":if engine.is_some(){cache_bytes+inflight_bytes}else{0},
        "max_query_handle_pages":MAX_QUERY_PAGES,"max_search_threads":MAX_SEARCH_THREADS,
        "stream_supported":true,"stream_query_count":20000,"stream_window_queries":1000,
        "stream_cache_return_counter":"worker_tls_hint_ready_pending_broadcast_take",
        "io_driver":driver.name(),"driver":driver.name(),
        "io_shards":if engine.is_some(){io_shards}else{0},"io_shards_requested":io_shards,
        "io_shard_config":engine.map(|engine|engine.configuration()),
        "io_worker_threads":engine.map(|engine|engine.worker_threads()),
        "io_scheduler_threads":if engine.is_some(){workers*io_shards}else{0},
        "ring_setup":engine.is_some()&&driver==Driver::Uring,
        "queue_depth":if engine.is_some()&&driver==Driver::Uring{256*io_shards}else{0},
        "queue_depth_per_shard":if engine.is_some()&&driver==Driver::Uring{256}else{0},
        "queue_capacity_total":if engine.is_some()&&driver==Driver::Uring{256*io_shards}else{0},
        "total_active_io_requested":total_active_io,
        "configured_active_io_total":if engine.is_some()&&driver==Driver::Uring{if total_active_io==0 {256*io_shards}else{total_active_io}}else{0},
        "configured_active_io_per_shard":if engine.is_some()&&driver==Driver::Uring{if total_active_io==0 {256}else{total_active_io/io_shards}}else{0},
        "registered_buffer_bytes":engine.map(|e|e.ring_info.registered_bytes).unwrap_or(0),
        "pinned_buffer_bytes":engine.map(|e|e.ring_info.registered_bytes).unwrap_or(0),
        "registered_file_count":if engine.is_some()&&driver==Driver::Uring{io_shards}else{0},
        "registered_unique_files":if engine.is_some()&&driver==Driver::Uring{1}else{0},
        "iowq_max_workers":if engine.is_some()&&driver==Driver::Uring{vec![io_shards,io_shards]}else{vec![0,0]},
        "iowq_max_workers_per_shard":if engine.is_some()&&driver==Driver::Uring{vec![1,1]}else{vec![0,0]},
        "iowq_affinity_cpu_count":engine.map(|e|e.ring_info.affinity_count).unwrap_or(0),
        "ring_mapping_bytes":engine.map(|e|e.ring_info.mapping_bytes).unwrap_or(0),
        "cold_encoding":encoding.name(),"cold_row_stride_bytes":row_stride,
        "io_block_bytes":PAGE,"os_page_bytes":OSPAGE,"score_mode":score_mode.name(),
        "scoring_kernel":score_mode.kernel(),"integer_kernel_self_check":true,
        "grouped_integer_self_check":true,"int8_group_size":layout.group_size,
        "int8_norm_correction":layout.norm_correction,"int8_groups":layout.groups(),
        "norm_factor_offset_bytes":816,"ls_factor_offset_bytes":820,"selected_factor_offset_bytes":layout.factor_offset(),
        "int8_scale_mode":layout.scale_mode.name(),"int8_query_mode":layout.query_mode.name(),
        "encoding_variant":c["encoding_variant"].as_str().unwrap_or("unspecified"),
        "refine_integer_self_check":true,"direct_dot_count_unit":"candidate_vectors",
        "norm_factor_definition":"f32(1/(normhat_f64*original_exact_scale_f32))，原scale遵守sq阈值1e-5，normhat与原向量同时0时factor1",
        "query_quantization":layout.query_mode.quantization(),
        "int8_bias_correction":"sum(q_u8*db_i8)-128*sum(db_i8)",
        "direct_score_restore":"六块顺序float32累加((idot_f32*qscale_block)*scale)+((signed_query_block_sum_f32*qscale_block)*bias)，末尾一次乘norm或LSfactor，外层乘original_exact_scale",
        "ls_factor_definition":"f32(sum_f64(raw_original*raw_reconstructed)/sum_f64(raw_reconstructed^2))",
        "cold_codec":layout.codec(),
        "cold_payload_bytes":encoding.payload(dim,layout),"scale_offset_bytes":if encoding==int8_codec::Encoding::Int8 {768}else{0},
        "scale_method":"group128，数据文件提供每块scale/bias，encoding_variant仅作为文件身份标签",
        "database_rounding":"float32_round_ties_even_clamp_minus127_127",
        "direct_io_fallback":false,"page_buffer_backend":"anonymous_mmap_slab_v2",
        "slab_mapping_bytes":if engine.is_some(){cache_bytes+inflight_bytes}else{0},
        "pool_allocation":"单块匿名mmap按需触页，独占页索引租约回收复用，缓存与查询持有页共享同一个硬上限"});
    println!("{}", json!({"ready":true,"pid":std::process::id(),"config":c,"io_config":io_config,
        "backend":backend.name(),"index_load_s":load_s,"install_s":install_s,
        "io_block_bytes":PAGE,"score_mode":score_mode.name(),"scoring_kernel":score_mode.kernel(),
        "decoded_vector_count":0,"direct_dot_count":0,"query_quantization_count":0,
        "raw_hot_stats_bytes":before,"routed_hot_stats_bytes":index.stats().hot_bytes,
        "loaded_status":loaded_status,"loaded_allocator":loaded_allocator,"installation_status":installation_status,
        "installation_residency":installation_residency,"installed_status":fs::read_to_string("/proc/self/status")?,
        "installed_allocator":allocator(),"routed_memory":index.corrected_sm_memory_components(),
        "vector_heap_logical_bytes":0,"vector_mmap_bytes":warm_vectors.as_ref().map(|v|v.mmap_bytes()).unwrap_or(0),
        "fp32_mmap_bytes":warm_vectors.as_ref().map(|v|v.mmap_bytes()).unwrap_or(0),
        "cold_mmap_bytes":warm_int8.as_ref().map(|v|v.len()).unwrap_or(0),"int8_validation_residency":int8_validation_residency,
        "residency":warm_vectors.as_ref().map(|v|residency(v,n,dim)),
        "input_file_residency":file_residency(basepath)?,
        "cold_file_residency":file_residency(coldpath)?,"cold_path":coldpath.to_string_lossy(),
        "cold_row_stride_bytes":row_stride,"cold_storage_bytes":cold_storage_bytes,
        "logical_fp32_bytes":logical_fp32_bytes,"cold_padding_bytes":cold_storage_bytes-n*encoding.payload(dim,layout),
        "cold_encoding":encoding.name(),"cold_payload_bytes":encoding.payload(dim,layout),"cold_codec":layout.codec(),
        "int8_group_size":layout.group_size,"int8_norm_correction":layout.norm_correction,
        "norm_factor_offset_bytes":816,"ls_factor_offset_bytes":820,
        "selected_factor_offset_bytes":layout.factor_offset(),"int8_scale_mode":layout.scale_mode.name(),
        "int8_query_mode":layout.query_mode.name(),"query_quantization":layout.query_mode.quantization(),
        "encoding_variant":c["encoding_variant"].as_str().unwrap_or("unspecified"),
        "io_stats_total":engine.map(|e|e.snapshot()),"timing":"同图SM-L2和原FP32精排，直接读取后端已释放安装映射"}));
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "EXIT" { break; }
        let request: Value = serde_json::from_str(&line)?;
        let result = (|| -> Result<Value, Box<dyn std::error::Error>> {
            let query = floats(Path::new(request["qfile"].as_str().ok_or_else(|| invalid("缺少查询路径"))?));
            if query.is_empty() || query.len()%dim != 0 { return Err(invalid("查询文件形状无效").into()); }
            let threads = field(&request,"threads",1);
            if threads == 0 || threads > MAX_SEARCH_THREADS { return Err(invalid("搜索线程必须在1到32之间").into()); }
            if request["mode"] != "l2" { return Err(invalid("SSD研究入口mode必须为l2").into()); }
            if request["reset_cache"].as_bool().unwrap_or(false) { if let Some(engine) = engine { engine.reset_cache(); } }
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build()?;
            let search = QuIVerSearchConfig { top_k:10, ef_search:field(&request,"ef",88), rerank_limit:Some(field(&request,"budget",64)) };
            let export=request["export_candidates"].as_bool().unwrap_or(false);
            if export && request["kind"]!="diagnostic" {return Err(invalid("候选导出仅允许diagnostic请求").into());}
            let rerank_slots=export.then(||Mutex::new(vec![Vec::<usize>::new();query.len()/dim]));
            let io_accounting=request["io_accounting"].as_bool().unwrap_or(false);
            if io_accounting && (request["kind"]!="diagnostic" || threads!=1 || cache_bytes!=0) {
                return Err(invalid("逐查询I/O计账只允许单线程零缓存diagnostic").into());
            }
            let one = |q: &[f32]| -> io::Result<Vec<(u64,f32)>> {
                let mut slots=Vec::new();
                let query_stats=local_metrics.then(direct_int8::Stats::default);
                let active_stats=query_stats.as_ref().unwrap_or(scoring_stats.as_ref());
                let result=
                if let Some(vectors) = &warm_vectors {
                    Ok(index.search(q, |slot, buffer| { if export {slots.push(slot);}buffer.clear();buffer.extend_from_slice(vectors.get(slot).unwrap());true }, &search))
                } else if let Some(mapping)=&warm_int8 {
                    let mut source=int8_codec::WarmSource {map:mapping,n,slots:if export {Some(&mut slots)}else{None},mode:score_mode,query:None,stats:active_stats,layout};
                    index.search_ssd(q,&mut source,&search)
                } else {
                    let mut source = QuerySource { engine:engine.unwrap(),backend,n,dim,row_stride,encoding,mode:score_mode,query:None,stats:active_stats,layout,
                        slots:if export {Some(&mut slots)}else{None},depth,every,handles:HashMap::new(),local_page_hits:0,stream_hint_counts:request["kind"]=="stream" };
                    index.search_ssd(q,&mut source,&search)
                };
                if let Some(local)=query_stats.as_ref() {
                    let counts=local.snapshot();
                    if counts[0]>0 {add(&scoring_stats.decoded,counts[0]);}
                    if counts[1]>0 {add(&scoring_stats.direct,counts[1]);}
                    if counts[2]>0 {add(&scoring_stats.queries,counts[2]);}
                }
                if io_accounting {if let Some(engine)=engine {engine.wait_idle();engine.check_error()?;}}
                if let Some(record)=&rerank_slots {
                    let qid=(q.as_ptr() as usize-query.as_ptr() as usize)/(std::mem::size_of::<f32>()*dim);
                    record.lock().unwrap()[qid]=slots;
                }
                result
            };
            let batch = || pool.install(|| query.par_chunks_exact(dim).map(one).collect::<io::Result<Vec<_>>>());
            let kind = request["kind"].as_str().unwrap_or("diagnostic");
            if kind == "throughput" { std::hint::black_box(batch()?); if let Some(engine) = engine { engine.wait_idle(); } }
            let stats_before = engine.map(|engine|engine.snapshot());
            let os_before = fs::read_to_string("/proc/self/io")?;
            let score_before=scoring_stats.snapshot();
            let native_start=uring::stamp()?;
            let begin = Instant::now(); let mut latencies = Vec::new(); let mut count = 0;
            let mut stream_windows:Option<Vec<Value>>=None;
            let neighbors = if kind=="stream" {
                let stream=run_stream(&pool,&query,dim,&one,engine,&scoring_stats)?;
                count=stream.neighbors.len();
                latencies=stream.latencies;
                stream_windows=Some(stream.windows);
                stream.neighbors
            } else if kind == "latency" {
                let rows = pool.install(|| query.par_chunks_exact(dim).map(|q| {
                    let start=Instant::now();one(q).map(|result|(result,start.elapsed().as_nanos() as u64))
                }).collect::<io::Result<Vec<_>>>())?;
                count=rows.len();
                rows.into_iter().map(|(result,ns)|{latencies.push(ns);result}).collect::<Vec<_>>()
            } else if kind == "throughput" {
                loop {
                    let results=batch()?;count+=results.len();std::hint::black_box(&results);
                    if begin.elapsed().as_secs_f64()>=request["seconds"].as_f64().unwrap_or(0.25) { break results; }
                }
            } else if kind == "diagnostic" {
                let results=batch()?;count=results.len();results
            } else { return Err(invalid("SSD入口支持stream、latency、throughput和diagnostic请求").into()); };
            if let Some(engine)=engine { engine.wait_idle(); engine.check_error()?; }
            let native_finish=uring::stamp()?;
            let score_after=scoring_stats.snapshot();
            let elapsed=(native_finish.monotonic_ns-native_start.monotonic_ns) as f64/1e9;
            let process_cpu_ns=native_finish.cpu_ns-native_start.cpu_ns;
            let native_interval=json!({"monotonic_start_ns":native_start.monotonic_ns,
                "monotonic_finish_ns":native_finish.monotonic_ns,"process_cpu_start_ns":native_start.cpu_ns,
                "process_cpu_finish_ns":native_finish.cpu_ns});
            let stats_total=engine.map(|engine|engine.snapshot());
            let stats=stats_before.as_ref().zip(stats_total.as_ref()).map(|(before,after)|delta(before,after));
            Ok(json!({"labels":neighbors.iter().map(|r|r.iter().map(|p|p.0-1).collect::<Vec<_>>()).collect::<Vec<_>>(),
                "score_bits":neighbors.iter().map(|r|r.iter().map(|p|p.1.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>(),
                "request":request,"queries_executed":count,"seconds":elapsed,"latency_ns":latencies,
                "process_cpu_ns":process_cpu_ns,"native_interval":native_interval,
                "stream_windows":stream_windows,"stream_window_queries":if kind=="stream" {1000}else{0},
                "stream_window_observation_included_in_total":kind=="stream",
                "stream_window_native_scope":"窗口查询调度、查询执行及最终I/O排空；广播、快照与窗口JSON在窗口native外但在顶层native内",
                "stream_cache_return_definition":"仅hint调用Engine.request实际返回的Ready/(Ready+Pending)，Pending包含新提交、在途合并及完成尚未移出pending的返回",
                "io_block_bytes":PAGE,"score_mode":score_mode.name(),"scoring_kernel":score_mode.kernel(),
                "decoded_vector_count":score_after[0]-score_before[0],"direct_dot_count":score_after[1]-score_before[1],
                "query_quantization_count":score_after[2]-score_before[2],
                "rerank_slots":rerank_slots.as_ref().map(|slots|slots.lock().unwrap().clone()),
                "io_shards":if engine.is_some(){io_shards}else{0},
                "backend":backend.name(),"io_config":io_config,"io_stats":stats,"io_stats_total":stats_total,
                "input_file_residency":file_residency(basepath)?,
        "cold_file_residency":file_residency(coldpath)?,"cold_path":coldpath.to_string_lossy(),
        "cold_row_stride_bytes":row_stride,"cold_storage_bytes":cold_storage_bytes,
        "logical_fp32_bytes":logical_fp32_bytes,"cold_padding_bytes":cold_storage_bytes-n*encoding.payload(dim,layout),
        "cold_encoding":encoding.name(),"cold_payload_bytes":encoding.payload(dim,layout),"cold_codec":layout.codec(),
        "int8_group_size":layout.group_size,"int8_norm_correction":layout.norm_correction,
        "norm_factor_offset_bytes":816,"ls_factor_offset_bytes":820,
        "selected_factor_offset_bytes":layout.factor_offset(),"int8_scale_mode":layout.scale_mode.name(),
        "int8_query_mode":layout.query_mode.name(),"query_quantization":layout.query_mode.quantization(),
        "encoding_variant":c["encoding_variant"].as_str().unwrap_or("unspecified"),
                "residency_before":Value::Null,"residency_after":warm_vectors.as_ref().map(|v|residency(v,n,dim)),
                "cold_query_residencies":[],"io_before":os_before,"io_after":fs::read_to_string("/proc/self/io")?,
                "allocator_after":allocator(),"status_after":fs::read_to_string("/proc/self/status")?,"stat_after":fs::read_to_string("/proc/self/stat")?}))
        })();
        match result {
            Ok(response)=>println!("{}",response),
            Err(error)=>{
                println!("{}",json!({"request":request,"error":error.to_string(),"labels":[],"score_bits":[],"queries_executed":0,"seconds":0.,"latency_ns":[],"process_cpu_ns":0,"native_interval":null,
        "cold_file_residency":file_residency(coldpath).ok(),"cold_path":coldpath.to_string_lossy(),
        "cold_row_stride_bytes":row_stride,"cold_storage_bytes":cold_storage_bytes,
        "logical_fp32_bytes":logical_fp32_bytes,"cold_padding_bytes":cold_storage_bytes-n*encoding.payload(dim,layout),
        "cold_encoding":encoding.name(),"cold_payload_bytes":encoding.payload(dim,layout),"cold_codec":layout.codec(),
        "int8_group_size":layout.group_size,"int8_norm_correction":layout.norm_correction,
        "norm_factor_offset_bytes":816,"ls_factor_offset_bytes":820,
        "selected_factor_offset_bytes":layout.factor_offset(),"int8_scale_mode":layout.scale_mode.name(),
        "int8_query_mode":layout.query_mode.name(),"query_quantization":layout.query_mode.quantization(),
        "encoding_variant":c["encoding_variant"].as_str().unwrap_or("unspecified"),
        "input_file_residency":file_residency(basepath).ok(),"io_stats_total":engine.map(|e|e.snapshot())}));
                io::stdout().flush()?;
                return Err(error);
            }
        }
        io::stdout().flush()?;
    }
    drop(runtime);
    Ok(())
}
