//! 线程私有查询缓冲与诊断状态。

use super::*;
#[cfg(feature = "research")]
#[derive(Default)]
pub(crate) struct QueryTrace {
    pub(crate) scored: std::collections::HashSet<u32>,
    pub(crate) raw_slots: std::collections::HashSet<usize>,
    pub(crate) beam: std::collections::HashSet<u32>,
    pub(crate) selected: std::collections::HashSet<u32>,
    pub(crate) expanded: std::collections::HashSet<u32>,
    pub(crate) expansion_calls: usize,
    pub(crate) exact_evals: usize,
    pub(crate) approximate_evals: usize,
    pub(crate) bound_rejections: usize,
    pub(crate) header_checks: usize,
    pub(crate) header_rejections: usize,
}
#[cfg(feature = "research")]
pub(crate) static TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "research")]
thread_local! { pub(crate) static TRACE: RefCell<Option<QueryTrace>> = const { RefCell::new(None) }; }
#[cfg(feature = "research")]
pub(crate) static STAGE_PROFILE_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "research")]
thread_local! { pub(crate) static STAGE_PROFILE: RefCell<SearchStageProfile> = const { RefCell::new(SearchStageProfile {
    upper_greedy_ns: 0, router_seed_ns: 0, beam_nav_ns: 0,
    asym_gate_ns: 0, exact_rerank_ns: 0, candidate_select_ns: 0,
    rerank_score_ns: 0, rerank_selected_count: 0, reranked_count: 0,
    rerank_radial_rejections: 0, navigation_passes: 0,
    main_expansion_calls: 0, horizon_expansion_calls: 0,
    stage05_candidates: 0, ef_nav: 0, router_seeds: 0,
}) }; }
#[cfg(feature = "research")]
#[inline]
pub(crate) fn trace_query(f: impl FnOnce(&mut QueryTrace)) {
    if TRACE_ACTIVE.load(AtomicOrdering::Relaxed) {
        TRACE.with(|t| {
            if let Some(t) = t.borrow_mut().as_mut() {
                f(t);
            }
        });
    } else if (EXECUTION_BITS.load(AtomicOrdering::Relaxed) & 1) == 0 {
        // O0=false 消融基线复现：保留 v3 非诊断态的 TLS + RefCell 空借用开销
        TRACE.with(|t| {
            std::hint::black_box(t.borrow_mut().is_some());
        });
    }
}
/// 查询期单次命中的高对齐切锥残差编码槽位（内部 Vec 缓冲跨查询复用，零热路径分配）。
pub(crate) struct CachedConeQuery {
    pub(crate) unit_res: Vec<f32>,
    pub(crate) sig: Bq2Signature,
    pub(crate) sm: SmCodeParams,
    pub(crate) asym_ready: bool,
    pub(crate) asym: AsymQuerySm,
}
impl Default for CachedConeQuery {
    fn default() -> Self {
        Self {
            unit_res: Vec::new(),
            sig: Bq2Signature {
                pos: Vec::new(),
                strong: Vec::new(),
            },
            sm: SmCodeParams::default(),
            asym_ready: false,
            asym: AsymQuerySm::new(),
        }
    }
}
/// 计算切锥正交残差重构下余弦估计标准差的一阶误差传播系数：
/// - 物化切锥槽位（`u_q` 切向投影残差已单位化重编码）：`hat_c = alpha_q * alpha_v + beta_q * beta_v * hat_c(r_q, r_v)`，严格系数为 `beta_q * beta_v`；
/// - 远端未物化切锥（直接复用全局查询编码）：`hat_c = alpha_q * alpha_v + beta_v * hat_c(u_q, r_v)`，严格系数为 `beta_v`。
#[inline]
pub(crate) fn residual_cosine_std_scale(
    alpha_q: f64,
    alpha_v: f64,
    materialized_cone: bool,
) -> f64 {
    let beta_v = (1.0 - alpha_v * alpha_v).clamp(0.0, 1.0).sqrt();
    if materialized_cone {
        let beta_q = (1.0 - alpha_q * alpha_q).clamp(0.0, 1.0).sqrt();
        beta_q * beta_v
    } else {
        beta_v
    }
}
/// 线程私有、世代号标记的查询切锥缓存（每查询最多物化前 16 个高对齐切锥主轴）。
pub(crate) struct QueryConeCache {
    pub(crate) generation: u32,
    pub(crate) stamp: [u32; MAX_RESIDUAL_CONES],
    pub(crate) alpha_q: [f32; MAX_RESIDUAL_CONES],
    pub(crate) beta_q: [f32; MAX_RESIDUAL_CONES],
    pub(crate) slot_idx: [u16; MAX_RESIDUAL_CONES],
    pub(crate) query_unit: Vec<f32>,
    pub(crate) query_valid: bool,
    pub(crate) used_slots: usize,
    pub(crate) slots: Vec<CachedConeQuery>,
}
impl QueryConeCache {
    pub(crate) const MAX_MATERIALIZED_SLOTS: usize = 16;
    pub(crate) const MATERIALIZE_ALPHA_MIN: f64 = 0.20;

