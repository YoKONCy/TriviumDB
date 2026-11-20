//! 保存实际图、量化签名及路由状态，恢复时无需重新构图。

use super::*;
use crate::index::pelorus::engine::quantization::SmCodeParams;
use crate::index::pelorus::engine::storage::binary::{Reader, put, put_usize, put_vec};
use crate::index::pelorus::engine::{Error, Result};
use hyperbolic::{NUM_RADIAL_BANDS, POLAR_CELL_CAP, PolarNodeMeta, PolarRouter, ResidualConeDict};

impl QuIVer {
    pub(crate) fn snapshot_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_usize(&mut out, self.n);
        put(&mut out, self.entry_point);
        put_usize(&mut out, self.max_level);
        put_usize(&mut out, self.dirty_count);
        put(&mut out, u32::from(self.quantized_native));
        put_usize(&mut out, self.layer0.stride);
        put_vec(&mut out, &self.layer0.data);
        put_vec(&mut out, &self.ids);
        let slots = self
            .slot_indices
            .iter()
            .map(|&slot| slot as u64)
            .collect::<Vec<_>>();
        put_vec(&mut out, &slots);
        put_usize(&mut out, self.n);
        out.extend_from_slice(&self.node_max_layer);
        out.extend(self.tombstones.iter().map(|&value| u8::from(value)));
        put_usize(&mut out, self.upper_layers.len());
        for layer in &self.upper_layers {
            put_usize(&mut out, layer.len());
            for neighbors in layer {
                put_vec(&mut out, neighbors);
            }
        }
        write_store(&mut out, &self.bq_store, self.dim);
        put_usize(&mut out, self.geometry_codes.len());
        for code in &self.geometry_codes {
            put(&mut out, code.radius);
            put(&mut out, code.boundary_factor);
            write_sm(&mut out, code.sm);
        }
        put(&mut out, u32::from(self.polar_router.is_some()));
        if let Some(router) = &self.polar_router {
            put_usize(&mut out, router.num_cones);
            write_store(&mut out, &router.cone_store, self.dim);
            put_usize(&mut out, router.cone_sm.len());
            for &sm in &router.cone_sm {
                write_sm(&mut out, sm);
            }
            put_vec(&mut out, &router.cone_inv_qnorm);
            for &edge in &router.band_edges {
                put(&mut out, edge);
            }
            put(&mut out, router.min_depth);
            put(&mut out, router.max_depth);
            put(&mut out, router.shell_span);
            put(&mut out, u32::from(router.adaptive_shells));
            put_usize(&mut out, router.cells.len());
            for cell in &router.cells {
                for &node in cell {
                    put(&mut out, node);
                }
            }
            put_vec(&mut out, &router.origin_hubs);
            put_usize(&mut out, self.n);
            for node in 0..self.n {
                write_meta(&mut out, *router.node_meta(node).unwrap());
            }
            put_usize(&mut out, router.query_coarse_count);
            put_usize(&mut out, router.query_coarse_cap);
            put_vec(&mut out, &router.query_coarse_cones);
            put(&mut out, u32::from(router.is_isotropic_manifold));
            put(&mut out, u32::from(router.residual_dict.is_some()));
            if let Some(dict) = &router.residual_dict {
                put_usize(&mut out, dict.num_axes);
                put_vec(&mut out, &dict.axes);
            }
            put(&mut out, u32::from(router.colocated_store.is_some()));
            if let Some(store) = &router.colocated_store {
                put(&mut out, u32::from(router.node_meta.is_empty()));
                for node in 0..self.n {
                    for &word in store.get(node).sketch256 {
                        put(&mut out, word);
                    }
                }
            }
        }
        out
    }

    pub(crate) fn from_snapshot(
        bytes: &[u8],
        dimensions: usize,
        config: &BuildConfig,
        ids: &[u64],
        active: &[bool],
    ) -> Result<Self> {
        let mut r = Reader::new(bytes);
        let n = r.usize()?;
        if n != ids.len() || n != active.len() {
            return Err(Error::CorruptIndex("图节点数与向量目录不一致"));
        }
        let mut graph = Self::new(dimensions, config);
        graph.n = n;
        graph.entry_point = r.get()?;
        graph.max_level = r.usize()?;
        graph.dirty_count = r.usize()?;
        if graph.dirty_count.checked_mul(4).is_none() {
            return Err(Error::CorruptIndex("增量计数超出平台范围"));
        }
        graph.quantized_native = r.boolean()?;
        let stride = r.usize()?;
        let data: Vec<u32> = r.vector()?;
        if stride != graph.layer0.stride
            || data.len() != n.checked_mul(stride).ok_or(Error::CapacityExceeded)?
        {
            return Err(Error::CorruptIndex("邻接表步长或长度错误"));
        }
        for row in data.chunks_exact(stride) {
            let degree = row[0] as usize;
            if degree >= stride || row[1..1 + degree].iter().any(|&node| node as usize >= n) {
                return Err(Error::CorruptIndex("邻接表度数或边标识越界"));
            }
        }
        graph.layer0 = FlatAdj { stride, data };
        graph.ids = r.vector()?;
        let slots: Vec<u64> = r.vector()?;
        if graph.ids.len() != n || slots.len() != n {
            return Err(Error::CorruptIndex("图标识目录长度错误"));
        }
        graph.slot_indices = slots
            .into_iter()
            .map(|slot| usize::try_from(slot).map_err(|_| Error::CapacityExceeded))
            .collect::<Result<_>>()?;
        if r.usize()? != n {
            return Err(Error::CorruptIndex("节点层级目录长度错误"));
        }
        graph.node_max_layer = r.take(n)?.to_vec();
        graph.tombstones = r
            .take(n)?
            .iter()
            .map(|&value| match value {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(Error::CorruptIndex("墓碑标记无效")),
            })
            .collect::<Result<_>>()?;
        let mut seen = vec![false; n];
        for node in 0..n {
            let slot = graph.slot_indices[node];
            if slot >= n
                || seen[slot]
                || graph.ids[node] != ids[slot]
                || graph.tombstones[node] == active[slot]
            {
                return Err(Error::CorruptIndex("图标识、墓碑与向量槽位不一致"));
            }
            seen[slot] = true;
            graph.id_to_internal.insert(graph.ids[node], node as u32);
        }
        let layers = r.usize()?;
        if layers > 255 || graph.max_level != layers || (n > 0 && graph.entry_point as usize >= n) {
            return Err(Error::CorruptIndex("入口或最大层级无效"));
        }
        for level in 0..layers {
            if r.usize()? != n {
                return Err(Error::CorruptIndex("高层邻接目录长度错误"));
            }
            let mut layer = Vec::with_capacity(n);
            for _ in 0..n {
                let neighbors: Vec<u32> = r.vector()?;
                if neighbors.iter().any(|&node| {
                    node as usize >= n || graph.node_max_layer[node as usize] as usize <= level
                }) {
                    return Err(Error::CorruptIndex("高层边标识或层级无效"));
                }
                layer.push(neighbors);
            }
            graph.upper_layers.push(layer);
        }
        graph.bq_store = read_store(&mut r, dimensions, n)?;
        let codes = r.usize()?;
        if codes != 0 && codes != n {
            return Err(Error::CorruptIndex("几何编码目录长度错误"));
        }
        for _ in 0..codes {
            let radius = r.get::<f64>()?;
            let boundary_factor = r.get::<f64>()?;
            if !radius.is_finite()
                || radius < 0.0
                || !boundary_factor.is_finite()
                || boundary_factor <= 0.0
            {
                return Err(Error::CorruptIndex("几何编码数值无效"));
            }
            graph.geometry_codes.push(GeometryCode {
                radius,
                boundary_factor,
                sm: read_sm(&mut r, dimensions)?,
            });
        }
        if r.boolean()? {
            graph.polar_router = Some(read_router(&mut r, dimensions, n, &graph.bq_store)?);
            if let Some(router) = &graph.polar_router
                && router.node_meta.is_empty()
                && let Some(store) = &router.colocated_store
            {
                graph.bq_store.reclaim_for_colocated(Arc::clone(store));
            }
        }
        r.finish()?;
        graph.visited = Bitset::new(n);
        Ok(graph)
    }
}

