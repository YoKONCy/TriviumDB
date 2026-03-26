#[cfg(feature = "nodejs")]
pub mod nodejs {
    use napi_derive::napi;
    use crate::database::Database as GenericDatabase;
    use crate::filter::Filter;

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
        let obj = val.as_object().ok_or_else(|| {
            napi::Error::from_reason("过滤条件必须是 JSON 对象")
        })?;

        let mut filters = Vec::new();

        for (key, v) in obj {
            if key == "$and" {
                let arr = v.as_array().ok_or_else(|| napi::Error::from_reason("$and 必须是数组"))?;
                let sub: napi::Result<Vec<Filter>> = arr.iter().map(json_to_filter).collect();
                filters.push(Filter::And(sub?));
                continue;
            }
            if key == "$or" {
                let arr = v.as_array().ok_or_else(|| napi::Error::from_reason("$or 必须是数组"))?;
                let sub: napi::Result<Vec<Filter>> = arr.iter().map(json_to_filter).collect();
                filters.push(Filter::Or(sub?));
                continue;
            }

            // 运算符字典：{"field": {"$gt": 18}}
            if let Some(op_obj) = v.as_object() {
                for (op, op_val) in op_obj {
                    let f = match op.as_str() {
                        "$eq"  => Filter::Eq(key.clone(), op_val.clone()),
                        "$ne"  => Filter::Ne(key.clone(), op_val.clone()),
                        "$gt"  => Filter::Gt(key.clone(), op_val.as_f64().ok_or_else(|| napi::Error::from_reason("$gt 需要数字"))?),
                        "$gte" => Filter::Gte(key.clone(), op_val.as_f64().ok_or_else(|| napi::Error::from_reason("$gte 需要数字"))?),
                        "$lt"  => Filter::Lt(key.clone(), op_val.as_f64().ok_or_else(|| napi::Error::from_reason("$lt 需要数字"))?),
                        "$lte" => Filter::Lte(key.clone(), op_val.as_f64().ok_or_else(|| napi::Error::from_reason("$lte 需要数字"))?),
                        "$in"  => {
                            let arr = op_val.as_array().ok_or_else(|| napi::Error::from_reason("$in 需要数组"))?;
                            Filter::In(key.clone(), arr.clone())
                        }
                        other => return Err(napi::Error::from_reason(format!("不支持的运算符: {}", other))),
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
            "full"   => Ok(crate::storage::wal::SyncMode::Full),
            "normal" => Ok(crate::storage::wal::SyncMode::Normal),
            "off"    => Ok(crate::storage::wal::SyncMode::Off),
            other    => Err(napi::Error::from_reason(format!("不支持的 sync_mode: {}，可选值: full/normal/off", other))),
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
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?
                ),
                "f16" => DbBackend::F16(
                    GenericDatabase::<half::f16>::open_with_sync(&path, dim, sm)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?
                ),
                "u64" => DbBackend::U64(
                    GenericDatabase::<u64>::open_with_sync(&path, dim, sm)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?
                ),
                _ => return Err(napi::Error::from_reason("dtype 必须是 f32 / f16 / u64")),
            };
            Ok(Self { inner, dtype: dtype_str.to_string() })
        }

        // ── CRUD ──

        /// 插入节点，返回新节点 ID
        #[napi]
        pub fn insert(&mut self, vector: Vec<f64>, payload: serde_json::Value) -> napi::Result<f64> {
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert(&v, payload).map(|id| id as f64).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert(&v, payload).map(|id| id as f64).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert(&v, payload).map(|id| id as f64).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
            }
        }

        /// 带指定 ID 插入节点
        #[napi]
        pub fn insert_with_id(&mut self, id: f64, vector: Vec<f64>, payload: serde_json::Value) -> napi::Result<()> {
            let id = id as u64;
            match &mut self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                    db.insert_with_id(id, &v, payload).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.insert_with_id(id, &v, payload).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.insert_with_id(id, &v, payload).map_err(|e| napi::Error::from_reason(e.to_string()))
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
                    db.update_vector(id, &v).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.update_vector(id, &v).map_err(|e| napi::Error::from_reason(e.to_string()))
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = vector.iter().map(|&x| x as u64).collect();
                    db.update_vector(id, &v).map_err(|e| napi::Error::from_reason(e.to_string()))
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
        pub fn link(&mut self, src: f64, dst: f64, label: Option<String>, weight: Option<f64>) -> napi::Result<()> {
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
                .into_iter().map(|id| id as f64).collect()
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
            let top_k       = top_k.unwrap_or(5) as usize;
            let expand_depth = expand_depth.unwrap_or(0) as usize;
            let min_score   = min_score.unwrap_or(0.5) as f32;

            let hits = match &self.inner {
                DbBackend::F32(db) => {
                    let v: Vec<f32> = query_vector.iter().map(|&x| x as f32).collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
                DbBackend::F16(db) => {
                    let v: Vec<half::f16> = query_vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
                DbBackend::U64(db) => {
                    let v: Vec<u64> = query_vector.iter().map(|&x| x as u64).collect();
                    db.search(&v, top_k, expand_depth, min_score)
                }
            }.map_err(|e| napi::Error::from_reason(e.to_string()))?;

            Ok(hits.into_iter().map(|h| JsSearchHit {
                id: h.id as f64,
                score: h.score as f64,
                payload: h.payload,
            }).collect())
        }

        // ── 元数据过滤 ──

        /// 类 MongoDB 语法条件过滤，返回匹配节点列表
        #[napi]
        pub fn filter_where(&self, condition: serde_json::Value) -> napi::Result<Vec<JsNodeView>> {
            let filter = json_to_filter(&condition)?;
            let views = match &self.inner {
                DbBackend::F32(db) => db.filter_where(&filter).into_iter().map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|&x| x as f64).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }).collect::<Vec<_>>(),
                DbBackend::F16(db) => db.filter_where(&filter).into_iter().map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }).collect::<Vec<_>>(),
                DbBackend::U64(db) => db.filter_where(&filter).into_iter().map(|n| JsNodeView {
                    id: n.id as f64,
                    vector: n.vector.iter().map(|&x| x as f64).collect(),
                    payload: n.payload,
                    num_edges: n.edges.len() as u32,
                }).collect::<Vec<_>>(),
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
                row: std::collections::HashMap<String, crate::node::NodeView<T>>
            ) -> serde_json::Value {
                let mut obj = serde_json::Map::new();
                for (var_name, node) in row {
                    obj.insert(var_name, serde_json::json!({
                        "id": node.id,
                        "payload": node.payload,
                        "numEdges": node.edges.len(),
                    }));
                }
                serde_json::Value::Object(obj)
            }

            match &self.inner {
                DbBackend::F32(db) => db.query(&cypher)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::F16(db) => db.query(&cypher)
                    .map_err(|e| napi::Error::from_reason(e.to_string()))
                    .map(|rows| rows.into_iter().map(row_to_json).collect()),
                DbBackend::U64(db) => db.query(&cypher)
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
                .into_iter().map(|id| id as f64).collect()
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
                    let (_, ids) = db.migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::F16(db) => {
                    let (_, ids) = db.migrate_to(&new_path, new_dim as usize)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
                    Ok(ids.into_iter().map(|id| id as f64).collect())
                }
                DbBackend::U64(db) => {
                    let (_, ids) = db.migrate_to(&new_path, new_dim as usize)
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
