#[cfg(feature = "nodejs")]
pub mod nodejs {
    use crate::database::Database as GenericDatabase;
    use crate::filter::Filter;
    use napi_derive::napi;

    // ════════ 后端枚举：封装三种泛型特化 ════════

    enum DbBackend {
        F32(GenericDatabase<f32>),
        F16(GenericDatabase<half::f16>),
        U64(GenericDatabase<u64>),
    }

    /// 统一分发宏：对三种后端执行相同的表达式
    macro_rules! dispatch {
        ($self:expr, $db:ident => $expr:expr) => {
            match &$self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
        ($self:expr, mut $db:ident => $expr:expr) => {
            match &mut $self.inner {
                DbBackend::F32($db) => $expr,
                DbBackend::F16($db) => $expr,
                DbBackend::U64($db) => $expr,
            }
        };
    }

    // ════════ JS 侧返回结构体 ════════

    /// 向量检索命中结果
    #[napi(object)]
    pub struct JsSearchHit {
        /// 节点 ID（JS Number，安全范围内的 u64）
        pub id: f64,
        /// 相似度得分
        pub score: f64,
        /// 节点元数据（JSON 对象）
        pub payload: serde_json::Value,
    }

    /// 高级管线专用配置结构
    #[napi(object)]
    pub struct JsSearchConfig {
        pub top_k: Option<u32>,
        pub expand_depth: Option<u32>,
        pub min_score: Option<f64>,
        pub teleport_alpha: Option<f64>,
        pub enable_advanced_pipeline: Option<bool>,
        pub enable_sparse_residual: Option<bool>,
        pub fista_lambda: Option<f64>,
        pub fista_threshold: Option<f64>,
        pub enable_dpp: Option<bool>,
        pub dpp_quality_weight: Option<f64>,
        pub enable_refractory_fatigue: Option<bool>,
        pub enable_text_hybrid_search: Option<bool>,
        pub text_boost: Option<f64>,
        pub force_brute_force: Option<bool>,
        pub custom_query_text: Option<String>,
    }

    /// 节点关系边
    #[napi(object)]
    pub struct JsEdge {
        pub target_id: f64,
        pub label: String,
        pub weight: f64,
    }

    /// 节点完整视图
    #[napi(object)]
    pub struct JsNodeView {
        pub id: f64,
        pub vector: Vec<f64>,
        pub payload: serde_json::Value,
        pub edges: Vec<JsEdge>,
        pub num_edges: u32,
    }

    /// Leiden 聚类结果结构
    #[napi(object)]
    pub struct JsClusterResult {
        /// 平铺数组: [nodeId1, clusterId1, nodeId2, clusterId2, ...]
        pub node_to_cluster: Vec<f64>,
        /// 平铺数组: [clusterId1, "label1", ...]
        pub cluster_labels: Vec<String>,
        /// 平铺首尾连接数组: [clusterId1, vector[0]...vector[dim], clusterId2, ...]
        pub centroids: Vec<f64>,
    }

    /// Leiden 聚类配置 (全部可选)
    #[napi(object)]
    pub struct JsLeidenConfig {
        /// 最小社区大小 (节点数 < 此值的碎片簇被丢弃, 默认 3)
        pub min_community_size: Option<u32>,
        /// 最大迭代轮次 (默认 15)
        pub max_iterations: Option<u32>,
        /// 是否计算质心 (默认 true)
        pub with_centroids: Option<bool>,
    }

    /// Hook 管线执行上下文（包含各阶段计时统计和自定义数据）
    #[napi(object)]
    pub struct JsHookContext {
        /// 各管线阶段的耗时统计（JSON 对象, 单位: 毫秒）
        pub timings: serde_json::Value,
        /// Hook 注入的自定义数据
        pub custom_data: serde_json::Value,
        /// 管线是否被 Hook 提前终止
        pub aborted: bool,
    }

    /// 带上下文的检索结果
    #[napi(object)]
    pub struct JsSearchWithContextResult {
        /// 检索结果列表
        pub hits: Vec<JsSearchHit>,
        /// Hook 管线上下文
        pub context: JsHookContext,
    }

    // ════════ 辅助：JSON Value → Filter ════════

    fn json_to_filter(val: &serde_json::Value) -> napi::Result<Filter> {
        Filter::from_json(val).map_err(|e| napi::Error::from_reason(e))
    }

    fn parse_sync_mode(s: &str) -> napi::Result<crate::storage::wal::SyncMode> {
        crate::storage::wal::SyncMode::parse(s).map_err(|e| napi::Error::from_reason(e))
    }

    // ════════ TriviumDB 主类 ════════

    #[napi(js_name = "TriviumDB")]
    pub struct TriviumDB {
        inner: DbBackend,
        dtype: String,
    }

    #[napi]
    impl TriviumDB {
        /// 打开或创建数据库
        ///
        /// ```js
        /// const db = new TriviumDB("data.tdb", 1536, "f32", "normal")
        /// ```
        #[napi(constructor)]
        pub fn new(
            path: String,
            dim: Option<u32>,
            dtype: Option<String>,
            sync_mode: Option<String>,
        ) -> napi::Result<Self> {
            let dim = dim.unwrap_or(1536) as usize;
            let dtype_str = dtype.as_deref().unwrap_or("f32");
            let sm = parse_sync_mode(sync_mode.as_deref().unwrap_or("normal"))?;

            let inner = match dtype_str {
                "f32" => DbBackend::F32(
                    GenericDatabase::<f32>::open_with_sync(&path, dim, sm)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?,
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_sync(&path, dim, sm)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?,
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_sync(&path, dim, sm)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?,
                ),
                _ => return Err(napi::Error::from_reason("dtype 必须是 f32 / f16 / u64")),
            };
            Ok(Self {
                inner,
                dtype: dtype_str.to_string(),
            })
        }

        // ── Hook 管理 ──

        /// 加载 C/C++ 动态库作为检索管线 Hook
        ///
        /// 动态库需导出 C ABI 符号（均可选）：
        /// - `trivium_recall`: 自定义召回
        /// - `trivium_rerank`: 自定义重排序
        ///
        /// ```js
        /// db.loadFfiHook('./libmy_plugin.so')
        /// const results = db.search(queryVec)  // 自动经过 C++ Hook
        /// ```
        #[napi]
        pub fn load_ffi_hook(&mut self, lib_path: String) -> napi::Result<()> {
            let ffi_hook = crate::hook::FfiHook::load(&lib_path)
                .map_err(|e| napi::Error::from_reason(format!("加载 FFI Hook 失败: {}", e)))?;
            dispatch!(self, mut db => db.set_hook(ffi_hook));
            Ok(())
        }

        /// 清除当前已注册的 Hook，恢复为默认的零开销 NoopHook
        #[napi]
        pub fn clear_hook(&mut self) {
            dispatch!(self, mut db => db.clear_hook());
        }

        /// 带 Hook 上下文的检索：返回 { hits, context }
        ///
        /// 除了检索结果外，同时返回管线各阶段的计时统计和 Hook 注入的自定义数据。
        ///
        /// ```js
        /// const { hits, context } = db.searchWithContext(queryVec, { topK: 10 })
        /// console.log(context.timings)     // { hook_pre_search: 0.1, graph_expand: 2.3 }
        /// console.log(context.customData)  // Hook 注入的自定义数据
        /// ```
        #[napi]
        pub fn search_with_context(
            &self,
            query_vector: Vec<f64>,
            config: Option<JsSearchConfig>,
        ) -> napi::Result<JsSearchWithContextResult> {
            let cfg = config.unwrap_or(JsSearchConfig {
                top_k: None,
                expand_depth: None,
                min_score: None,
                teleport_alpha: None,
                enable_advanced_pipeline: None,
                enable_sparse_residual: None,
                fista_lambda: None,
                fista_threshold: None,
                enable_dpp: None,
                dpp_quality_weight: None,
                enable_refractory_fatigue: None,
                custom_query_text: None,
                enable_text_hybrid_search: None,
                text_boost: None,
                force_brute_force: None,
            });

            let core_config = crate::database::SearchConfig {
                top_k: cfg.top_k.unwrap_or(5) as usize,
                expand_depth: cfg.expand_depth.unwrap_or(2) as usize,
                min_score: cfg.min_score.unwrap_or(0.1) as f32,
                teleport_alpha: cfg.teleport_alpha.unwrap_or(0.0) as f32,
                enable_advanced_pipeline: cfg.enable_advanced_pipeline.unwrap_or(false),
                enable_sparse_residual: cfg.enable_sparse_residual.unwrap_or(false),
                fista_lambda: cfg.fista_lambda.unwrap_or(0.1) as f32,
                fista_threshold: cfg.fista_threshold.unwrap_or(0.3) as f32,
                enable_dpp: cfg.enable_dpp.unwrap_or(false),
                dpp_quality_weight: cfg.dpp_quality_weight.unwrap_or(1.0) as f32,
                enable_refractory_fatigue: cfg.enable_refractory_fatigue.unwrap_or(false),
                enable_text_hybrid_search: cfg.enable_text_hybrid_search.unwrap_or(false),
                text_boost: cfg.text_boost.unwrap_or(1.5) as f32,
                force_brute_force: cfg.force_brute_force.unwrap_or(false),
                ..Default::default()
            };

            let q_text = cfg.custom_query_text.as_deref();

            let (results, hook_ctx) = match &self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                    db.search_hybrid_with_context(q_text, Some(&v), &core_config)
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = query_vector
                        .iter()
                        .map(|&x| half::f16::from_f64(x))
                        .collect();
                    db.search_hybrid_with_context(q_text, Some(&v), &core_config)
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid_with_context(q_text, Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            // 转换 hits
            let hits: Vec<JsSearchHit> = results
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect();

            // 转换 HookContext → JsHookContext
            let mut timings_map = serde_json::Map::new();
            for (stage, dur) in &hook_ctx.stage_timings {
                timings_map.insert(
                    stage.clone(),
                    serde_json::json!(dur.as_secs_f64() * 1000.0), // 转为毫秒
                );
            }

            let context = JsHookContext {
                timings: serde_json::Value::Object(timings_map),
                custom_data: hook_ctx.custom_data,
                aborted: hook_ctx.abort,
            };

            Ok(JsSearchWithContextResult { hits, context })
        }

        // ── CRUD ──

        /// 插入节点，返回新节点 ID
        #[napi]
        pub fn insert(
            &mut self,
            vector: Vec<f64>,
            payload: serde_json::Value,
        ) -> napi::Result<f64> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert(&v, payload)
                        .map(|id| id as f64)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 批量插入节点，返回新分配的 ID 列表
        #[napi]
        pub fn batch_insert(
            &mut self,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<serde_json::Value>,
        ) -> napi::Result<Vec<f64>> {
            if vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("向量列表与负载列表长度不一致"));
            }
            let mut ids = Vec::with_capacity(vectors.len());
            for (v, p) in vectors.into_iter().zip(payloads.into_iter()) {
                let id = self.insert(v, p)?;
                ids.push(id);
            }
            Ok(ids)
        }

        /// 批量插入指定 ID 的节点
        #[napi]
        pub fn batch_insert_with_ids(
            &mut self,
            ids: Vec<f64>,
            vectors: Vec<Vec<f64>>,
            payloads: Vec<serde_json::Value>,
        ) -> napi::Result<()> {
            if ids.len() != vectors.len() || vectors.len() != payloads.len() {
                return Err(napi::Error::from_reason("ID、向量与负载列表长度不一致"));
            }
            for ((id, v), p) in ids
                .into_iter()
                .zip(vectors.into_iter())
                .zip(payloads.into_iter())
            {
                self.insert_with_id(id, v, p)?;
            }
            Ok(())
        }

        /// 带指定 ID 插入节点
        #[napi]
        pub fn insert_with_id(
            &mut self,
            id: f64,
            vector: Vec<f64>,
            payload: serde_json::Value,
        ) -> napi::Result<()> {
            let id = id as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert_with_id(id, &v, payload)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 按 ID 获取节点，不存在时返回 null
        #[napi]
        pub fn get(&self, id: f64) -> Option<JsNodeView> {
            let id = id as u64;
            match &self.inner {
                DbBackend::F32(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: e.weight as f64,
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
                DbBackend::F16(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: e.weight as f64,
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
                DbBackend::U64(db) => db.get(id).map(|n| {
                    let num_edges = n.edges.len() as u32;
                    let edges_arr = n
                        .edges
                        .into_iter()
                        .map(|e| JsEdge {
                            target_id: e.target_id as f64,
                            label: e.label.clone(),
                            weight: e.weight as f64,
                        })
                        .collect();
                    JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        edges: edges_arr,
                        num_edges,
                    }
                }),
            }
        }

        /// 更新节点元数据
        #[napi]
        pub fn update_payload(&mut self, id: f64, payload: serde_json::Value) -> napi::Result<()> {
            dispatch!(self, mut db => db.update_payload(id as u64, payload))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 部分更新节点 Payload（$set / $inc / $unset）
        ///
        /// 只修改指定字段，其他字段保持不变。
        ///
        /// ```js
        /// db.patchPayload(id, { $set: { name: "Alice" } })
        /// db.patchPayload(id, { $inc: { visits: 1 } })
        /// db.patchPayload(id, { $unset: { oldField: true } })
        /// db.patchPayload(id, { name: "Bob" })  // 简写，等价于 $set
        /// ```
        #[napi]
        pub fn patch_payload(&mut self, id: f64, patch: serde_json::Value) -> napi::Result<()> {
            dispatch!(self, mut db => db.patch_payload(id as u64, patch))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 更新节点向量
        #[napi]
        pub fn update_vector(&mut self, id: f64, vector: Vec<f64>) -> napi::Result<()> {
            let id = id as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> =
                        vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.update_vector(id, &v)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 删除节点（三层原子联删：向量 + Payload + 所有关联边）
        #[napi]
        pub fn delete(&mut self, id: f64) -> napi::Result<()> {
            dispatch!(self, mut db => db.delete(id as u64))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        // ── 图谱操作 ──

        /// 建立有向带权边
        #[napi]
        pub fn link(
            &mut self,
            src: f64,
            dst: f64,
            label: Option<String>,
            weight: Option<f64>,
        ) -> napi::Result<()> {
            let label = label.as_deref().unwrap_or("related");
            let weight = weight.unwrap_or(1.0) as f32;
            dispatch!(self, mut db => db.link(src as u64, dst as u64, label, weight))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 断开两节点间的所有边
        #[napi]
        pub fn unlink(&mut self, src: f64, dst: f64) -> napi::Result<()> {
            dispatch!(self, mut db => db.unlink(src as u64, dst as u64))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 获取 N 跳邻居节点 ID 列表
        #[napi]
        pub fn neighbors(&self, id: f64, depth: Option<u32>) -> Vec<f64> {
            let depth = depth.unwrap_or(1) as usize;
            dispatch!(self, db => db.neighbors(id as u64, depth))
                .into_iter()
                .map(|id| id as f64)
                .collect()
        }

        // ── 社区聚类 ──

        /// 基于物理记忆图谱进行 Leiden 社区发现
        ///
        /// **无锁设计**: 短暂持锁快照邻接表后立即释放，聚类在锁外计算。
        /// 调用期间数据库仍可正常读写。
        #[napi]
        pub fn leiden_cluster(
            &self,
            config: Option<JsLeidenConfig>,
        ) -> napi::Result<JsClusterResult> {
            let cfg = config.unwrap_or(JsLeidenConfig {
                min_community_size: None,
                max_iterations: None,
                with_centroids: None,
            });
            let min_c = cfg.min_community_size.unwrap_or(3) as usize;
            let max_iter = cfg.max_iterations.map(|v| v as usize);
            let with_cent = cfg.with_centroids;

            let result = dispatch!(self, db => db.leiden_cluster(min_c, max_iter, with_cent))
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            // 排序确保确定性输出
            let mut sorted_nodes: Vec<_> = result.node_to_cluster.into_iter().collect();
            sorted_nodes.sort_by_key(|&(id, _)| id);

            let mut node_to_cluster = Vec::with_capacity(sorted_nodes.len() * 2);
            for (n, c) in sorted_nodes {
                node_to_cluster.push(n as f64);
                node_to_cluster.push(c as f64);
            }

            // 簇标签: 排序后输出
            let mut sorted_sizes: Vec<_> = result.cluster_sizes.iter().collect();
            sorted_sizes.sort_by_key(|(c, _)| *c);

            let mut cluster_labels = Vec::with_capacity(sorted_sizes.len() * 2);
            for (c, size) in &sorted_sizes {
                cluster_labels.push(c.to_string());
                cluster_labels.push(format!("Cluster {} ({})", c, size));
            }

            // 质心: 排序后平铺
            let mut sorted_centroids: Vec<_> = result.centroids.into_iter().collect();
            sorted_centroids.sort_by_key(|(c, _)| *c);

            let mut centroids = Vec::new();
            for (c, v) in sorted_centroids {
                centroids.push(c as f64);
                for val in v {
                    centroids.push(val as f64);
                }
            }

            Ok(JsClusterResult {
                node_to_cluster,
                cluster_labels,
                centroids,
            })
        }

        // ── 向量检索 ──

        /// 混合检索：向量锚定 + 图谱扩散
        #[napi]
        pub fn search(
            &self,
            query_vector: Vec<f64>,
            top_k: Option<u32>,
            expand_depth: Option<u32>,
            min_score: Option<f64>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = top_k.unwrap_or(5) as usize;
            let expand_depth = expand_depth.unwrap_or(0) as usize;
            let min_score = min_score.unwrap_or(0.5) as f32;

            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = query_vector
                        .iter()
                        .map(|&x| half::f16::from_f64(x))
                        .collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        /// 认知检索引擎：完全参数化暴露的高级功能 (FISTA, DPP, PPR)
        #[napi]
        pub fn search_advanced(
            &self,
            query_vector: Vec<f64>,
            config: Option<JsSearchConfig>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let cfg = config.unwrap_or(JsSearchConfig {
                top_k: None,
                expand_depth: None,
                min_score: None,
                teleport_alpha: None,
                enable_advanced_pipeline: None,
                enable_sparse_residual: None,
                fista_lambda: None,
                fista_threshold: None,
                enable_dpp: None,
                dpp_quality_weight: None,
                enable_refractory_fatigue: None,
                custom_query_text: None,
                enable_text_hybrid_search: None,
                text_boost: None,
                force_brute_force: None,
            });

            let core_config = crate::database::SearchConfig {
                top_k: cfg.top_k.unwrap_or(5) as usize,
                expand_depth: cfg.expand_depth.unwrap_or(2) as usize,
                min_score: cfg.min_score.unwrap_or(0.1) as f32,
                teleport_alpha: cfg.teleport_alpha.unwrap_or(0.0) as f32,
                enable_advanced_pipeline: cfg.enable_advanced_pipeline.unwrap_or(true),
                enable_sparse_residual: cfg.enable_sparse_residual.unwrap_or(false),
                fista_lambda: cfg.fista_lambda.unwrap_or(0.1) as f32,
                fista_threshold: cfg.fista_threshold.unwrap_or(0.3) as f32,
                enable_dpp: cfg.enable_dpp.unwrap_or(false),
                dpp_quality_weight: cfg.dpp_quality_weight.unwrap_or(1.0) as f32,
                enable_refractory_fatigue: cfg.enable_refractory_fatigue.unwrap_or(false),
                enable_text_hybrid_search: cfg.enable_text_hybrid_search.unwrap_or(false),
                text_boost: cfg.text_boost.unwrap_or(1.5) as f32,
                force_brute_force: cfg.force_brute_force.unwrap_or(false),
                ..Default::default()
            };

            let q_text = cfg.custom_query_text.as_deref();

            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                    db.search_hybrid(q_text, Some(&v), &core_config)
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = query_vector
                        .iter()
                        .map(|&x| half::f16::from_f64(x))
                        .collect();
                    db.search_hybrid(q_text, Some(&v), &core_config)
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid(q_text, Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        /// 混合检索增强入口：带图扩散的双路检索
        #[napi]
        pub fn search_hybrid(
            &self,
            query_vector: Vec<f64>,
            query_text: String,
            top_k: Option<u32>,
            expand_depth: Option<u32>,
            min_score: Option<f64>,
            hybrid_alpha: Option<f64>,
        ) -> napi::Result<Vec<JsSearchHit>> {
            let top_k = top_k.unwrap_or(5) as usize;
            let expand_depth = expand_depth.unwrap_or(2) as usize;
            let min_score = min_score.unwrap_or(0.1) as f32;
            let alpha = hybrid_alpha.unwrap_or(0.7) as f32;
            // 简单的启发式权重换算
            let boost = (1.0 - alpha).max(0.1) * 3.0;

            let core_config = crate::database::SearchConfig {
                top_k,
                expand_depth,
                min_score,
                enable_text_hybrid_search: true,
                text_boost: boost,
                ..Default::default()
            };

            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                    db.search_hybrid(Some(&query_text), Some(&v), &core_config)
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = query_vector
                        .iter()
                        .map(|&x| half::f16::from_f64(x))
                        .collect();
                    db.search_hybrid(Some(&query_text), Some(&v), &core_config)
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search_hybrid(Some(&query_text), Some(&v), &core_config)
                }
            }
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits
                .into_iter()
                .map(|h| JsSearchHit {
                    id: h.id as f64,
                    score: h.score as f64,
                    payload: h.payload,
                })
                .collect())
        }

        // ── 文本索引 ──

        /// 对节点建立用于双路召回的长文本 BM25 索引
        #[napi]
        pub fn index_text(&mut self, id: f64, text: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_text(id as u64, &text))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 对节点建立用于精确命中的 AC自动机 高级关键词索引
        #[napi]
        pub fn index_keyword(&mut self, id: f64, keyword: String) -> napi::Result<()> {
            dispatch!(self, mut db => db.index_keyword(id as u64, &keyword))
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 在批量插入或重启后必须调用，用于重编译自动机与词频
        #[napi]
        pub fn build_text_index(&mut self) {
            let _ = dispatch!(self, mut db => db.build_text_index());
        }

        // ── 属性二级索引 ──

        /// 创建属性索引：对指定 payload 字段建立倒排索引
        ///
        /// ```js
        /// db.createIndex('name')   // 之后 tql('FIND {name: "Alice"} RETURN *') 使用 O(1) 索引
        /// ```
        #[napi]
        pub fn create_index(&mut self, field: String) {
            dispatch!(self, mut db => db.create_index(&field));
        }

        /// 删除属性索引（查询仍可用，退化为全扫描）
        #[napi]
        pub fn drop_index(&mut self, field: String) {
            dispatch!(self, mut db => db.drop_index(&field));
        }

        // ── 轻量级单字段查询 ──

        /// 获取节点的 payload（不含向量，比 get() 更轻量）
        #[napi]
        pub fn get_payload(&self, id: f64) -> Option<serde_json::Value> {
            dispatch!(self, db => db.get_payload(id as u64))
        }

        /// 获取节点的出边列表
        #[napi]
        pub fn get_edges(&self, id: f64) -> Vec<JsEdge> {
            dispatch!(self, db => db.get_edges(id as u64))
                .into_iter()
                .map(|e| JsEdge {
                    target_id: e.target_id as f64,
                    label: e.label,
                    weight: e.weight as f64,
                })
                .collect()
        }

        // ── TQL 统一查询 ──

        /// 执行 TQL (Trivium Query Language) 统一查询
        ///
        /// 支持三种入口：MATCH (图遍历) / FIND (文档过滤) / SEARCH (向量检索)
        ///
        /// ```js
        /// // 图遍历
        /// const rows = db.tql('MATCH (a)-[:knows]->(b) WHERE b.age > 18 RETURN b')
        /// // 文档过滤
        /// const rows = db.tql('FIND {type: "event", heat: {$gte: 0.7}} RETURN *')
        /// ```
        #[napi]
        pub fn tql(&self, query: String) -> napi::Result<Vec<serde_json::Value>> {
            fn row_to_json<T: crate::vector::VectorType>(
                row: std::collections::HashMap<String, crate::node::Node<T>>,
            ) -> serde_json::Value {
                let mut obj = serde_json::Map::new();
                for (var_name, node) in row {
                    obj.insert(
                        var_name,
                        serde_json::json!({
                            "id": node.id,
                            "payload": node.payload,
                            "numEdges": node.edges.len(),
                        }),
                    );
                }
                serde_json::Value::Object(obj)
            }

            match &self.inner {
                DbBackend::F32(db) => db
                    .tql(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::F16(db) => db
                    .tql(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::U64(db) => db
                    .tql(&query)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
            }
        }

        /// 执行 TQL 写操作（CREATE / SET / DELETE / DETACH DELETE）
        ///
        /// 返回 { affected: number, createdIds: number[] }
        ///
        /// ```js
        /// const result = db.tqlMut('CREATE (a {name: "Alice", age: 30})')
        /// console.log(result.affected)     // 1
        /// console.log(result.createdIds)   // [1]
        ///
        /// db.tqlMut('MATCH (a {name: "Alice"}) SET a.age == 31')
        /// db.tqlMut('MATCH (a {name: "Alice"}) DELETE a')
        /// ```
        #[napi]
        pub fn tql_mut(&mut self, query: String) -> napi::Result<serde_json::Value> {
            let result = dispatch!(self, mut db => db.tql_mut(&query))
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(serde_json::json!({
                "affected": result.affected,
                "createdIds": result.created_ids,
            }))
        }

        // ── 持久化与管理 ──

        /// 手动落盘
        #[napi]
        pub fn flush(&mut self) -> napi::Result<()> {
            dispatch!(self, mut db => db.flush())
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 运行时切换 WAL 同步模式
        #[napi]
        pub fn set_sync_mode(&mut self, mode: String) -> napi::Result<()> {
            let sm = parse_sync_mode(&mode)?;
            dispatch!(self, mut db => db.set_sync_mode(sm));
            Ok(())
        }

        /// 启动后台自动压缩（每 interval_secs 秒落盘一次，默认 2 小时=7200秒）
        #[napi]
        pub fn enable_auto_compaction(&mut self, interval_secs: Option<u32>) {
            let secs = interval_secs.unwrap_or(7200) as u64;
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs)));
        }

        /// 停止后台自动压缩
        #[napi]
        pub fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
        }

        /// 手动触发全量压实（阻塞当前线程）
        #[napi]
        pub fn compact(&mut self) -> napi::Result<()> {
            dispatch!(self, mut db => db.compact())
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }

        /// 设置内存上限（MB），0 = 无限制
        #[napi]
        pub fn set_memory_limit(&mut self, mb: u32) {
            dispatch!(self, mut db => db.set_memory_limit(mb as usize * 1024 * 1024));
        }

        /// 估算当前内存占用（字节）
        #[napi]
        pub fn estimated_memory(&self) -> f64 {
            dispatch!(self, db => db.estimated_memory()) as f64
        }

        /// 获取向量维度
        #[napi]
        pub fn dim(&self) -> u32 {
            dispatch!(self, db => db.dim()) as u32
        }

        /// 获取节点总数
        #[napi]
        pub fn node_count(&self) -> u32 {
            dispatch!(self, db => db.node_count()) as u32
        }

        /// 获取所有活跃节点 ID
        #[napi]
        pub fn all_node_ids(&self) -> Vec<f64> {
            dispatch!(self, db => db.all_node_ids())
                .into_iter()
                .map(|id| id as f64)
                .collect()
        }

        /// 维度迁移：结构复制到新维度数据库，返回需要更新向量的节点 ID 列表
        #[napi]
        pub fn migrate(&self, new_path: String, new_dim: u32) -> napi::Result<Vec<f64>> {
            match &self.inner {
                DbBackend::F32(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::F16(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::U64(db) => {
                    let (_, ids) = db
                        .migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
            }
        }

        /// 获取 dtype 字符串（"f32" / "f16" / "u64"）
        #[napi(getter)]
        pub fn dtype(&self) -> String {
            self.dtype.clone()
        }

        /// 检查节点是否存在
        #[napi]
        pub fn contains(&self, id: f64) -> bool {
            dispatch!(self, db => db.contains(id as u64))
        }

        /// 显式关闭数据库（落盘后释放资源）
        #[napi]
        pub fn close(&mut self) -> napi::Result<()> {
            self.flush()
        }
    } // impl TriviumDB
} // mod nodejs
