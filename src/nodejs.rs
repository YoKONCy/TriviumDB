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
        pub enable_text_hybrid_search: Option<bool>,
        pub text_boost: Option<f64>,
        pub custom_query_text: Option<String>,
    }

    /// 节点完整视图
    #[napi(object)]
    pub struct JsNodeView {
        pub id: f64,
        pub vector: Vec<f64>,
        pub payload: serde_json::Value,
        pub num_edges: u32,
    }

    // ════════ 辅助：JSON Value → Filter ════════

    fn json_to_filter(val: &serde_json::Value) -> napi::Result<Filter> {
        let obj = val
            .as_object()
            .ok_or_else(|| napi::Error::from_reason("过滤条件必须是 JSON 对象"))?;

        let mut filters = Vec::new();

        for (key, v) in obj {
            if key == "$and" {
                let arr = v
                    .as_array()
                    .ok_or_else(|| napi::Error::from_reason("$and 必须是数组"))?;
                let sub: napi::Result<Vec<Filter>> = arr.iter().map(json_to_filter).collect();
                filters.push(Filter::And(sub?));
                continue;
            }
            if key == "$or" {
                let arr = v
                    .as_array()
                    .ok_or_else(|| napi::Error::from_reason("$or 必须是数组"))?;
                let sub: napi::Result<Vec<Filter>> = arr.iter().map(json_to_filter).collect();
                filters.push(Filter::Or(sub?));
                continue;
            }

            // 运算符字典：{"field": {"$gt": 18}}
            if let Some(op_obj) = v.as_object() {
                for (op, op_val) in op_obj {
                    let f = match op.as_str() {
                        "$eq" => Filter::Eq(key.clone(), op_val.clone()),
                        "$ne" => Filter::Ne(key.clone(), op_val.clone()),
                        "$gt" => Filter::Gt(
                            key.clone(),
                            op_val
                                .as_f64()
                                .ok_or_else(|| napi::Error::from_reason("$gt 需要数字"))?,
                        ),
                        "$gte" => Filter::Gte(
                            key.clone(),
                            op_val
                                .as_f64()
                                .ok_or_else(|| napi::Error::from_reason("$gte 需要数字"))?,
                        ),
                        "$lt" => Filter::Lt(
                            key.clone(),
                            op_val
                                .as_f64()
                                .ok_or_else(|| napi::Error::from_reason("$lt 需要数字"))?,
                        ),
                        "$lte" => Filter::Lte(
                            key.clone(),
                            op_val
                                .as_f64()
                                .ok_or_else(|| napi::Error::from_reason("$lte 需要数字"))?,
                        ),
                        "$in" => {
                            let arr = op_val
                                .as_array()
                                .ok_or_else(|| napi::Error::from_reason("$in 需要数组"))?;
                            Filter::In(key.clone(), arr.clone())
                        }
                        "$exists" => {
                            let b = op_val
                                .as_bool()
                                .ok_or_else(|| napi::Error::from_reason("$exists 需要布尔值"))?;
                            Filter::Exists(key.clone(), b)
                        }
                        "$nin" => {
                            let arr = op_val
                                .as_array()
                                .ok_or_else(|| napi::Error::from_reason("$nin 需要数组"))?;
                            Filter::Nin(key.clone(), arr.clone())
                        }
                        "$size" => {
                            let s = op_val
                                .as_u64()
                                .ok_or_else(|| napi::Error::from_reason("$size 需要非负整数"))?;
                            Filter::Size(key.clone(), s as usize)
                        }
                        "$all" => {
                            let arr = op_val
                                .as_array()
                                .ok_or_else(|| napi::Error::from_reason("$all 需要数组"))?;
                            Filter::All(key.clone(), arr.clone())
                        }
                        "$type" => {
                            let t = op_val
                                .as_str()
                                .ok_or_else(|| napi::Error::from_reason("$type 需要字符串"))?;
                            Filter::TypeMatch(key.clone(), t.to_string())
                        }
                        other => {
                            return Err(napi::Error::from_reason(format!(
                                "不支持的运算符: {}",
                                other
                            )));
                        }
                    };
                    filters.push(f);
                }
            } else {
                // 简写等值：{"field": value}
                filters.push(Filter::Eq(key.clone(), v.clone()));
            }
        }

        match filters.len() {
            0 => Ok(Filter::Eq("none".into(), serde_json::Value::Null)),
            1 => Ok(filters.pop().unwrap()),
            _ => Ok(Filter::And(filters)),
        }
    }

    fn parse_sync_mode(s: &str) -> napi::Result<crate::storage::wal::SyncMode> {
        match s {
            "full" => Ok(crate::storage::wal::SyncMode::Full),
            "normal" => Ok(crate::storage::wal::SyncMode::Normal),
            "off" => Ok(crate::storage::wal::SyncMode::Off),
            other => Err(napi::Error::from_reason(format!(
                "不支持的 sync_mode: {}，可选值: full/normal/off",
                other
            ))),
        }
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
                DbBackend::F32(db) => db.get(id).map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|&x| x as f64).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }),
                DbBackend::F16(db) => db.get(id).map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }),
                DbBackend::U64(db) => db.get(id).map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|&x| x as f64).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }),
            }
        }

        /// 更新节点元数据
        #[napi]
        pub fn update_payload(&mut self, id: f64, payload: serde_json::Value) -> napi::Result<()> {
            dispatch!(self, mut db => db.update_payload(id as u64, payload))
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
            let cfg = config.unwrap_or_else(|| JsSearchConfig {
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
                enable_text_hybrid_search: cfg.enable_text_hybrid_search.unwrap_or(false),
                text_boost: cfg.text_boost.unwrap_or(1.5) as f32,
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
            dispatch!(self, mut db => db.build_text_index());
        }

        // ── 元数据过滤 ──

        /// 类 MongoDB 语法条件过滤，返回匹配节点列表
        #[napi]
        pub fn filter_where(&self, condition: serde_json::Value) -> napi::Result<Vec<JsNodeView>> {
            let filter = json_to_filter(&condition)?;
            let views = match &self.inner {
                DbBackend::F32(db) => db
                    .filter_where(&filter)
                    .into_iter()
                    .map(|n| JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        num_edges: n.edges.len() as u32,
                    })
                    .collect::<Vec<_>>(),
                DbBackend::F16(db) => db
                    .filter_where(&filter)
                    .into_iter()
                    .map(|n| JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                        payload: n.payload,
                        num_edges: n.edges.len() as u32,
                    })
                    .collect::<Vec<_>>(),
                DbBackend::U64(db) => db
                    .filter_where(&filter)
                    .into_iter()
                    .map(|n| JsNodeView {
                        id: n.id as f64,
                        vector: n.vector.iter().map(|&x| x as f64).collect(),
                        payload: n.payload,
                        num_edges: n.edges.len() as u32,
                    })
                    .collect::<Vec<_>>(),
            };
            Ok(views)
        }

        // ── Cypher 图谱查询 ──

        /// 执行类 Cypher 查询，返回每行变量绑定的 JSON 数组
        ///
        /// 每个结果行是 `{ varName: { id, payload, numEdges } }` 结构的对象
        #[napi]
        pub fn query(&self, cypher: String) -> napi::Result<Vec<serde_json::Value>> {
            // 辅助闭包：将一个 row(HashMap) 转成 serde_json::Value
            fn row_to_json<T: crate::vector::VectorType>(
                row: std::collections::HashMap<String, crate::node::NodeView<T>>,
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
                    .query(&cypher)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::F16(db) => db
                    .query(&cypher)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::U64(db) => db
                    .query(&cypher)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
            }
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

        /// 启动后台自动压缩（每 interval_secs 秒落盘一次）
        #[napi]
        pub fn enable_auto_compaction(&mut self, interval_secs: Option<u32>) {
            let secs = interval_secs.unwrap_or(30) as u64;
            dispatch!(self, mut db => db.enable_auto_compaction(std::time::Duration::from_secs(secs)));
        }

        /// 停止后台自动压缩
        #[napi]
        pub fn disable_auto_compaction(&mut self) {
            dispatch!(self, mut db => db.disable_auto_compaction());
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

        /// 重建 HNSW 向量索引（BruteForce 模式下为 no-op）
        #[napi]
        pub fn rebuild_index(&mut self) {
            dispatch!(self, mut db => db.rebuild_index());
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
    } // impl TriviumDB
} // mod nodejs
