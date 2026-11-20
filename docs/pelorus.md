# Pelorus 双曲向量索引

> Pelorus 为 TriviumDB 提供固定曲率 Poincaré 球的近似最近邻检索。普通向量查询默认选择 [QuIVer](quiver.md)。应用通过独立接口显式选择 Pelorus。

---

## 目录

- [度量与构建](#度量与构建)
- [检索入口](#检索入口)
- [精排与驻留](#精排与驻留)
- [持久化与写入](#持久化与写入)

---

## 度量与构建

曲率参数 `c` 是负曲率的绝对值，须为正有限数。每个向量满足 `c · ‖x‖² < 1`。内核支持 `f32` 和 `f16`，以实际存储或查询的浮点坐标校验。配置变更时，构建过程核对所有当前向量后才安装新索引。

构建使用 Pelorus 的双曲邻居选择和剪枝。默认 `geometry_bq2` 保留精确径向信息，用 2-bit 方向编码导航。设置 `navigation: "exact"` 可在图上用精确双曲距离导航。两种导航均按 K 近邻图召回候选。检索选项 `exact: true` 从原始坐标计算全量精确结果。

```bash
tdb build-pelorus notes.tdb --config '{"curvature":1.0,"navigation":"geometry_bq2"}'
```

Rust 使用 `Database::build_pelorus_index(PelorusConfig { .. })` 构建并发布检查点。Python 和 Node.js 的构建请求共用这一提交点。Server 使用 `POST /v1/indexes/pelorus` 接收同一配置，发布后返回当前代际。

---

## 检索入口

Rust 的 `search_pelorus` 和 `search_pelorus_batch` 返回 `f64` 测地距离。距离从小到大排列，同距离按节点 ID 升序。可用 `max_distance` 限制半径，用 `payload_filter` 限制候选。批量查询共享同一事实读视图，按输入顺序返回。

文档检索以 `f32` 返回负距离分数。超出该类型有限范围的分数返回 `TDB_QUERY_EXECUTION`。SDK 的 JSON 响应在分配编码缓冲区前核算实际字节数，包含转义字符占用。

Python 使用 `pelorus_request`，Node 使用 `pelorusRequest`。JSON 协议支持 `build`、`configure`、`search` 和 `search_batch`。响应以十进制字符串承载节点 ID。

```python
import json

result = json.loads(db.pelorus_request(json.dumps({
    "operation": "search",
    "vector": [0.12, 0.0],
    "options": {"top_k": 5, "max_distance": 0.8, "exact": True},
})))
for hit in result["hits"]:
    print(hit["id"], hit["distance"])
```

TQL 的 `SEARCH PELORUS` 支持 Prepared 参数、命名图范围与精确重排。TQL 的 `similarity(_)` 返回负测地距离，分数越大越近。SA-PPR 将双曲近邻分数转成正的种子权重。命名图成员在候选截断前生效。

```sql
GRAPH {graph_ids: ['events']}
SEARCH PELORUS {vector: [0.12, 0.0], options: {top_k: 5}}
RETURN _.id AS id, similarity(_) AS score
ORDER BY score DESC, id
```

文档向量与混合检索通过 Rust 的 `search_documents_pelorus`，或在 `document_request`、`documentRequest` 及 TQL `DOCUMENTS` 的 `search_vector`／`search_hybrid` 请求中设置 `pelorus: true`。文档和片段范围先限定向量候选，再按文档分组。混合模式按向量与文本排名融合。文档的 `min_score` 作用于负测地距离。

---

## 精排与驻留

精排支持 FP32、FP16、BF16、SQ8 和打包 SQ4，使用原始查询坐标和双精度累加。极坐标编码保留原始半径平方并压缩方向。SQ 支持对称、中心化和分组仿射编码。文档索引查询可以选择原始坐标或已配置的压缩行。

检查点发布后，可以将重排行切换到 `hot`、`mmap` 或 `ssd`。SSD 使用内核的共享 I/O 实现，支持按字节设置行缓存和在途预算。Linux 可显式选择 `io_uring`。Windows 直读复用 IOCP；`auto` 按设备和系统条件选择可用后端。精确扫描直接读取 TDB 的原始坐标。

```json
{
  "operation": "configure",
  "options": {
    "residency": "ssd",
    "io_mode": "auto",
    "io_depth": 4,
    "cache_bytes": 134217728,
    "inflight_bytes": 33554432
  }
}
```

使用 `profile_io: true` 获取单次请求的逻辑冷读、系统读取与缓存命中计数。关闭实例会等待已进入的查询结束，再释放读盘任务、缓存及文件映射。

---

## 持久化与写入

配置、图映像和变更节点集合写入统一 [.idx 容器](storage-format.md)。原始向量由数据库事实层维护。增删及更新后的向量按当前事实精确计分，已变更节点的旧图记录仅参与导航。事务恢复同样维护这一判定。增量达到维护上限后可重新构图，有配置的读视图可按缺索引策略执行同度量精确检索或返回结构化错误。

新建 Pelorus 索引后，写入使用对应曲率校验向量。事务和批处理在写 WAL 前检查，ReadOnly 与 Immutable 继续按只读规则执行。

> **发布提醒**：切换曲率时，内核先封存使用旧曲率的待恢复写入，再发布新检查点。若新检查点已经发布而 WAL 清理失败，错误返回 `write.outcome = "committed"`，Server 会更新全局 ETag。收到 `unknown` 时，重新打开数据库确认发布结果。

> **驻留提醒**：SSD 模式需要已发布的图映像。构建成功后可以切换驻留方式。后续更新向量时，写入方先完成检查点，再切换驻留方式。
