//! GRU Router Index — 基于 MinGRU 路由的 ANN 索引
//!
//! # 核心思想
//!
//! 将 768 维向量拆为 T 个 chunk，用 MinGRU 逐步处理。
//! 每步的 gate 输出产生一个二分路由决策，T 步产生 T 位二进制签名。
//! 签名按整数排序后形成一维连续索引——等价于一棵隐式学习型空间二叉树。
//!
//! # Lipschitz 收敛保证
//!
//! 近邻向量的 chunk 差异小 → 每步 gate 差异 ≤ L·‖Δchunk‖
//! 遗忘门衰减历史误差 → 累积误差有界
//! → 近邻的路由签名大概率相同（LSH 性质）

/// MinGRU 路由器（推理用，权重由外部训练后注入）
pub struct MinGru {
    pub h_dim: usize,
    pub c_dim: usize,
    pub num_chunks: usize,
    /// Gate 权重: (h_dim + c_dim) × h_dim，行优先
    pub w_z: Vec<f32>,
    /// Candidate 权重: c_dim × h_dim，行优先
    pub w_h: Vec<f32>,
    /// Routing 权重: h_dim
    pub w_r: Vec<f32>,
    /// Gate bias: h_dim
    pub b_z: Vec<f32>,
    /// Routing bias: 标量
    pub b_r: f32,
}

impl MinGru {
    /// 参数数量
    pub fn num_params(&self) -> usize {
        self.w_z.len() + self.w_h.len() + self.w_r.len() + self.b_z.len() + 1
    }

    /// 获取所有参数的可变引用（用于训练）
    pub fn params_mut(&mut self) -> Vec<*mut f32> {
        let mut ptrs = Vec::with_capacity(self.num_params());
        for p in self.w_z.iter_mut() { ptrs.push(p as *mut f32); }
        for p in self.w_h.iter_mut() { ptrs.push(p as *mut f32); }
        for p in self.w_r.iter_mut() { ptrs.push(p as *mut f32); }
        for p in self.b_z.iter_mut() { ptrs.push(p as *mut f32); }
        ptrs.push(&mut self.b_r as *mut f32);
        ptrs
    }

    /// 前向传播：返回 T 个软路由值 (0,1)
    pub fn forward_soft(&self, x: &[f32]) -> Vec<f32> {
        let t = self.num_chunks;
        let h_dim = self.h_dim;
        let c_dim = self.c_dim;
        let mut h = vec![0.0f32; h_dim];
        let mut routes = Vec::with_capacity(t);

        for step in 0..t {
            let chunk = &x[step * c_dim..(step + 1) * c_dim];

            // concat = [h, chunk]
            let concat_dim = h_dim + c_dim;
            let mut concat = Vec::with_capacity(concat_dim);
            concat.extend_from_slice(&h);
            concat.extend_from_slice(chunk);

            // z = sigmoid(W_z @ concat + b_z)
            let mut z = vec![0.0f32; h_dim];
            for i in 0..h_dim {
                let mut sum = self.b_z[i];
                for j in 0..concat_dim {
                    sum += self.w_z[i * concat_dim + j] * concat[j];
                }
                z[i] = sigmoid(sum);
            }

            // h_tilde = W_h @ chunk  (MinGRU 简化：无 tanh)
            let mut h_tilde = vec![0.0f32; h_dim];
            for i in 0..h_dim {
                let mut sum = 0.0f32;
                for j in 0..c_dim {
                    sum += self.w_h[i * c_dim + j] * chunk[j];
                }
                h_tilde[i] = sum;
            }

            // h = (1 - z) * h + z * h_tilde
            for i in 0..h_dim {
                h[i] = (1.0 - z[i]) * h[i] + z[i] * h_tilde[i];
            }

            // route = sigmoid(w_r @ h + b_r)
            let mut r_sum = self.b_r;
            for i in 0..h_dim {
                r_sum += self.w_r[i] * h[i];
            }
            routes.push(sigmoid(r_sum));
        }

        routes
    }

    /// 计算二进制路由签名（hash）
    pub fn route_hash(&self, x: &[f32]) -> u64 {
        let routes = self.forward_soft(x);
        let mut hash: u64 = 0;
        for (i, &r) in routes.iter().enumerate() {
            if i >= 64 { break; }
            if r > 0.5 {
                hash |= 1u64 << (63 - i);
            }
        }
        hash
    }

