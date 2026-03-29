use crate::VectorType;
use crate::error::{Result, TriviumError};
#[cfg(not(feature = "hnsw"))]
use crate::index::brute_force;
#[cfg(feature = "hnsw")]
use crate::index::hnsw::HnswIndex;
use crate::node::{NodeId, SearchHit};
use crate::storage::compaction::CompactionThread;
use crate::storage::file_format;
use crate::storage::memtable::MemTable;
use crate::storage::wal::{SyncMode, Wal, WalEntry};
use fs2::FileExt;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StorageMode {
    /// Mmap 分离模式（默认）：高性能、海量数据，产生 `.tdb` 和 `.vec` 两个文件
    #[default]
    Mmap,
    /// Rom 单文件模式：高便携性，所有数据保存在一个 `.tdb` 文件中（纯内存加载以获得极高并发）
    Rom,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub dim: usize,
    pub sync_mode: SyncMode,
    pub storage_mode: StorageMode,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dim: 1536,
            sync_mode: SyncMode::default(),
            storage_mode: StorageMode::default(),
        }
    }
}

/// 基于外部参数化配置的查询配置参数矩阵
#[derive(Debug, Clone, Copy)]
pub struct SearchConfig {
    pub top_k: usize,
    pub expand_depth: usize,
    pub min_score: f32,
    pub teleport_alpha: f32, // L6 PPR 阻尼因子/回家概率

    // 认知层统开开关 (当为 false 时，管线完全退化为最极简的传统检索引擎)
    pub enable_advanced_pipeline: bool,

    // L4 / L5: 残差与二次搜索
    pub enable_sparse_residual: bool,
    pub fista_lambda: f32,
    pub fista_threshold: f32,

    // L9: DPP
    pub enable_dpp: bool,
    pub dpp_quality_weight: f32,

    // --- 高级认知选项 (完全 Opt-in) ---
    /// 启用时，将使用 `1.0 / (1.0 + log10(in_degree))` 对泛化扩散节点施加反向惩罚
    pub enable_inverse_inhibition: bool,
    /// 当 > 0 时，作为侧向抑制起保护作用，自动截断扩散网络 (如传入 5000)
    pub lateral_inhibition_threshold: usize,
    /// 是否启用 L1 Binary Quantization 两段式初筛管线 (极速混沌轨道)
    pub enable_bq_coarse_search: bool,
    /// BQ 粗筛候选集占总数据量的比例
    pub bq_candidate_ratio: f32,

    // --- 混合倒排与文本检索 (Hybrid Search) ---
    /// 启用文本混合查询时，决定文本匹配的分数提权倍率 (Boost)
    pub text_boost: f32,
    /// 开启基于 AC 自动机的强制文本召回锚点机制 (等价于 PEDSA第一阶段)
    pub enable_text_hybrid_search: bool,
    pub bm25_k1: f32,
    pub bm25_b: f32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            expand_depth: 2,
            min_score: 0.1,
            teleport_alpha: 0.0,
            enable_advanced_pipeline: false,
            enable_sparse_residual: false,
            fista_lambda: 0.1,
            fista_threshold: 0.30,
            enable_dpp: false,
            dpp_quality_weight: 1.0,
            enable_inverse_inhibition: false,
            lateral_inhibition_threshold: 0,
            enable_bq_coarse_search: false,
            bq_candidate_ratio: 0.1,
            text_boost: 1.5,
            enable_text_hybrid_search: false,
            bm25_k1: 1.2,
            bm25_b: 0.75,
        }
    }
}

/// 安全获取 Mutex 锁：如果锁中毒（某个线程 panic 持有锁），
/// 则恢复内部数据继续运行，而不是 panic 整个进程。
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("Mutex was poisoned, recovering...");
        poisoned.into_inner()
    })
}

/// 数据库核心入口实例
pub struct Database<T: VectorType> {
    db_path: String,
    memtable: Arc<Mutex<MemTable<T>>>,
    wal: Arc<Mutex<Wal>>,
    #[cfg(feature = "hnsw")]
    hnsw_index: HnswIndex<T>,
    compaction: Option<CompactionThread>,
    /// 文件锁：防止多进程同时打开同一个数据库
    _lock_file: std::fs::File,
    /// 内存上限（字节），0 = 无限制
    memory_limit: usize,
    /// 存储模式
    storage_mode: StorageMode,
}