fn write_store(out: &mut Vec<u8>, store: &Bq2Store, dim: usize) {
    put_usize(out, store.len());
    for node in 0..store.len() {
        for &word in store.pos_slice(node).iter().take(dim.div_ceil(64)) {
            put(out, word);
        }
        for &word in store.strong_slice(node).iter().take(dim.div_ceil(64)) {
            put(out, word);
        }
    }
}

fn read_store(r: &mut Reader<'_>, dim: usize, expected: usize) -> Result<Bq2Store> {
    let count = r.usize()?;
    if count != expected {
        return Err(Error::CorruptIndex("签名数量错误"));
    }
    let chunks = dim.div_ceil(64);
    let size = count
        .checked_mul(chunks)
        .and_then(|v| v.checked_mul(16))
        .ok_or(Error::CapacityExceeded)?;
    let mut values = Reader::new(r.take(size)?);
    let mut store = Bq2Store::new(dim);
    for _ in 0..count {
        let mut sig = Bq2Signature {
            pos: Vec::with_capacity(chunks),
            strong: Vec::with_capacity(chunks),
        };
        for _ in 0..chunks {
            sig.pos.push(values.get()?);
        }
        for _ in 0..chunks {
            sig.strong.push(values.get()?);
        }
        store.push_sig(&sig);
    }
    Ok(store)
}