    pub(crate) fn new() -> Self {
        let mut slots = Vec::with_capacity(Self::MAX_MATERIALIZED_SLOTS);
        for _ in 0..Self::MAX_MATERIALIZED_SLOTS {
            slots.push(CachedConeQuery::default());
        }
        Self {
            generation: 1,
            stamp: [0u32; MAX_RESIDUAL_CONES],
            alpha_q: [0.0f32; MAX_RESIDUAL_CONES],
            beta_q: [0.0f32; MAX_RESIDUAL_CONES],
            slot_idx: [u16::MAX; MAX_RESIDUAL_CONES],
            query_unit: Vec::new(),
            query_valid: false,
            used_slots: 0,
            slots,
        }
    }

    #[inline]
    pub(crate) fn begin_query(&mut self, query: &[f32], q_radius: f64, active: bool) {
        if !active || q_radius <= 1e-12 || !q_radius.is_finite() {
            self.query_valid = false;
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
        self.used_slots = 0;
        let dim = query.len();
        self.query_unit.resize(dim, 0.0);
        let inv = 1.0 / q_radius;
        for j in 0..dim {
            self.query_unit[j] = (f64::from(query[j]) * inv) as f32;
        }
        self.query_valid = true;
    }

    /// 按需计算并缓存查询在切锥 `cid` 上的投影余弦与切空间正交残差编码。
    #[inline]
    pub(crate) fn ensure_cone(&mut self, cid: usize, dict: &ResidualConeDict) -> (f64, f64, u16) {
        if self.stamp[cid] == self.generation {
            return (
                f64::from(self.alpha_q[cid]),
                f64::from(self.beta_q[cid]),
                self.slot_idx[cid],
            );
        }
        let axis = dict.axis(cid);
        let dim = dict.dim;
        let alpha1 = dot_f32_f64(&self.query_unit, axis);
        let alpha_clamped = alpha1.clamp(-1.0, 1.0);
        let beta = (1.0 - alpha_clamped * alpha_clamped).max(0.0).sqrt();

        let mut assigned_slot = u16::MAX;
        if alpha_clamped.abs() >= Self::MATERIALIZE_ALPHA_MIN
            && beta > 1e-6
            && self.used_slots < Self::MAX_MATERIALIZED_SLOTS
        {
            let s_idx = self.used_slots;
            let cq = &mut self.slots[s_idx];
            cq.unit_res.resize(dim, 0.0);
            let mut alpha2 = 0.0f64;
            for j in 0..dim {
                let wj = f64::from(self.query_unit[j]) - alpha1 * f64::from(axis[j]);
                cq.unit_res[j] = wj as f32;
                alpha2 += wj * f64::from(axis[j]);
            }
            let mut nsq = 0.0f64;
            for j in 0..dim {
                let wj = f64::from(cq.unit_res[j]) - alpha2 * f64::from(axis[j]);
                cq.unit_res[j] = wj as f32;
                nsq += wj * wj;
            }
            if nsq > 1e-12 {
                let inv_res = (1.0 / nsq.sqrt()) as f32;
                for v in &mut cq.unit_res {
                    *v *= inv_res;
                }
                cq.sig.encode_f32_into(&cq.unit_res);
                let (sm_q, _) = SmCodeParams::from_vector_and_sig(&cq.unit_res, &cq.sig);
                cq.sm = sm_q;
                cq.asym_ready = false;
                let exact_alpha = (alpha1 + alpha2).clamp(-1.0, 1.0);
                let exact_beta = (1.0 - exact_alpha * exact_alpha).max(0.0).sqrt();
                self.alpha_q[cid] = exact_alpha as f32;
                self.beta_q[cid] = exact_beta as f32;
                assigned_slot = s_idx as u16;
                self.used_slots += 1;
                self.slot_idx[cid] = assigned_slot;
                self.stamp[cid] = self.generation;
                return (exact_alpha, exact_beta, assigned_slot);
            }
        }

        self.alpha_q[cid] = alpha_clamped as f32;
        self.beta_q[cid] = beta as f32;
        self.slot_idx[cid] = assigned_slot;
        self.stamp[cid] = self.generation;
        (alpha_clamped, beta, assigned_slot)
    }

    /// 读取当前查询相对节点 `meta` 所属切锥的余弦不确定度传播系数（命中缓存 O(1)，未命中时按精确内积回退）。
    #[inline]
    pub(crate) fn query_residual_atten(
        &self,
        dict: Option<&ResidualConeDict>,
        query: &[f32],
        q_radius: f64,
        meta: PolarNodeMeta,
    ) -> f64 {
        let cid = meta.cone_id() as usize;
        if cid == usize::from(NO_CONE_ID) {
            return 1.0;
        }
        let Some(dict) = dict else {
            return 1.0;
        };
        if cid >= dict.num_axes {
            return 1.0;
        }
        let alpha_v = f64::from(meta.cone_alpha);
        if meta.sm_sigma <= 0.0 {
            return 0.0;
        }
        if self.query_valid && self.stamp[cid] == self.generation {
            let beta_v = (1.0 - alpha_v * alpha_v).clamp(0.0, 1.0).sqrt();
            return if self.slot_idx[cid] != u16::MAX {
                f64::from(self.beta_q[cid]) * beta_v
            } else {
                beta_v
            };
        }
        let alpha_q = if self.query_valid && self.query_unit.len() == dict.dim {
            dot_f32_f64(&self.query_unit, dict.axis(cid)).clamp(-1.0, 1.0)
        } else if q_radius > 1e-12 && query.len() == dict.dim {
            (dot_f32_f64(query, dict.axis(cid)) / q_radius).clamp(-1.0, 1.0)
        } else {
            return 1.0;
        };
        let beta_q = (1.0 - alpha_q * alpha_q).clamp(0.0, 1.0).sqrt();
        let materialized = alpha_q.abs() >= Self::MATERIALIZE_ALPHA_MIN
            && beta_q > 1e-6
            && self.used_slots < Self::MAX_MATERIALIZED_SLOTS;
        residual_cosine_std_scale(alpha_q, alpha_v, materialized)
    }

    /// 评估对称 5-popcount 球面余弦（自动透明分派切锥正交残差重构与常规全局重构）。
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn eval_sym_cosine(
        &mut self,
        dict: Option<&ResidualConeDict>,
        global_q_sig: &Bq2Signature,
        global_q_sm: SmCodeParams,
        pos_ptr: *const u64,
        strong_ptr: *const u64,
        meta: PolarNodeMeta,
        dim: usize,
    ) -> f64 {
        let cid = meta.cone_id() as usize;
        if cid != usize::from(NO_CONE_ID)
            && self.query_valid
            && let Some(dict) = dict
            && cid < dict.num_axes
        {
            let (alpha_q, beta_q, slot) = self.ensure_cone(cid, dict);
            let av_f32 = meta.cone_alpha;
            let alpha_v = f64::from(av_f32);
            let beta_v = f64::from((1.0f32 - av_f32 * av_f32).max(0.0).sqrt());
            if meta.sm_sigma <= 0.0 || beta_v <= 1e-8 {
                return (alpha_q * alpha_v).clamp(-1.0, 1.0);
            }
            if slot != u16::MAX {
                let cq = &self.slots[slot as usize];
                let counts = crate::index::pelorus::engine::quantization::sm_dot_counts_raw(
                    pos_ptr,
                    strong_ptr,
                    cq.sig.pos.as_ptr(),
                    cq.sig.strong.as_ptr(),
                    dim,
                );
                let cos_res = sm_calibrated_cosine_fast(cq.sm, meta.sm_params(), counts, dim);
                return (alpha_q * alpha_v + beta_q * beta_v * cos_res).clamp(-1.0, 1.0);
            }
            // 远端弱相关切锥：利用 <u_q, r_v> = beta_q * <r_{q,c}, r_v> 直接复用全局查询签名
            let counts = crate::index::pelorus::engine::quantization::sm_dot_counts_raw(
                pos_ptr,
                strong_ptr,
                global_q_sig.pos.as_ptr(),
                global_q_sig.strong.as_ptr(),
                dim,
            );
            let cos_qr = sm_calibrated_cosine_fast(global_q_sm, meta.sm_params(), counts, dim);
            return (alpha_q * alpha_v + beta_v * cos_qr).clamp(-1.0, 1.0);
        }

        let counts = crate::index::pelorus::engine::quantization::sm_dot_counts_raw(
            pos_ptr,
            strong_ptr,
            global_q_sig.pos.as_ptr(),
            global_q_sig.strong.as_ptr(),
            dim,
        );
        sm_calibrated_cosine_fast(global_q_sm, meta.sm_params(), counts, dim)
    }

    /// 评估 Stage-0.5 非对称校准球面余弦（自动透明分派切锥正交残差重构与常规全局重构）。
    #[inline(always)]
    pub(crate) fn eval_asym_cosine(
        &mut self,
        dict: Option<&ResidualConeDict>,
        global_asym: &AsymQuerySm,
        pos_slice: &[u64],
        strong_slice: &[u64],
        meta: PolarNodeMeta,
    ) -> f64 {
        let cid = meta.cone_id() as usize;
        if cid != usize::from(NO_CONE_ID)
            && self.query_valid
            && let Some(dict) = dict
            && cid < dict.num_axes
        {
            let (alpha_q, beta_q, slot) = self.ensure_cone(cid, dict);
            let av_f32 = meta.cone_alpha;
            let alpha_v = f64::from(av_f32);
            let beta_v = f64::from((1.0f32 - av_f32 * av_f32).max(0.0).sqrt());
            if meta.sm_sigma <= 0.0 || beta_v <= 1e-8 {
                return (alpha_q * alpha_v).clamp(-1.0, 1.0);
            }
            if slot != u16::MAX {
                let cq = &mut self.slots[slot as usize];
                if !cq.asym_ready {
                    cq.asym.encode_into(&cq.unit_res, 1.0);
                    cq.asym_ready = true;
                }
                let cos_res =
                    cq.asym
                        .calibrated_cosine_slices(pos_slice, strong_slice, meta.sm_params());
                return (alpha_q * alpha_v + beta_q * beta_v * cos_res).clamp(-1.0, 1.0);
            }
            let cos_qr =
                global_asym.calibrated_cosine_slices(pos_slice, strong_slice, meta.sm_params());
            return (alpha_q * alpha_v + beta_v * cos_qr).clamp(-1.0, 1.0);
        }

        global_asym.calibrated_cosine_slices(pos_slice, strong_slice, meta.sm_params())
    }
}
pub(crate) struct Scratch {
    pub(crate) visited: SparseBitset,
    pub(crate) candidates: BinaryHeap<Reverse<Candidate>>,
    pub(crate) results: BinaryHeap<Candidate>,
    pub(crate) horiz_candidates: BinaryHeap<Reverse<(u32, u32)>>,
    pub(crate) horiz_results: BinaryHeap<(u32, u32)>,
    pub(crate) horiz_portals: Vec<(u32, u32)>,
    pub(crate) apex_candidates: BinaryHeap<Reverse<Candidate>>,
    pub(crate) apex_results: BinaryHeap<Candidate>,
    pub(crate) apex_portals: Vec<Candidate>,
    pub(crate) cone_buf: Vec<(i32, usize)>,
    pub(crate) router_seeds: Vec<u32>,
    pub(crate) unvisited_buf: Vec<u32>,
    pub(crate) survivors: Vec<u32>,
    pub(crate) asym_query: AsymQuerySm,
    pub(crate) asym_sketch256: AsymSketch256Query,
    pub(crate) cone_cache: QueryConeCache,
    pub(crate) gate_pool: Vec<Candidate>,
    pub(crate) last_query_radius: f64,
    pub(crate) rerank: crate::index::pelorus::engine::storage::RerankScratch,
}
impl Scratch {
    pub(crate) fn new() -> Self {
        Self {
            visited: SparseBitset::new(0),
            candidates: BinaryHeap::new(),
            results: BinaryHeap::new(),
            horiz_candidates: BinaryHeap::new(),
            horiz_results: BinaryHeap::new(),
            horiz_portals: Vec::new(),
            apex_candidates: BinaryHeap::new(),
            apex_results: BinaryHeap::new(),
            apex_portals: Vec::new(),
            cone_buf: Vec::new(),
            router_seeds: Vec::new(),
            unvisited_buf: Vec::new(),
            survivors: Vec::new(),
            asym_query: AsymQuerySm::new(),
            asym_sketch256: AsymSketch256Query::default(),
            cone_cache: QueryConeCache::new(),
            gate_pool: Vec::new(),
            last_query_radius: -1.0,
            rerank: crate::index::pelorus::engine::storage::RerankScratch::default(),
        }
    }
}
thread_local! {
    pub(crate) static SCRATCH: RefCell<Scratch> = RefCell::new(Scratch::new());
}