    /// 带缓存的前向传播（BPTT 训练用）
    pub fn forward_with_cache(&self, x: &[f32]) -> GruCache {
        let t = self.num_chunks;
        let hd = self.h_dim;
        let cd = self.c_dim;
        let concat_dim = hd + cd;

        let mut cache = GruCache {
            h: vec![vec![0.0; hd]; t + 1],   // h[0] = zeros
            z: vec![vec![0.0; hd]; t],
            h_tilde: vec![vec![0.0; hd]; t],
            concat: vec![vec![0.0; concat_dim]; t],
            routes: vec![0.0; t],
            route_pre: vec![0.0; t],
        };

        for step in 0..t {
            let chunk = &x[step * cd..(step + 1) * cd];

            // concat = [h_{t-1}, chunk]
            cache.concat[step][..hd].copy_from_slice(&cache.h[step]);
            cache.concat[step][hd..].copy_from_slice(chunk);

            // z = sigmoid(W_z @ concat + b_z)
            for i in 0..hd {
                let mut s = self.b_z[i];
                for j in 0..concat_dim {
                    s += self.w_z[i * concat_dim + j] * cache.concat[step][j];
                }
                cache.z[step][i] = sigmoid(s);
            }

            // h_tilde = W_h @ chunk
            for i in 0..hd {
                let mut s = 0.0f32;
                for j in 0..cd {
                    s += self.w_h[i * cd + j] * chunk[j];
                }
                cache.h_tilde[step][i] = s;
            }

            // h_t = (1-z) * h_{t-1} + z * h_tilde
            for i in 0..hd {
                cache.h[step + 1][i] = (1.0 - cache.z[step][i]) * cache.h[step][i]
                    + cache.z[step][i] * cache.h_tilde[step][i];
            }

            // route = sigmoid(w_r · h_t + b_r)
            let mut rs = self.b_r;
            for i in 0..hd {
                rs += self.w_r[i] * cache.h[step + 1][i];
            }
            cache.route_pre[step] = rs;
            cache.routes[step] = sigmoid(rs);
        }
        cache
    }

    /// BPTT 反向传播：从 dL/d(routes) 计算所有权重梯度
    ///
    /// `d_routes`: T 个 dL/d(route_t) 值
    pub fn backward(&self, x: &[f32], cache: &GruCache, d_routes: &[f32]) -> GruGradients {
        let t = self.num_chunks;
        let hd = self.h_dim;
        let cd = self.c_dim;
        let concat_dim = hd + cd;

        let mut grad = GruGradients {
            dw_z: vec![0.0; self.w_z.len()],
            dw_h: vec![0.0; self.w_h.len()],
            dw_r: vec![0.0; hd],
            db_z: vec![0.0; hd],
            db_r: 0.0,
        };

        // dL/dh — 从未来步传播回来的梯度
        let mut dh = vec![0.0f32; hd];

        // 从 T-1 到 0 反向遍历
        for step in (0..t).rev() {
            let chunk = &x[step * cd..(step + 1) * cd];
            let r = cache.routes[step];

            // ── route_t = sigmoid(s_t), s_t = w_r · h_t + b_r ──
            let dr = d_routes[step];
            let ds = dr * r * (1.0 - r); // dL/ds_t

            // 梯度累加到 w_r, b_r
            for i in 0..hd {
                grad.dw_r[i] += ds * cache.h[step + 1][i];
            }
            grad.db_r += ds;

            // dL/dh_t 来自两个源：route 和 future
            // dL/dh_t from route = ds * w_r
            for i in 0..hd {
                dh[i] += ds * self.w_r[i];
            }

            // ── h_t = (1-z_t) * h_{t-1} + z_t * h̃_t ──

            // dL/dz_t = dL/dh_t * (h̃_t - h_{t-1})
            let mut dz = vec![0.0f32; hd];
            for i in 0..hd {
                dz[i] = dh[i] * (cache.h_tilde[step][i] - cache.h[step][i]);
            }

            // dL/dh̃_t = dL/dh_t * z_t
            let mut dh_tilde = vec![0.0f32; hd];
            for i in 0..hd {
                dh_tilde[i] = dh[i] * cache.z[step][i];
            }

            // dL/dh_{t-1} from h_t = dL/dh_t * (1 - z_t)
            let mut dh_prev = vec![0.0f32; hd];
            for i in 0..hd {
                dh_prev[i] = dh[i] * (1.0 - cache.z[step][i]);
            }

            // ── z_t = sigmoid(W_z @ concat + b_z) ──
            // dL/d(pre_z) = dL/dz * z * (1-z)
            let mut dpre_z = vec![0.0f32; hd];
            for i in 0..hd {
                dpre_z[i] = dz[i] * cache.z[step][i] * (1.0 - cache.z[step][i]);
            }

            // dL/dW_z += dpre_z outer concat
            for i in 0..hd {
                for j in 0..concat_dim {
                    grad.dw_z[i * concat_dim + j] += dpre_z[i] * cache.concat[step][j];
                }
            }
            // dL/db_z += dpre_z
            for i in 0..hd {
                grad.db_z[i] += dpre_z[i];
            }
            // dL/d(concat) = W_z^T @ dpre_z
            let mut d_concat = vec![0.0f32; concat_dim];
            for j in 0..concat_dim {
                for i in 0..hd {
                    d_concat[j] += self.w_z[i * concat_dim + j] * dpre_z[i];
                }
            }
            // dh_prev = 直接路径 + z 路径
            // 直接路径: dL/dh_t * (1 - z_t)
            // z 路径:   d_concat 的前 hd 维
            for i in 0..hd {
                dh_prev[i] += d_concat[i];
            }

            // ── h̃_t = W_h @ chunk ──
            // dL/dW_h += dh̃ outer chunk
            for i in 0..hd {
                for j in 0..cd {
                    grad.dw_h[i * cd + j] += dh_tilde[i] * chunk[j];
                }
            }

            // 传递 dh 到上一步
            dh = dh_prev;
        }

        grad
    }

