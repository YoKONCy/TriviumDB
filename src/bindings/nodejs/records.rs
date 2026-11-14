//! 节点读取、批量写入与 CRUD 参数转换。

use super::arguments::{
    node_id_to_js, node_ids_to_js, parse_u64_vector, parse_usize_argument, write_array, write_id,
    write_json, write_string, write_vector,
};
use super::database::{DbBackend, TriviumDB};
use super::errors::{to_napi_error, write_input_error};
use super::json::NodeJson;
use super::types::{JsEdge, JsNodeView, round_api_f32};
use napi::bindgen_prelude::Either;
use napi::{Env, JsUnknown};
use napi_derive::napi;

#[napi]
impl TriviumDB {
    // ── CRUD ──

    /// 插入节点，返回新节点 ID
    #[napi]
    pub fn insert(&mut self, env: Env, vector: JsUnknown, payload: JsUnknown) -> napi::Result<f64> {
        let vector = write_vector(env, vector, "vector", &self.dtype)?;
        let payload = write_json(env, payload, "payload")?;
        self.check_generated_ids(env, 1)?;
        match &mut self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                db.insert(&v, payload)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_id_to_js(env, value))
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                db.insert(&v, payload)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_id_to_js(env, value))
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                db.insert(&v, payload)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_id_to_js(env, value))
            }
        }
    }

    /// 批量插入节点，返回新分配的 ID 列表
    #[napi]
    pub fn batch_insert(
        &mut self,
        env: Env,
        vectors: JsUnknown,
        payloads: JsUnknown,
    ) -> napi::Result<Vec<f64>> {
        let vectors = write_array(env, vectors, "vectors", |env, value, path| {
            write_vector(env, value, path, &self.dtype)
        })?;
        let payloads = write_array(env, payloads, "payloads", write_json)?;
        if vectors.len() != payloads.len() {
            return Err(write_input_error(
                env,
                "payloads",
                "向量列表与 Payload 列表长度不一致 (Vector and payload list lengths differ)",
            ));
        }
        self.check_generated_ids(env, vectors.len())?;
        match &mut self.inner {
            DbBackend::F32(db) => {
                let converted: Vec<Vec<f32>> = vectors
                    .into_iter()
                    .map(|vector| vector.into_iter().map(|value| value as f32).collect())
                    .collect();
                let mut tx = crate::database::TxBuilder::new();
                for (vector, payload) in converted.iter().zip(payloads) {
                    tx.insert(vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_ids_to_js(env, value))
            }
            DbBackend::F16(db) => {
                let converted: Vec<Vec<half::f16>> = vectors
                    .into_iter()
                    .map(|vector| vector.into_iter().map(half::f16::from_f64).collect())
                    .collect();
                let mut tx = crate::database::TxBuilder::new();
                for (vector, payload) in converted.iter().zip(payloads) {
                    tx.insert(vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_ids_to_js(env, value))
            }
            DbBackend::U64(db) => {
                let converted: Vec<Vec<u64>> = vectors
                    .iter()
                    .map(|vector| parse_u64_vector(env, vector))
                    .collect::<napi::Result<_>>()?;
                let mut tx = crate::database::TxBuilder::new();
                for (vector, payload) in converted.iter().zip(payloads) {
                    tx.insert(vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map_err(|error| to_napi_error(env, error))
                    .and_then(|value| node_ids_to_js(env, value))
            }
        }
    }

    /// 批量插入指定 ID 的节点
    #[napi]
    pub fn batch_insert_with_ids(
        &mut self,
        env: Env,
        ids: JsUnknown,
        vectors: JsUnknown,
        payloads: JsUnknown,
    ) -> napi::Result<()> {
        let parsed_ids = write_array(env, ids, "ids", write_id)?;
        let vectors = write_array(env, vectors, "vectors", |env, value, path| {
            write_vector(env, value, path, &self.dtype)
        })?;
        let payloads = write_array(env, payloads, "payloads", write_json)?;
        if vectors.len() != payloads.len() {
            return Err(write_input_error(
                env,
                "payloads",
                "向量列表与 Payload 列表长度不一致 (Vector and payload list lengths differ)",
            ));
        }
        if parsed_ids.len() != vectors.len() {
            return Err(write_input_error(
                env,
                "ids",
                "ID 列表与向量列表长度不一致 (ID and vector list lengths differ)",
            ));
        }
        match &mut self.inner {
            DbBackend::F32(db) => {
                let converted: Vec<Vec<f32>> = vectors
                    .into_iter()
                    .map(|vector| vector.into_iter().map(|value| value as f32).collect())
                    .collect();
                let mut tx = crate::database::TxBuilder::new();
                for ((id, vector), payload) in parsed_ids.iter().zip(converted.iter()).zip(payloads)
                {
                    tx.insert_with_id(*id, vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::F16(db) => {
                let converted: Vec<Vec<half::f16>> = vectors
                    .into_iter()
                    .map(|vector| vector.into_iter().map(half::f16::from_f64).collect())
                    .collect();
                let mut tx = crate::database::TxBuilder::new();
                for ((id, vector), payload) in parsed_ids.iter().zip(converted.iter()).zip(payloads)
                {
                    tx.insert_with_id(*id, vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::U64(db) => {
                let converted: Vec<Vec<u64>> = vectors
                    .iter()
                    .map(|vector| parse_u64_vector(env, vector))
                    .collect::<napi::Result<_>>()?;
                let mut tx = crate::database::TxBuilder::new();
                for ((id, vector), payload) in parsed_ids.iter().zip(converted.iter()).zip(payloads)
                {
                    tx.insert_with_id(*id, vector, payload);
                }
                db.commit_tx(tx)
                    .map_err(crate::sdk::direct_write_error)
                    .map(|_| ())
                    .map_err(|error| to_napi_error(env, error))
            }
        }
    }

    /// 带指定 ID 插入节点
    #[napi]
    pub fn insert_with_id(
        &mut self,
        env: Env,
        id: JsUnknown,
        vector: JsUnknown,
        payload: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let vector = write_vector(env, vector, "vector", &self.dtype)?;
        let payload = write_json(env, payload, "payload")?;
        match &mut self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                db.insert_with_id(id, &v, payload)
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                db.insert_with_id(id, &v, payload)
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                db.insert_with_id(id, &v, payload)
                    .map_err(|error| to_napi_error(env, error))
            }
        }
    }

    /// 使用自定义 ID 插入或覆盖节点。
    #[napi]
    pub fn upsert_with_id(
        &mut self,
        env: Env,
        id: JsUnknown,
        vector: JsUnknown,
        payload: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let vector = write_vector(env, vector, "vector", &self.dtype)?;
        let payload = write_json(env, payload, "payload")?;
        match &mut self.inner {
            DbBackend::F32(db) => {
                let vector = vector
                    .into_iter()
                    .map(|value| value as f32)
                    .collect::<Vec<_>>();
                db.upsert_with_id(id, &vector, payload)
            }
            DbBackend::F16(db) => {
                let vector = vector
                    .into_iter()
                    .map(half::f16::from_f64)
                    .collect::<Vec<_>>();
                db.upsert_with_id(id, &vector, payload)
            }
            DbBackend::U64(db) => {
                let vector = parse_u64_vector(env, &vector)?;
                db.upsert_with_id(id, &vector, payload)
            }
        }
        .map_err(|error| to_napi_error(env, error))
    }

    #[napi]
    pub fn get(&self, env: Env, id: Either<f64, JsUnknown>) -> napi::Result<Option<JsNodeView>> {
        {
            let id = parse_usize_argument(env, id, "id", None)? as u64;
            match &self.inner {
                DbBackend::F32(db) => db
                    .get(id)
                    .map_err(|error| to_napi_error(env, error))?
                    .map(|n| {
                        let num_edges = n.edges.len() as u32;
                        let edges_arr = n
                            .edges
                            .into_iter()
                            .map(|e| {
                                Ok(JsEdge {
                                    target_id: node_id_to_js(env, e.target_id)?,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: e.metadata.clone().into(),
                                })
                            })
                            .collect::<napi::Result<_>>()?;
                        Ok(JsNodeView {
                            id: node_id_to_js(env, n.id)?,
                            vector: n.vector.iter().map(|&x| x as f64).collect(),
                            payload: n.payload.into(),
                            edges: edges_arr,
                            num_edges,
                        })
                    })
                    .transpose(),
                DbBackend::F16(db) => db
                    .get(id)
                    .map_err(|error| to_napi_error(env, error))?
                    .map(|n| {
                        let num_edges = n.edges.len() as u32;
                        let edges_arr = n
                            .edges
                            .into_iter()
                            .map(|e| {
                                Ok(JsEdge {
                                    target_id: node_id_to_js(env, e.target_id)?,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: e.metadata.clone().into(),
                                })
                            })
                            .collect::<napi::Result<_>>()?;
                        Ok(JsNodeView {
                            id: node_id_to_js(env, n.id)?,
                            vector: n.vector.iter().map(|x| x.to_f64()).collect(),
                            payload: n.payload.into(),
                            edges: edges_arr,
                            num_edges,
                        })
                    })
                    .transpose(),
                DbBackend::U64(db) => db
                    .get(id)
                    .map_err(|error| to_napi_error(env, error))?
                    .map(|n| {
                        let num_edges = n.edges.len() as u32;
                        let edges_arr = n
                            .edges
                            .into_iter()
                            .map(|e| {
                                Ok(JsEdge {
                                    target_id: node_id_to_js(env, e.target_id)?,
                                    label: e.label.clone(),
                                    weight: round_api_f32(e.weight),
                                    metadata: e.metadata.clone().into(),
                                })
                            })
                            .collect::<napi::Result<_>>()?;
                        Ok(JsNodeView {
                            id: node_id_to_js(env, n.id)?,
                            vector: n.vector.iter().map(|&x| x as f64).collect(),
                            payload: n.payload.into(),
                            edges: edges_arr,
                            num_edges,
                        })
                    })
                    .transpose(),
            }
        }
    }

    /// 更新节点元数据
    #[napi]
    pub fn update_payload(
        &mut self,
        env: Env,
        id: JsUnknown,
        payload: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let payload = write_json(env, payload, "payload")?;
        dispatch!(self, mut db => db.update_payload(id, payload))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 部分更新节点 Payload（$set / $inc / $unset）
    ///
    /// 只修改指定字段，其他字段保持不变。
    ///
    /// ```js
    /// db.patchPayload(id, { $set: { name: "Alice" } })
    /// db.patchPayload(id, { $inc: { visits: 1 } })
    /// db.patchPayload(id, { $unset: { oldField: true } })
    /// ```
    #[napi]
    pub fn patch_payload(&mut self, env: Env, id: JsUnknown, patch: JsUnknown) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let patch = write_json(env, patch, "patch")?;
        dispatch!(self, mut db => db.patch_payload(id, patch))
            .map_err(|error| to_napi_error(env, error))
    }

    /// 更新节点向量
    #[napi]
    pub fn update_vector(
        &mut self,
        env: Env,
        id: JsUnknown,
        vector: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let vector = write_vector(env, vector, "vector", &self.dtype)?;
        match &mut self.inner {
            DbBackend::F32(db) => {
                let v: Vec<f32> = vector.iter().map(|&x| x as f32).collect();
                db.update_vector(id, &v)
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::F16(db) => {
                let v: Vec<half::f16> = vector.iter().map(|&x| half::f16::from_f64(x)).collect();
                db.update_vector(id, &v)
                    .map_err(|error| to_napi_error(env, error))
            }
            DbBackend::U64(db) => {
                let v: Vec<u64> = parse_u64_vector(env, &vector)?;
                db.update_vector(id, &v)
                    .map_err(|error| to_napi_error(env, error))
            }
        }
    }

    #[napi]
    pub fn delete_many_atomic(&mut self, env: Env, ids: JsUnknown) -> napi::Result<u32> {
        let ids = write_array(env, ids, "ids", write_id)?;
        let deleted = dispatch!(self, mut db => db.delete_many_atomic(&ids))
            .map_err(|error| to_napi_error(env, error))?;
        u32::try_from(deleted).map_err(|_| napi::Error::from_reason("删除数量超出 u32"))
    }

    #[napi]
    pub fn compare_and_set_payload_field(
        &mut self,
        env: Env,
        id: JsUnknown,
        field: JsUnknown,
        expected: JsUnknown,
        replacement: JsUnknown,
    ) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        let field = write_string(env, field, "field")?;
        let expected = write_json(env, expected, "expected")?;
        let replacement = write_json(env, replacement, "replacement")?;
        dispatch!(self, mut db => db.compare_and_set_payload_field(id, &field, &expected, replacement))
                .map_err(|error| to_napi_error(env, error))
    }

    /// 删除节点（三层原子联删：向量 + Payload + 所有关联边）
    #[napi]
    pub fn delete(&mut self, env: Env, id: JsUnknown) -> napi::Result<()> {
        let id = write_id(env, id, "id")?;
        dispatch!(self, mut db => db.delete(id)).map_err(|error| to_napi_error(env, error))
    }

    // ── 轻量级单字段查询 ──

    /// 获取节点的 payload（不含向量，比 get() 更轻量）
    #[napi]
    pub fn get_payload(
        &self,
        env: Env,
        id: Either<f64, JsUnknown>,
    ) -> napi::Result<Option<NodeJson>> {
        let id = parse_usize_argument(env, id, "id", None)? as u64;
        Ok(
            dispatch!(self, db => db.get_payload(id).map_err(|error| to_napi_error(env, error))?)
                .map(NodeJson),
        )
    }
}