fn write_sm(out: &mut Vec<u8>, sm: SmCodeParams) {
    put(out, sm.weak);
    put(out, sm.delta);
    put(out, sm.sigma);
    put(out, sm.strong_ones);
}
fn read_sm(r: &mut Reader<'_>, dim: usize) -> Result<SmCodeParams> {
    let value = SmCodeParams {
        weak: r.get()?,
        delta: r.get()?,
        sigma: r.get()?,
        strong_ones: r.get()?,
    };
    if [value.weak, value.delta, value.sigma]
        .iter()
        .any(|v| !v.is_finite())
        || value.weak < 0.0
        || value.sigma < 0.0
        || value.strong_ones as usize > dim
    {
        return Err(Error::CorruptIndex("幅值重构参数无效"));
    }
    Ok(value)
}
fn write_meta(out: &mut Vec<u8>, meta: PolarNodeMeta) {
    for value in [
        meta.radius,
        meta.boundary_factor,
        meta.depth,
        meta.sm_weak,
        meta.sm_delta,
        meta.sm_sigma,
    ] {
        put(out, value);
    }
    put(out, meta.packed_strong_cone);
    put(out, meta.cone_alpha);
}
fn read_meta(r: &mut Reader<'_>) -> Result<PolarNodeMeta> {
    let meta = PolarNodeMeta {
        radius: r.get()?,
        boundary_factor: r.get()?,
        depth: r.get()?,
        sm_weak: r.get()?,
        sm_delta: r.get()?,
        sm_sigma: r.get()?,
        packed_strong_cone: r.get()?,
        cone_alpha: r.get()?,
    };
    if [
        meta.radius,
        meta.boundary_factor,
        meta.depth,
        meta.sm_weak,
        meta.sm_delta,
        meta.sm_sigma,
        meta.cone_alpha,
    ]
    .iter()
    .any(|v| !v.is_finite())
        || meta.radius < 0.0
        || meta.boundary_factor < 1.0
        || meta.depth < 0.0
        || meta.sm_weak < 0.0
        || meta.sm_sigma < 0.0
        || !(-1.0..=1.0).contains(&meta.cone_alpha)
    {
        return Err(Error::CorruptIndex("极坐标元数据数值无效"));
    }
    Ok(meta)
}