    /// BPTT 从 hidden state 外部梯度反向传播
    ///
    /// `dh_external`: T+1 个外部注入到 h[step] 的梯度（可以稀疏，大部分为零）
    /// 不经过 route sigmoid，直接从 hidden state 反向传播到权重
    pub fn backward_from_hidden(
        &self, x: &[f32], cache: &GruCache, dh_external: &[Vec<f32>],
    ) -> GruGradients {
        let t = self.num_chunks;
        let hd = self.h_dim;
        let cd = self.c_dim;
        let concat_dim = hd + cd;

        let mut grad = GruGradients {
            dw_z: vec![0.0; self.w_z.len()],
            dw_h: vec![0.0; self.w_h.len()],
            dw_r: vec![0.0; hd],
            db_z: vec![0.0; hd],
            db_r: 0.0,
        };

        // dL/dh — 从未来步传播回来的梯度
        let mut dh = dh_external[t].clone(); // h[T] 的外部梯度

        for step in (0..t).rev() {
            let chunk = &x[step * cd..(step + 1) * cd];

            // 注入当前步 h[step+1] 的外部梯度
            // （dh 已经包含了从 step+1 传播下来的 + h[step+1] 的外部梯度）
            // h[step+1] 对应 forward 中 step 产生的 h_t

            // ── h_t = (1-z_t) * h_{t-1} + z_t * h̃_t ──
            let mut dz = vec![0.0f32; hd];
            for i in 0..hd {
                dz[i] = dh[i] * (cache.h_tilde[step][i] - cache.h[step][i]);
            }

            let mut dh_tilde = vec![0.0f32; hd];
            for i in 0..hd {
                dh_tilde[i] = dh[i] * cache.z[step][i];
            }

            let mut dh_prev = vec![0.0f32; hd];
            for i in 0..hd {
                dh_prev[i] = dh[i] * (1.0 - cache.z[step][i]);
            }

            // ── z_t = sigmoid(W_z @ concat + b_z) ──
            let mut dpre_z = vec![0.0f32; hd];
            for i in 0..hd {
                dpre_z[i] = dz[i] * cache.z[step][i] * (1.0 - cache.z[step][i]);
            }

            for i in 0..hd {
                for j in 0..concat_dim {
                    grad.dw_z[i * concat_dim + j] += dpre_z[i] * cache.concat[step][j];
                }
            }
            for i in 0..hd {
                grad.db_z[i] += dpre_z[i];
            }

            let mut d_concat = vec![0.0f32; concat_dim];
            for j in 0..concat_dim {
                for i in 0..hd {
                    d_concat[j] += self.w_z[i * concat_dim + j] * dpre_z[i];
                }
            }
            for i in 0..hd {
                dh_prev[i] += d_concat[i];
            }

            // ── h̃_t = W_h @ chunk ──
            for i in 0..hd {
                for j in 0..cd {
                    grad.dw_h[i * cd + j] += dh_tilde[i] * chunk[j];
                }
            }

            // 加上 h[step] 的外部梯度，传递到上一步
            for i in 0..hd {
                dh_prev[i] += dh_external[step][i];
            }
            dh = dh_prev;
        }

        grad
    }

    /// 应用梯度更新（SGD）
    pub fn apply_gradients(&mut self, grad: &GruGradients, lr: f32) {
        for (w, g) in self.w_z.iter_mut().zip(&grad.dw_z) { *w -= lr * g; }
        for (w, g) in self.w_h.iter_mut().zip(&grad.dw_h) { *w -= lr * g; }
        for (w, g) in self.w_r.iter_mut().zip(&grad.dw_r) { *w -= lr * g; }
        for (w, g) in self.b_z.iter_mut().zip(&grad.db_z) { *w -= lr * g; }
        self.b_r -= lr * grad.db_r;
    }
}