impl<T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Database<T> {
    /// 打开或创建数据库（默认：Mmap 模式，SyncMode::Normal）
    pub fn open(path: &str, dim: usize) -> Result<Self> {
        let config = Config {
            dim,
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// 打开或创建数据库，指定 WAL 同步模式 (向后兼容)
    pub fn open_with_sync(path: &str, dim: usize, sync_mode: SyncMode) -> Result<Self> {
        let config = Config {
            dim,
            sync_mode,
            ..Default::default()
        };
        Self::open_with_config(path, config)
    }

    /// 打开或创建数据库（高级配置入口）
    pub fn open_with_config(path: &str, config: Config) -> Result<Self> {
        let dim = config.dim;
        // ═══ 自动递归创建上层目录 ═══
        if let Some(parent_dir) = std::path::Path::new(path).parent() {
            if !parent_dir.as_os_str().is_empty() {
                std::fs::create_dir_all(parent_dir)?;
            }
        }

        // ═══ 文件锁：防止多进程并发写同一个数据库 ═══
        let lock_path = format!("{}.lock", path);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&lock_path)?;
        lock_file.try_lock_exclusive().map_err(|_| {
            TriviumError::Generic(format!(
                "Database '{}' is already opened by another process. \
                 If this is unexpected, delete '{}'",
                path, lock_path
            ))
        })?;

        let mut memtable = if std::path::Path::new(path).exists() {
            file_format::load(path, config.storage_mode)?
        } else {
            MemTable::new(dim)
        };

        if Wal::needs_recovery(path) {
            let entries = Wal::read_entries::<T>(path)?;
            if !entries.is_empty() {
                tracing::info!("Recovering {} entries from WAL...", entries.len());
                for entry in entries {
                    replay_entry(&mut memtable, entry);
                }
            }
        }

        // 从已有 payload 自动重建 TextIndex（解决重启后文本检索退化的问题）
        memtable.rebuild_text_index_from_payloads();

        let wal = Wal::open_with_sync(path, config.sync_mode)?;

        #[cfg(feature = "hnsw")]
        let hnsw_index = HnswIndex::<T>::new(dim);

        Ok(Self {
            db_path: path.to_string(),
            memtable: Arc::new(Mutex::new(memtable)),
            wal: Arc::new(Mutex::new(wal)),
            #[cfg(feature = "hnsw")]
            hnsw_index,
            compaction: None,
            _lock_file: lock_file,
            memory_limit: 0,
            storage_mode: config.storage_mode,
        })
    }

    /// 运行时切换 WAL 同步模式
    pub fn set_sync_mode(&mut self, mode: SyncMode) {
        let mut w = lock_or_recover(&self.wal);
        w.set_sync_mode(mode);
    }

    /// 设置内存上限（字节）
    ///
    /// 当 MemTable 估算内存超过此值时，写操作后会自动触发 flush 落盘。
    /// 设为 0 表示无限制（默认）。
    ///
    /// 推荐值：
    ///   - 256 MB = 256 * 1024 * 1024
    ///   - 1 GB   = 1024 * 1024 * 1024
    pub fn set_memory_limit(&mut self, bytes: usize) {
        self.memory_limit = bytes;
    }

    /// 查询当前 MemTable 估算内存占用（字节）
    pub fn estimated_memory(&self) -> usize {
        lock_or_recover(&self.memtable).estimated_memory_bytes()
    }

    /// 内部方法：检查内存压力，超出上限时自动 flush
    fn check_memory_pressure(&mut self) {
        if self.memory_limit > 0 {
            let usage = lock_or_recover(&self.memtable).estimated_memory_bytes();
            if usage > self.memory_limit {
                tracing::info!(
                    "Memory pressure: {}MB > limit {}MB. Auto-flushing...",
                    usage / (1024 * 1024),
                    self.memory_limit / (1024 * 1024)
                );
                if let Err(e) = self.flush() {
                    tracing::error!("Auto-flush failed: {}", e);
                }
            }
        }
    }

    /// 启动后台自动 Compaction 线程
    pub fn enable_auto_compaction(&mut self, interval: Duration) {
        self.compaction.take();
        let ct = CompactionThread::spawn(
            interval,
            Arc::clone(&self.memtable),
            Arc::clone(&self.wal),
            self.db_path.clone(),
            self.storage_mode,
        );
        self.compaction = Some(ct);
    }

    pub fn disable_auto_compaction(&mut self) {
        self.compaction.take();
    }

    // ════════ 写操作 ════════

    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) -> Result<NodeId> {
        let id = {
            let mut mt = lock_or_recover(&self.memtable);
            mt.insert(vector, payload.clone())?
        };
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Insert {
                id,
                vector: vector.to_vec(),
                payload: payload.to_string(),
            })?;
        }
        self.check_memory_pressure();
        Ok(id)
    }

    pub fn insert_with_id(
        &mut self,
        id: NodeId,
        vector: &[T],
        payload: serde_json::Value,
    ) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.insert_with_id(id, vector, payload.clone())?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Insert {
                id,
                vector: vector.to_vec(),
                payload: payload.to_string(),
            })?;
        }
        self.check_memory_pressure();
        Ok(())
    }

    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.link(src, dst, label.to_string(), weight)?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Link::<T> {
                src,
                dst,
                label: label.to_string(),
                weight,
            })?;
        }
        Ok(())
    }

    pub fn delete(&mut self, id: NodeId) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.delete(id)?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Delete::<T> { id })?;
        }
        Ok(())
    }

    pub fn unlink(&mut self, src: NodeId, dst: NodeId) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.unlink(src, dst)?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::Unlink::<T> { src, dst })?;
        }
        Ok(())
    }

    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.update_payload(id, payload.clone())?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::UpdatePayload::<T> {
                id,
                payload: payload.to_string(),
            })?;
        }
        Ok(())
    }

    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) -> Result<()> {
        {
            let mut mt = lock_or_recover(&self.memtable);
            mt.update_vector(id, vector)?;
        }
        {
            let mut w = lock_or_recover(&self.wal);
            w.append(&WalEntry::UpdateVector::<T> {
                id,
                vector: vector.to_vec(),
            })?;
        }
        Ok(())
    }

    // ════════ 读操作 ════════

    pub fn index_keyword(&mut self, id: NodeId, keyword: &str) -> Result<()> {
        let mut mt = lock_or_recover(&self.memtable);
        mt.index_keyword(id, keyword);
        Ok(())
    }

    pub fn index_text(&mut self, id: NodeId, text: &str) -> Result<()> {
        let mut mt = lock_or_recover(&self.memtable);
        mt.index_text(id, text);
        Ok(())
    }

    pub fn build_text_index(&mut self) -> Result<()> {
        let mut mt = lock_or_recover(&self.memtable);
        mt.build_text_index();
        Ok(())
    }

    pub fn get_payload(&self, id: NodeId) -> Option<serde_json::Value> {
        let mt = lock_or_recover(&self.memtable);
        mt.get_payload(id).cloned()
    }

    pub fn get_edges(&self, id: NodeId) -> Vec<crate::node::Edge> {
        let mt = lock_or_recover(&self.memtable);
        mt.get_edges(id).map(|e| e.to_vec()).unwrap_or_default()
    }

    pub fn get_all_ids(&self) -> Vec<NodeId> {
        let mt = lock_or_recover(&self.memtable);
        mt.get_all_ids() // 需要在 memtable.rs 补充
    }

    pub fn search(
        &self,
        query_vector: &[T],
        top_k: usize,
        expand_depth: usize,
        min_score: f32,
    ) -> Result<Vec<SearchHit>> {
        let config = SearchConfig {
            top_k,
            expand_depth,
            min_score,
            enable_advanced_pipeline: false,
            ..Default::default()
        };
        self.search_hybrid(None, Some(query_vector), &config)
    }

    pub fn search_advanced(
        &self,
        query_vector: &[T],
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        self.search_hybrid(None, Some(query_vector), config)
    }

    /// 全能混合检索核心引擎 (Hybrid Advanced Pipeline)
    /// 包含文本稀疏索引 + 稠密连续向量空间 + 图谱数学约束的真正完全体检索引擎
    pub fn search_hybrid(
        &self,
        query_text: Option<&str>,
        query_vector: Option<&[T]>,
        config: &SearchConfig,
    ) -> Result<Vec<SearchHit>> {
        let mut mt = lock_or_recover(&self.memtable);

        // --- 0. 容错与防御式编程 (Sanity Checks) ---
        let dim = mt.dim();
        if let Some(qv) = query_vector {
            if qv.len() != dim {
                return Err(crate::error::TriviumError::DimensionMismatch {
                    expected: dim,
                    got: qv.len(),
                });
            }
            for item in qv {
                let f = item.to_f32();
                if f.is_nan() || f.is_infinite() {
                    return Err(crate::error::TriviumError::Generic(
                        "Query vector contains NaN or Infinity".to_string(),
                    ));
                }
            }
        }

        // 隔离作用域：强行钳平越界的玄学配置参数，防止底层矩阵求解 Panic 或死循环
        let mut safe_cfg = *config;
        safe_cfg.top_k = safe_cfg.top_k.max(1);
        safe_cfg.fista_lambda = safe_cfg.fista_lambda.clamp(1e-5, 100.0); // 惩罚太小等于没正则，太大全变0
        safe_cfg.teleport_alpha = safe_cfg.teleport_alpha.clamp(0.0, 1.0); // 必须在 0 - 1 的概率之间
        safe_cfg.dpp_quality_weight = safe_cfg.dpp_quality_weight.clamp(0.0, 10.0); // 幂次太高会导致 float 溢出
        safe_cfg.fista_threshold = safe_cfg.fista_threshold.clamp(0.0, f32::MAX);
        safe_cfg.bq_candidate_ratio = safe_cfg.bq_candidate_ratio.clamp(0.0, 1.0);

        let config = &safe_cfg; // 复用下文变量名

        // --- L1 & L2 混合检索 (文本 + 向量初排 / NMF) ---
        let mut anchor_hits: Vec<SearchHit> = Vec::new();
        let mut seed_map: std::collections::HashMap<NodeId, f32> = std::collections::HashMap::new();

        // 1. 如果有文本且开启文本引擎
        if config.enable_text_hybrid_search {
            if let Some(txt) = query_text {
                let text_engine = mt.text_engine();
                // 1.1 精准 AC 命中
                let ac_hits = text_engine.search_ac(txt);
                for (id, score) in ac_hits {
                    *seed_map.entry(id).or_insert(0.0) += score * config.text_boost; // 高优先权权重
                }

                // 1.2 大段 BM25 兜底
                let bm25_hits = text_engine.search_bm25(txt, config.bm25_k1, config.bm25_b);
                for (id, score) in bm25_hits {
                    // Normalize the score somewhat
                    let normalized_score = (score / 10.0).clamp(0.0, 1.0) * config.text_boost;
                    *seed_map.entry(id).or_insert(0.0) += normalized_score;
                }
            }
        }

        // 2. 如果存在向量查询，进入数学多层筛选管线
        if let Some(query_vector) = query_vector {
            #[cfg(not(feature = "hnsw"))]
            let vector_hits: Vec<SearchHit> = {
                let dim = mt.dim();
                mt.ensure_vectors_cache();
                let vectors = mt.flat_vectors();

                if config.enable_bq_coarse_search {
                    // --- BQ 混沌检索分支: L1 1-bit Hamming 粗排 ---
                    let q_bq = crate::index::bq::BqSignature::from_vector(query_vector);
                    // 必须扫描全部内部槽位（含 tombstone），而非活跃节点数
                    // 否则删除节点后尾部活跃节点会被漏扫
                    let slot_count = mt.internal_slot_count();
                    let candidate_cnt = (((mt.node_count() as f32) * config.bq_candidate_ratio)
                        .ceil() as usize)
                        .max(config.top_k);

                    let mut bq_scores: Vec<(usize, u32)> = (0..slot_count)
                        .filter(|&i| mt.get_id_by_index(i) != 0) // 跳过 tombstone 槽位
                        .filter_map(|i| {
                            mt.get_bq_signature(i)
                                .map(|sig| (i, sig.hamming_distance(&q_bq)))
                        })
                        .collect();

                    // Hamming 距离越小越好（非严格 Top-K 可以不用完全排序，这里为保证精度采用完全排序后截断）
                    bq_scores.sort_unstable_by_key(|&(_, dist)| dist);
                    bq_scores.truncate(candidate_cnt);

                    // --- BQ 混沌检索分支: L2 f32/f16 Cosine 精排 ---
                    // 这里重用 brute_force 的单次距离点积（由于已经在 cache 里了，直接对这些 index 切片）
                    let mut refined = Vec::with_capacity(candidate_cnt);
                    for (i, _dist) in bq_scores {
                        let offset = i * dim;
                        // 这里有个隐患如果 offset 越界。因为 m_count 包含了被删除节点空洞。
                        // 但底层 vec_pool 其实能容纳所有的内部 index。
                        if offset + dim <= vectors.len() {
                            let score = T::similarity(query_vector, &vectors[offset..offset + dim]);
                            if score >= config.min_score {
                                refined.push(SearchHit {
                                    id: mt.get_id_by_index(i),
                                    score,
                                    payload: serde_json::Value::Null, // 暂时置空，最后统一组装 payload
                                });
                            }
                        }
                    }
                    refined.sort_by(|a, b| {
                        b.score
                            .partial_cmp(&a.score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    refined.truncate(config.top_k);

                    // 补充 Payload
                    for hit in &mut refined {
                        if let Some(p) = mt.get_payload(hit.id) {
                            hit.payload = p.clone();
                        }
                    }
                    refined
                } else {
                    // 原生基础全局爆搜
                    brute_force::search(
                        query_vector,
                        vectors,
                        dim,
                        config.top_k,
                        config.min_score,
                        |idx| mt.get_id_by_index(idx),
                    )
                }
            };
            #[cfg(feature = "hnsw")]
            let vector_hits: Vec<SearchHit> = {
                self.hnsw_index
                    .search(query_vector, config.top_k, config.min_score)
            };

            for hit in vector_hits {
                *seed_map.entry(hit.id).or_insert(0.0) += hit.score;
            }

            if config.enable_advanced_pipeline {
                if config.enable_sparse_residual && !seed_map.is_empty() {
                    // --- L4 FISTA Sparse Residual ---
                    let entity_vecs: Vec<Vec<f32>> = seed_map
                        .keys()
                        .filter_map(|&id| {
                            mt.get_vector(id)
                                .map(|v| v.iter().map(|&x| x.to_f32()).collect())
                        })
                        .collect();
                    let q_f32: Vec<f32> = query_vector.iter().map(|&x| x.to_f32()).collect();

                    let (_, residual, residual_norm) = crate::cognitive::fista_solve(
                        &q_f32,
                        &entity_vecs,
                        config.fista_lambda,
                        80,
                    );

                    // --- L5 Shadow Query ---
                    if residual_norm > config.fista_threshold {
                        tracing::debug!(
                            "FISTA Residual magnitude high ({} > {}). Triggering Shadow Query.",
                            residual_norm,
                            config.fista_threshold
                        );
                        let r_orig: Vec<T> = residual.iter().map(|&x| T::from_f32(x)).collect();
                        let shadow_hits: Vec<SearchHit> = {
                            #[cfg(not(feature = "hnsw"))]
                            {
                                let dim = mt.dim();
                                brute_force::search(
                                    &r_orig,
                                    mt.flat_vectors(),
                                    dim,
                                    config.top_k,
                                    config.min_score,
                                    |idx| mt.get_id_by_index(idx),
                                )
                            }
                            #[cfg(feature = "hnsw")]
                            {
                                self.hnsw_index
                                    .search(&r_orig, config.top_k, config.min_score)
                            }
                        };

                        for sh in shadow_hits {
                            *seed_map.entry(sh.id).or_insert(0.0) += sh.score * 0.8; // 影子抑制衰减
                        }
                    }
                }
            }
        }

        // 将混合融合收集的所有 seed_map 对象转换为 `anchor_hits` 进入后处理
        for (id, score) in seed_map {
            if score >= config.min_score {
                let payload = mt
                    .get_payload(id)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                anchor_hits.push(SearchHit { id, score, payload });
            }
        }
        anchor_hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        anchor_hits.truncate(config.top_k.max(15)); // 不要在此处把图种子提前卡死

        // 没搜到任何东西直接短路返回
        if anchor_hits.is_empty() {
            return Ok(vec![]);
        }

        if anchor_hits.is_empty() {
            return Ok(Vec::new());
        }

        let mut seeds = Vec::with_capacity(anchor_hits.len());
        for mut hit in anchor_hits {
            if let Some(payload) = mt.get_payload(hit.id) {
                hit.payload = payload.clone();
                seeds.push(hit);
            }
        }

        // --- L6 & L7: PPR 结合内向原生边漫游 ---
        let mut expanded = crate::graph::traversal::expand_graph(
            &mt,
            seeds,
            config.expand_depth,
            config.teleport_alpha,
            config.enable_inverse_inhibition,
            config.lateral_inhibition_threshold,
        );

        // L8 (时间衰减与多维重排) 已被设计哲学剥离：不应在此侵入 JSON 业务字段，交由上层 Agent 侧处理。

        if config.enable_advanced_pipeline && config.enable_dpp && expanded.len() > config.top_k {
            let limit = config.top_k;
            let dpp_pool_size = std::cmp::min(expanded.len(), limit * 3);
            let mut pool_vecs = Vec::with_capacity(dpp_pool_size);
            let mut pool_scores = Vec::with_capacity(dpp_pool_size);
            let mut pool_valid = Vec::with_capacity(dpp_pool_size);

            for i in 0..dpp_pool_size {
                let hit = &expanded[i];
                if let Some(v) = mt.get_vector(hit.id) {
                    pool_vecs.push(v.iter().map(|&x| x.to_f32()).collect());
                    pool_scores.push(hit.score);
                    pool_valid.push(hit.clone());
                }
            }

            if pool_valid.len() > limit {
                let selected_idx = crate::cognitive::dpp_greedy(
                    &pool_vecs,
                    &pool_scores,
                    limit,
                    config.dpp_quality_weight,
                );

                let mut final_results = Vec::with_capacity(limit);
                for &idx in &selected_idx {
                    final_results.push(pool_valid[idx].clone());
                }
                final_results.sort_by(|a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                return Ok(final_results);
            }
        }

        expanded.truncate(config.top_k);
        Ok(expanded)
    }

    pub fn get(&self, id: NodeId) -> Option<crate::node::NodeView<T>> {
        let mt = lock_or_recover(&self.memtable);
        let payload = mt.get_payload(id)?.clone();
        let vector = mt.get_vector(id)?.to_vec();
        let edges = mt.get_edges(id).unwrap_or(&[]).to_vec();
        Some(crate::node::NodeView {
            id,
            vector,
            payload,
            edges,
        })
    }

    pub fn neighbors(&self, id: NodeId, depth: usize) -> Vec<NodeId> {
        use std::collections::{HashSet, VecDeque};
        let mt = lock_or_recover(&self.memtable);
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        visited.insert(id);
        queue.push_back((id, 0usize));
        while let Some((curr, d)) = queue.pop_front() {
            if d >= depth {
                continue;
            }
            if let Some(edges) = mt.get_edges(curr) {
                for edge in edges {
                    if visited.insert(edge.target_id) {
                        queue.push_back((edge.target_id, d + 1));
                    }
                }
            }
        }
        visited.remove(&id);
        visited.into_iter().collect()
    }

    pub fn filter(&self, key: &str, value: &serde_json::Value) -> Vec<crate::node::NodeView<T>> {
        let mt = lock_or_recover(&self.memtable);
        let mut results = Vec::new();
        for nid in mt.all_node_ids() {
            if let Some(payload) = mt.get_payload(nid) {
                if payload.get(key) == Some(value) {
                    let vector = mt.get_vector(nid).unwrap_or(&[]).to_vec();
                    let edges = mt.get_edges(nid).unwrap_or(&[]).to_vec();
                    results.push(crate::node::NodeView {
                        id: nid,
                        vector,
                        payload: payload.clone(),
                        edges,
                    });
                }
            }
        }
        results
    }

    pub fn filter_where(&self, condition: &crate::filter::Filter) -> Vec<crate::node::NodeView<T>> {
        let mt = lock_or_recover(&self.memtable);
        let mut results = Vec::new();
        for nid in mt.all_node_ids() {
            if let Some(payload) = mt.get_payload(nid) {
                if condition.matches(payload) {
                    let vector = mt.get_vector(nid).unwrap_or(&[]).to_vec();
                    let edges = mt.get_edges(nid).unwrap_or(&[]).to_vec();
                    results.push(crate::node::NodeView {
                        id: nid,
                        vector,
                        payload: payload.clone(),
                        edges,
                    });
                }
            }
        }
        results
    }

    /// 将内存数据持久化到磁盘
    ///
    /// 安全顺序（防止崩溃丢数据）：
    ///   1. 原子写入 .tdb（写 .tmp → fsync → rename）
    ///   2. 确认 .tdb 写入成功后，才清除 WAL
    ///
    /// 崩溃场景分析：
    ///   - 步骤 1 中途崩溃 → .tmp 残留但 .tdb 完好 → 重启用旧 .tdb + WAL 回放
    ///   - 步骤 1 完成、步骤 2 前崩溃 → 新 .tdb 已就绪 + WAL 仍存在 → 重启回放幂等数据（安全冗余）
    ///   - 全部完成 → 干净状态
    pub fn flush(&mut self) -> Result<()> {
        // Step 1: 分层原子写入（根据 mode 决定单文件 .tdb 或 .vec + .tdb）
        {
            let mut mt = lock_or_recover(&self.memtable);
            file_format::save(&mut mt, &self.db_path, self.storage_mode)?;
        }
        // Step 2: .tdb 已安全落盘，现在清除 WAL
        {
            let mut w = lock_or_recover(&self.wal);
            w.clear()?;
        }
        Ok(())
    }

    pub fn close(mut self) -> Result<()> {
        self.disable_auto_compaction();
        self.flush()
    }

    pub fn node_count(&self) -> usize {
        lock_or_recover(&self.memtable).node_count()
    }
    pub fn contains(&self, id: NodeId) -> bool {
        lock_or_recover(&self.memtable).contains(id)
    }
    pub fn dim(&self) -> usize {
        lock_or_recover(&self.memtable).dim()
    }

    /// 获取所有活跃节点的 ID 列表
    pub fn all_node_ids(&self) -> Vec<NodeId> {
        lock_or_recover(&self.memtable).all_node_ids()
    }

    /// 重建 HNSW 向量索引
    ///
    /// 仅在启用 `hnsw` feature 时有效。BruteForce 模式下调用此方法为 no-op。
    /// 通常在批量插入完成后调用一次，以构建高效的近似搜索索引。
    pub fn rebuild_index(&mut self) {
        #[cfg(feature = "hnsw")]
        {
            let mut mt = lock_or_recover(&self.memtable);
            let dim = mt.dim();
            mt.ensure_vectors_cache();
            let flat = mt.flat_vectors();
            self.hnsw_index.rebuild(
                flat,
                dim,
                |idx| mt.get_id_by_index(idx),
                |idx| {
                    let nid = mt.get_id_by_index(idx);
                    mt.contains(nid)
                },
            );
            tracing::info!("HNSW 索引重建完成，共 {} 个活跃节点", mt.node_count());
        }
        #[cfg(not(feature = "hnsw"))]
        {
            tracing::debug!("未启用 HNSW feature，rebuild_index 为 no-op");
        }
    }

    /// 维度迁移：从当前数据库导出所有节点和边到一个新维度的数据库。
    ///
    /// 向量数据需要外部重新生成（因为维度变了，旧向量无法直接复用）。
    /// 此方法会：
    ///   1. 以 placeholder 零向量创建新库中的所有节点（保留原 ID 和 Payload）
    ///   2. 复制所有图谱边关系
    ///   3. 返回新数据库实例，用户随后需要调用 update_vector 逐个更新向量
    ///
    /// # 返回
    /// `(new_db, node_ids)` — 新数据库实例和需要更新向量的节点 ID 列表
    pub fn migrate_to(&self, new_path: &str, new_dim: usize) -> Result<(Database<T>, Vec<NodeId>)>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let mt = lock_or_recover(&self.memtable);
        let mut node_ids = mt.all_node_ids();
        node_ids.sort();

        // 创建新库
        let mut new_db = Database::<T>::open(new_path, new_dim)?;

        // 迁移所有节点（使用零向量占位，保留 ID 和 Payload）
        let zero_vec = vec![T::zero(); new_dim];
        for &nid in &node_ids {
            if let Some(payload) = mt.get_payload(nid) {
                new_db.insert_with_id(nid, &zero_vec, payload.clone())?;
            }
        }

        // 迁移所有边
        for &nid in &node_ids {
            if let Some(edges) = mt.get_edges(nid) {
                for edge in edges {
                    // 只迁移目标节点也存在的边
                    if mt.get_payload(edge.target_id).is_some() {
                        new_db.link(nid, edge.target_id, &edge.label, edge.weight)?;
                    }
                }
            }
        }

        new_db.flush()?;
        tracing::info!(
            "维度迁移完成: {} → {}，共迁移 {} 个节点",
            mt.dim(),
            new_dim,
            node_ids.len()
        );

        Ok((new_db, node_ids))
    }

    /// 开启一个轻量级事务
    ///
    /// 事务期间所有写操作仅缓冲在内存中，调用 commit() 后原子性写入。
    /// 如果 commit() 中途任一操作失败，已应用的部分不会回滚
    /// （WAL 回放可在重启后修正一致性）。
    ///
    /// 用法：
    /// ```rust,ignore
    /// let mut tx = db.begin_tx();
    /// tx.insert(&vec1, payload1);
    /// tx.insert(&vec2, payload2);
    /// tx.link(1, 2, "knows", 1.0);
    /// let ids = tx.commit()?;  // 原子提交，返回生成的 ID
    /// ```
    pub fn begin_tx(&mut self) -> Transaction<'_, T> {
        Transaction {
            db: self,
            ops: Vec::new(),
            committed: false,
        }
    }

    /// 执行类 Cypher 图谱查询语句，返回匹配到的变量绑定集合。
    ///
    /// 语法示例：
    /// ```text
    /// MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b
    /// MATCH (a {id: 1})-[]->(b) RETURN b
    /// ```
    pub fn query(
        &self,
        cypher: &str,
    ) -> Result<Vec<std::collections::HashMap<String, crate::node::NodeView<T>>>> {
        let ast = crate::query::parser::parse(cypher)
            .map_err(|e| crate::error::TriviumError::Generic(format!("查询语句解析失败: {}", e)))?;
        let mt = lock_or_recover(&self.memtable);
        Ok(crate::query::executor::execute(&ast, &mt))
    }
}

/// 安全析构：确保 WAL BufWriter 的缓冲数据在 Database 被 drop 时显式落盘。
///
/// 没有这个 Drop 实现，Arc<Mutex<Wal>> 的复杂析构链中 BufWriter 可能
/// 不会可靠地 flush，尤其在 Windows 平台上会导致 WAL 数据静默丢失。
impl<T: VectorType> Drop for Database<T> {
    fn drop(&mut self) {
        // 1. 停止自动压缩线程（避免在析构期间还有后台写入）
        self.compaction.take();

        // 2. 显式 flush WAL BufWriter 到磁盘
        if let Ok(mut w) = self.wal.lock() {
            w.flush_writer();
        }
    }
}
fn replay_entry<T: VectorType>(mt: &mut MemTable<T>, entry: WalEntry<T>) {
    match entry {
        WalEntry::Insert {
            id,
            vector,
            payload,
        } => {
            if mt.contains(id) {
                // 幂等：该 ID 已存在（可能来自 .tdb 加载或重复回放），跳过
                tracing::debug!("WAL 回放跳过已存在的节点 {}", id);
            } else {
                let payload_val: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or_default();
                let _ = mt.raw_insert(id, &vector, payload_val);
            }
            // 无论是否跳过，都必须推进 next_id 防止后续 insert 复用已物化的 ID
            mt.advance_next_id(id + 1);
        }
        WalEntry::Link {
            src,
            dst,
            label,
            weight,
        } => {
            if mt.contains(src) && mt.contains(dst) {
                let _ = mt.link(src, dst, label, weight);
            }
        }
        WalEntry::Delete { id } => {
            if mt.contains(id) {
                let _ = mt.delete(id);
            }
        }
        WalEntry::Unlink { src, dst } => {
            if mt.contains(src) {
                let _ = mt.unlink(src, dst);
            }
        }
        WalEntry::UpdatePayload { id, payload } => {
            if mt.contains(id) {
                let payload_val: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or_default();
                let _ = mt.update_payload(id, payload_val);
            }
        }
        WalEntry::UpdateVector { id, vector } => {
            if mt.contains(id) {
                let _ = mt.update_vector(id, &vector);
            }
        }
        WalEntry::TxBegin { .. } | WalEntry::TxCommit { .. } => {
            // 已在 wal.rs 内的回放过滤环节处理，这里不应再收到，直接忽略
        }
    }
}

// ════════════════════════════════════════════════════════
//  轻量级事务支持
// ════════════════════════════════════════════════════════

/// 事务操作类型（内部缓冲用）
enum TxOp<T> {
    Insert {
        vector: Vec<T>,
        payload: serde_json::Value,
    },
    InsertWithId {
        id: NodeId,
        vector: Vec<T>,
        payload: serde_json::Value,
    },
    Link {
        src: NodeId,
        dst: NodeId,
        label: String,
        weight: f32,
    },
    Delete {
        id: NodeId,
    },
    Unlink {
        src: NodeId,
        dst: NodeId,
    },
    UpdatePayload {
        id: NodeId,
        payload: serde_json::Value,
    },
    UpdateVector {
        id: NodeId,
        vector: Vec<T>,
    },
}

/// 轻量级事务
///
/// 所有操作在 commit() 前仅缓冲在内存中，不会影响数据库状态。
/// - `commit()` → 一次性持有锁，按顺序应用到 memtable + WAL，任何一步失败则回滚
/// - `rollback()` → 丢弃缓冲（或 drop 自动丢弃）
///
/// ```rust,ignore
/// let mut tx = db.begin_tx();
/// tx.insert(&vec, payload);
/// tx.link(1, 2, "knows", 1.0);
/// tx.commit()?;  // 原子提交
/// ```
pub struct Transaction<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> {
    db: &'a mut Database<T>,
    ops: Vec<TxOp<T>>,
    committed: bool,
}

impl<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Transaction<'a, T> {
    /// 缓冲一个插入操作
    pub fn insert(&mut self, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::Insert {
            vector: vector.to_vec(),
            payload,
        });
    }

    /// 缓冲一个带自定义 ID 的插入操作
    pub fn insert_with_id(&mut self, id: NodeId, vector: &[T], payload: serde_json::Value) {
        self.ops.push(TxOp::InsertWithId {
            id,
            vector: vector.to_vec(),
            payload,
        });
    }

    /// 缓冲一个连边操作
    pub fn link(&mut self, src: NodeId, dst: NodeId, label: &str, weight: f32) {
        self.ops.push(TxOp::Link {
            src,
            dst,
            label: label.to_string(),
            weight,
        });
    }

    /// 缓冲一个删除操作
    pub fn delete(&mut self, id: NodeId) {
        self.ops.push(TxOp::Delete { id });
    }

    /// 缓冲一个断边操作
    pub fn unlink(&mut self, src: NodeId, dst: NodeId) {
        self.ops.push(TxOp::Unlink { src, dst });
    }

    /// 缓冲一个更新 payload 操作
    pub fn update_payload(&mut self, id: NodeId, payload: serde_json::Value) {
        self.ops.push(TxOp::UpdatePayload { id, payload });
    }

    /// 缓冲一个更新向量操作
    pub fn update_vector(&mut self, id: NodeId, vector: &[T]) {
        self.ops.push(TxOp::UpdateVector {
            id,
            vector: vector.to_vec(),
        });
    }

    /// 当前事务中缓冲的操作数
    pub fn pending_count(&self) -> usize {
        self.ops.len()
    }

    /// 原子提交事务
    ///
    /// 流程（WAL-first 持久化语义）：
    ///   1. Dry-Run 预检：虚拟状态验证 + 预分配 ID
    ///   2. 构建 WAL 条目（不触碰 memtable）
    ///   3. 先写 WAL（若失败则 memtable 完全未变，安全回滚）
    ///   4. 再应用到 memtable（Infallible，干跑已排除所有异常）
    pub fn commit(mut self) -> Result<Vec<NodeId>> {
        let ops = std::mem::take(&mut self.ops);
        if ops.is_empty() {
            self.committed = true;
            return Ok(Vec::new());
        }

        let mut mt = lock_or_recover(&self.db.memtable);

        // ════════ 第一阶段：预检前置 (Dry-Run) + 预分配 ID ════════
        let mut sim_next_id = mt.next_id_value();
        let dim = mt.dim();
        let mut pending_ids = std::collections::HashSet::new();
        let mut pending_deletes = std::collections::HashSet::new();
        // 记录每个 Insert 操作预分配的 ID（非插入操作为 None）
        let mut pre_assigned_ids: Vec<Option<NodeId>> = Vec::with_capacity(ops.len());

        macro_rules! check_exists {
            ($id:expr) => {
                !pending_deletes.contains($id) && (pending_ids.contains($id) || mt.contains(*$id))
            };
        }

        for op in &ops {
            match op {
                TxOp::Insert { vector, .. } => {
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    pre_assigned_ids.push(Some(sim_next_id));
                    pending_ids.insert(sim_next_id);
                    sim_next_id += 1;
                }
                TxOp::InsertWithId { id, vector, .. } => {
                    if check_exists!(id) {
                        return Err(crate::error::TriviumError::Generic(format!(
                            "Node {} already exists",
                            id
                        )));
                    }
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    pre_assigned_ids.push(Some(*id));
                    pending_ids.insert(*id);
                    if *id >= sim_next_id {
                        sim_next_id = *id + 1;
                    }
                }
                TxOp::Link { src, dst, .. } => {
                    if !check_exists!(src) {
                        return Err(crate::error::TriviumError::NodeNotFound(*src));
                    }
                    if !check_exists!(dst) {
                        return Err(crate::error::TriviumError::NodeNotFound(*dst));
                    }
                    pre_assigned_ids.push(None);
                }
                TxOp::Delete { id } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    pending_deletes.insert(*id);
                    pre_assigned_ids.push(None);
                }
                TxOp::Unlink { src, .. } => {
                    if !check_exists!(src) {
                        return Err(crate::error::TriviumError::NodeNotFound(*src));
                    }
                    pre_assigned_ids.push(None);
                }
                TxOp::UpdatePayload { id, .. } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    pre_assigned_ids.push(None);
                }
                TxOp::UpdateVector { id, vector } => {
                    if !check_exists!(id) {
                        return Err(crate::error::TriviumError::NodeNotFound(*id));
                    }
                    if vector.len() != dim {
                        return Err(crate::error::TriviumError::DimensionMismatch {
                            expected: dim,
                            got: vector.len(),
                        });
                    }
                    pre_assigned_ids.push(None);
                }
            }
        }

        // ════════ 第二阶段：构建 WAL 条目（不触碰 memtable） ════════
        let mut wal_entries: Vec<WalEntry<T>> = Vec::with_capacity(ops.len());
        let mut generated_ids: Vec<NodeId> = Vec::new();

        for (i, op) in ops.iter().enumerate() {
            match op {
                TxOp::Insert { vector, payload } => {
                    let id = pre_assigned_ids[i].unwrap();
                    generated_ids.push(id);
                    wal_entries.push(WalEntry::Insert {
                        id,
                        vector: vector.clone(),
                        payload: payload.to_string(),
                    });
                }
                TxOp::InsertWithId {
                    id,
                    vector,
                    payload,
                } => {
                    generated_ids.push(*id);
                    wal_entries.push(WalEntry::Insert {
                        id: *id,
                        vector: vector.clone(),
                        payload: payload.to_string(),
                    });
                }
                TxOp::Link {
                    src,
                    dst,
                    label,
                    weight,
                } => {
                    wal_entries.push(WalEntry::Link {
                        src: *src,
                        dst: *dst,
                        label: label.clone(),
                        weight: *weight,
                    });
                }
                TxOp::Delete { id } => {
                    wal_entries.push(WalEntry::Delete { id: *id });
                }
                TxOp::Unlink { src, dst } => {
                    wal_entries.push(WalEntry::Unlink {
                        src: *src,
                        dst: *dst,
                    });
                }
                TxOp::UpdatePayload { id, payload } => {
                    wal_entries.push(WalEntry::UpdatePayload {
                        id: *id,
                        payload: payload.to_string(),
                    });
                }
                TxOp::UpdateVector { id, vector } => {
                    wal_entries.push(WalEntry::UpdateVector {
                        id: *id,
                        vector: vector.clone(),
                    });
                }
            }
        }

        // ════════ 第三阶段：先写 WAL（若失败则 memtable 完全未变） ════════
        {
            let mut w = lock_or_recover(&self.db.wal);
            let tx_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            w.append_batch(tx_id, &wal_entries)?;
        }
        // ↑ 如果 WAL 写入失败，函数在此返回 Err，memtable 零损伤

        // ════════ 第四阶段：应用到 memtable（Infallible Apply） ════════
        // Dry-Run 已排除所有业务逻辑异常，此处应用保证不会中途失败
        for entry in wal_entries {
            match entry {
                WalEntry::Insert {
                    id,
                    vector,
                    payload,
                } => {
                    let payload_val: serde_json::Value =
                        serde_json::from_str(&payload).unwrap_or_default();
                    let _ = mt.insert_with_id(id, &vector, payload_val);
                }
                WalEntry::Link {
                    src,
                    dst,
                    label,
                    weight,
                } => {
                    let _ = mt.link(src, dst, label, weight);
                }
                WalEntry::Delete { id } => {
                    let _ = mt.delete(id);
                }
                WalEntry::Unlink { src, dst } => {
                    let _ = mt.unlink(src, dst);
                }
                WalEntry::UpdatePayload { id, payload } => {
                    let payload_val: serde_json::Value =
                        serde_json::from_str(&payload).unwrap_or_default();
                    let _ = mt.update_payload(id, payload_val);
                }
                WalEntry::UpdateVector { id, vector } => {
                    let _ = mt.update_vector(id, &vector);
                }
                _ => {}
            }
        }
        drop(mt); // 释放 memtable 锁

        self.committed = true;
        Ok(generated_ids)
    }

    /// 显式回滚（丢弃所有缓冲操作）
    pub fn rollback(mut self) {
        self.ops.clear();
        self.committed = true; // 标记已处理，防止 Drop 时警告
    }
}

impl<'a, T: VectorType + serde::Serialize + serde::de::DeserializeOwned> Drop
    for Transaction<'a, T>
{
    fn drop(&mut self) {
        if !self.committed && !self.ops.is_empty() {
            tracing::warn!(
                "Transaction with {} pending ops was dropped without commit/rollback. Operations discarded.",
                self.ops.len()
            );
        }
    }
}