fn read_router(
    r: &mut Reader<'_>,
    dim: usize,
    n: usize,
    signatures: &Bq2Store,
) -> Result<PolarRouter> {
    let cones = r.usize()?;
    if cones > u16::MAX as usize {
        return Err(Error::CorruptIndex("方向锥数量无效"));
    }
    let cone_store = read_store(r, dim, cones)?;
    let sm_count = r.usize()?;
    if sm_count != cones {
        return Err(Error::CorruptIndex("方向锥参数数量错误"));
    }
    let cone_sm = (0..cones)
        .map(|_| read_sm(r, dim))
        .collect::<Result<Vec<_>>>()?;
    let cone_inv_qnorm = r.vector::<f32>()?;
    if cone_inv_qnorm.len() != cones || cone_inv_qnorm.iter().any(|v| !v.is_finite()) {
        return Err(Error::CorruptIndex("方向锥归一化参数错误"));
    }
    let mut band_edges = [0.0f32; NUM_RADIAL_BANDS - 1];
    for edge in &mut band_edges {
        *edge = r.get()?;
    }
    let min_depth = r.get::<f32>()?;
    let max_depth = r.get::<f32>()?;
    let shell_span = r.get::<f32>()?;
    if !min_depth.is_finite()
        || !max_depth.is_finite()
        || !shell_span.is_finite()
        || shell_span <= 0.0
        || band_edges.iter().any(|v| !v.is_finite())
        || band_edges.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(Error::CorruptIndex("径向壳层参数无效"));
    }
    let adaptive_shells = r.boolean()?;
    if r.usize()? != cones * NUM_RADIAL_BANDS {
        return Err(Error::CorruptIndex("路由单元数量错误"));
    }
    let mut cells = vec![[EMPTY_NB; POLAR_CELL_CAP]; cones * NUM_RADIAL_BANDS];
    for cell in &mut cells {
        for value in cell {
            *value = r.get()?;
            if *value != EMPTY_NB && *value as usize >= n {
                return Err(Error::CorruptIndex("路由单元标识越界"));
            }
        }
    }
    let origin_hubs = r.vector::<u32>()?;
    if origin_hubs.iter().any(|&node| node as usize >= n) {
        return Err(Error::CorruptIndex("路由枢纽标识越界"));
    }
    if r.usize()? != n {
        return Err(Error::CorruptIndex("极坐标元数据数量错误"));
    }
    let mut node_meta = (0..n).map(|_| read_meta(r)).collect::<Result<Vec<_>>>()?;
    let query_coarse_count = r.usize()?;
    let query_coarse_cap = r.usize()?;
    let query_coarse_cones = r.vector::<u16>()?;
    if query_coarse_count > cones
        || query_coarse_count.checked_mul(query_coarse_cap) != Some(query_coarse_cones.len())
        || query_coarse_cones
            .iter()
            .any(|&cone| cone as usize >= cones)
    {
        return Err(Error::CorruptIndex("粗细方向锥目录错误"));
    }
    let is_isotropic_manifold = r.boolean()?;
    let residual_dict = if r.boolean()? {
        let num_axes = r.usize()?;
        let axes = r.vector::<f32>()?;
        if num_axes > 4095
            || num_axes.checked_mul(dim) != Some(axes.len())
            || axes.iter().any(|v| !v.is_finite())
        {
            return Err(Error::CorruptIndex("残差主轴字典错误"));
        }
        Some(ResidualConeDict {
            num_axes,
            dim,
            axes,
        })
    } else {
        None
    };
    if node_meta.iter().any(|meta| {
        meta.strong_ones() as usize > dim
            || (meta.has_residual_cone()
                && residual_dict
                    .as_ref()
                    .is_none_or(|dict| meta.cone_id() as usize >= dict.num_axes))
    }) {
        return Err(Error::CorruptIndex("节点残差主轴编号错误"));
    }
    let colocated_store = if r.boolean()? {
        if !hyperbolic::HotSmStore::supports_dim(dim) {
            return Err(Error::CorruptIndex("单页合表维度无效"));
        }
        let reclaimed = r.boolean()?;
        let size = n.checked_mul(32).ok_or(Error::CapacityExceeded)?;
        let mut words = Reader::new(r.take(size)?);
        let sketches = (0..n)
            .map(|_| Ok([words.get()?, words.get()?, words.get()?, words.get()?]))
            .collect::<Result<Vec<_>>>()?;
        let store = hyperbolic::HotSmStore::restore(dim, signatures, &node_meta, &sketches);
        if reclaimed {
            node_meta = Vec::new();
        }
        Some(Arc::new(store))
    } else {
        None
    };
    Ok(PolarRouter {
        num_cones: cones,
        cone_store,
        cone_sm,
        cone_inv_qnorm,
        band_edges,
        min_depth,
        max_depth,
        shell_span,
        adaptive_shells,
        cells,
        origin_hubs,
        node_meta,
        query_coarse_count,
        query_coarse_cap,
        query_coarse_cones,
        colocated_store,
        residual_dict,
        is_isotropic_manifold,
    })
}