/// BPTT 前向缓存
pub struct GruCache {
    /// T+1 个 hidden states，h[0] = 全零
    pub h: Vec<Vec<f32>>,
    /// T 个 gate 值
    pub z: Vec<Vec<f32>>,
    /// T 个 candidate 值
    pub h_tilde: Vec<Vec<f32>>,
    /// T 个 concat 输入
    pub concat: Vec<Vec<f32>>,
    /// T 个软路由值
    pub routes: Vec<f32>,
    /// T 个路由 pre-sigmoid 值
    pub route_pre: Vec<f32>,
}

/// 权重梯度
pub struct GruGradients {
    pub dw_z: Vec<f32>,
    pub dw_h: Vec<f32>,
    pub dw_r: Vec<f32>,
    pub db_z: Vec<f32>,
    pub db_r: f32,
}

impl GruGradients {
    /// 将另一个梯度累加到自身
    pub fn accumulate(&mut self, other: &GruGradients) {
        for (a, b) in self.dw_z.iter_mut().zip(&other.dw_z) { *a += *b; }
        for (a, b) in self.dw_h.iter_mut().zip(&other.dw_h) { *a += *b; }
        for (a, b) in self.dw_r.iter_mut().zip(&other.dw_r) { *a += *b; }
        for (a, b) in self.db_z.iter_mut().zip(&other.db_z) { *a += *b; }
        self.db_r += other.db_r;
    }

    /// 缩放所有梯度
    pub fn scale(&mut self, s: f32) {
        for v in &mut self.dw_z { *v *= s; }
        for v in &mut self.dw_h { *v *= s; }
        for v in &mut self.dw_r { *v *= s; }
        for v in &mut self.db_z { *v *= s; }
        self.db_r *= s;
    }

    /// 创建零梯度
    pub fn zeros(w_z_len: usize, w_h_len: usize, h_dim: usize) -> Self {
        GruGradients {
            dw_z: vec![0.0; w_z_len],
            dw_h: vec![0.0; w_h_len],
            dw_r: vec![0.0; h_dim],
            db_z: vec![0.0; h_dim],
            db_r: 0.0,
        }
    }
}

/// GRU Router 索引
pub struct GruRouterIndex {
    dim: usize,
    n: usize,
    gru: MinGru,
    /// 向量（按路由签名排序）
    vectors: Vec<f32>,
    /// ID（同序）
    ids: Vec<u64>,
    /// 路由签名（已排序）
    hashes: Vec<u64>,
}

/// 查询配置
pub struct GruSearchConfig {
    pub top_k: usize,
    pub window_size: usize,
}

impl GruRouterIndex {
    /// 从已训练的 GRU 构建索引
    pub fn build(
        vectors: &[f32],
        ids: &[u64],
        dim: usize,
        gru: MinGru,
    ) -> Self {
        let n = ids.len();
        assert_eq!(vectors.len(), n * dim);
        assert_eq!(dim, gru.c_dim * gru.num_chunks);

        // 计算每个向量的路由签名
        let mut entries: Vec<(u64, usize)> = (0..n)
            .map(|i| {
                let v = &vectors[i * dim..(i + 1) * dim];
                (gru.route_hash(v), i)
            })
            .collect();

        entries.sort_unstable_by_key(|e| e.0);

        let mut sorted_vecs = Vec::with_capacity(n * dim);
        let mut sorted_ids = Vec::with_capacity(n);
        let mut sorted_hashes = Vec::with_capacity(n);

        for &(hash, orig) in &entries {
            sorted_vecs.extend_from_slice(&vectors[orig * dim..(orig + 1) * dim]);
            sorted_ids.push(ids[orig]);
            sorted_hashes.push(hash);
        }

        GruRouterIndex {
            dim, n, gru,
            vectors: sorted_vecs,
            ids: sorted_ids,
            hashes: sorted_hashes,
        }
    }

    /// 查询
    pub fn search(&self, query: &[f32], config: &GruSearchConfig) -> Vec<(u64, f32)> {
        let q_hash = self.gru.route_hash(query);

        let landing = self.hashes.partition_point(|&h| h < q_hash);

        let half = config.window_size / 2;
        let lo = landing.saturating_sub(half);
        let hi = (landing + half).min(self.n);

        let mut cands: Vec<(u64, f32)> = (lo..hi)
            .map(|i| {
                let v = &self.vectors[i * self.dim..(i + 1) * self.dim];
                (self.ids[i], cosine_sim(query, v))
            })
            .collect();

        cands.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        cands.truncate(config.top_k);
        cands
    }

    pub fn node_count(&self) -> usize { self.n }
    pub fn overhead_bytes(&self) -> usize {
        self.hashes.len() * 8 + self.gru.num_params() * 4
    }
}

#[inline]
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

#[inline]
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let ab: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    let d = na * nb;
    if d < 1e-30 { 0.0 } else { ab / d }
}
