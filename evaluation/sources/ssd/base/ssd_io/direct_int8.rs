//! query沿用全向量对称量化，数据库逐组VNNI整数点积与仿射偏置恢复。
use super::*;
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum Mode {Decode,Direct}
impl Mode {
    pub fn parse(c:&Value)->io::Result<Self> {
        match c["score_mode"].as_str().unwrap_or("direct_int8") {
            "direct_int8"=>Ok(Self::Direct),_=>Err(invalid("本轮只允许direct_int8评分")),
        }
    }
    pub fn name(self)->&'static str {"direct_int8"}
    pub fn kernel(self)->&'static str {"avx512_vnni_grouped_i32"}
}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum ScaleMode {Norm,Ls}
impl ScaleMode {
    pub fn name(self)->&'static str {match self {Self::Norm=>"norm",Self::Ls=>"ls"}}
}
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum QueryMode {Global,Local}
impl QueryMode {
    pub fn name(self)->&'static str {match self {Self::Global=>"global",Self::Local=>"local"}}
    pub fn quantization(self)->&'static str {match self {
        Self::Global=>"normalized_query_maxabs127_f32_ties_even",
        Self::Local=>"normalized_query_block128_maxabs127_f32_ties_even",
    }}
}
#[derive(Clone,Copy)]
pub(super) struct Layout {
    pub group_size:usize,pub norm_correction:bool,pub scale_mode:ScaleMode,pub query_mode:QueryMode,
}
impl Layout {
    pub fn parse(c:&Value)->io::Result<Self> {
        let group_size=match c.get("int8_group_size") {
            None=>128,Some(value)=>value.as_u64().ok_or_else(||invalid("INT8组大小须为整数"))? as usize,
        };
        let norm_correction=match c.get("int8_norm_correction") {
            None=>true,Some(value)=>value.as_bool().ok_or_else(||invalid("INT8范数校正须为布尔值"))?,
        };
        if group_size!=128 || !norm_correction {return Err(invalid("本轮固定group128及norm_correction=true"));}
        let scale_mode=match c.get("int8_scale_mode").map(Value::as_str).unwrap_or(Some("norm")) {
            Some("norm")=>ScaleMode::Norm,Some("ls")=>ScaleMode::Ls,_=>return Err(invalid("缩放模式须为norm或ls")),
        };
        let query_mode=match c.get("int8_query_mode").map(Value::as_str).unwrap_or(Some("global")) {
            Some("global")=>QueryMode::Global,Some("local")=>QueryMode::Local,_=>return Err(invalid("query模式须为global或local")),
        };
        Ok(Self {group_size,norm_correction,scale_mode,query_mode})
    }
    pub fn groups(self)->usize {6}
    pub fn payload(self)->usize {824}
    pub fn factor_offset(self)->usize {match self.scale_mode {ScaleMode::Norm=>816,ScaleMode::Ls=>820}}
    pub fn codec(self)->&'static str {"affine_int8_group128_norm_ls"}
}
#[derive(Default)]
pub(super) struct Stats {pub decoded:AtomicU64,pub direct:AtomicU64,pub queries:AtomicU64}
impl Stats {pub fn snapshot(&self)->[u64;3] {[value(&self.decoded),value(&self.direct),value(&self.queries)]}}
pub(super) struct Query {bytes:[u8;768],scales:[f32;6],sums:[i32;12],group_size:usize}
fn available()->bool {
    #[cfg(target_arch="x86_64")]
    {is_x86_feature_detected!("avx512f")&&is_x86_feature_detected!("avx512bw")&&is_x86_feature_detected!("avx512vnni")}
    #[cfg(not(target_arch="x86_64"))]
    {false}
}
impl Query {
    pub fn new(q:&[f32],stats:&Stats,layout:Layout)->io::Result<Self> {
        if !available() {return Err(io::Error::new(io::ErrorKind::Unsupported,"分组INT8要求AVX512VNNI"));}
        if q.len()!=768 || q.iter().any(|x|!x.is_finite()) {return Err(invalid("量化query须为有限768维"));}
        let mut bytes=[128u8;768];
        let mut scales=[0f32;6];
        if layout.query_mode==QueryMode::Global {
            // 保留父global量化的float32算式、ties-even与完整768维最大值。
            let maximum=q.iter().fold(0f32,|m,x|m.max(x.abs()));
            let scale=if maximum==0. {1.}else{maximum/127.};
            if scale<=0. || !scale.is_finite() {return Err(invalid("query量化scale无效"));}
            for (out,&x) in bytes.iter_mut().zip(q) {
                let code=(x/scale).round_ties_even().clamp(-127.,127.) as i32;
                *out=(code+128) as u8;
            }
            scales.fill(scale);
        }else{
            for (block,values) in q.chunks_exact(128).enumerate() {
                let maximum=values.iter().fold(0f32,|m,x|m.max(x.abs()));
                let scale=if maximum==0. {1.}else{maximum/127.};
                if scale<=0. || !scale.is_finite() {return Err(invalid("局部query量化scale无效"));}
                scales[block]=scale;
                for (out,&x) in bytes[block*128..(block+1)*128].iter_mut().zip(values) {
                    let code=(x/scale).round_ties_even().clamp(-127.,127.) as i32;
                    *out=(code+128) as u8;
                }
            }
        }
        let mut sums=[0i32;12];
        for (block,codes) in bytes.chunks_exact(layout.group_size).enumerate() {
            sums[block]=codes.iter().map(|&code|code as i32-128).sum();
        }
        add(&stats.queries,1);
        Ok(Self {bytes,scales,sums,group_size:layout.group_size})
    }
}
#[cfg(target_arch="x86_64")]
#[target_feature(enable="avx512f,avx512bw,avx512vnni")]
unsafe fn group_dots<const N:usize>(q:&[u8;768],rows:[&[u8];N],start:usize,len:usize)->[i32;N] {
    use std::arch::x86_64::*;
    let mut dots=[_mm512_setzero_si512();N];
    let mut sums=[_mm512_setzero_si512();N];
    let ones=_mm512_set1_epi8(1);
    for offset in (start..start+len).step_by(64) {
        // SAFETY：组范围由768/128/64配置生成，边界为64倍数且不超出768维数组。
        let query=unsafe {_mm512_loadu_si512(q.as_ptr().add(offset).cast())};
        for k in 0..N {
            // SAFETY：每行至少含768字节编码，当前64字节载入完全位于已验证的组内。
            let db=unsafe {_mm512_loadu_si512(rows[k].as_ptr().add(offset).cast())};
            dots[k]=_mm512_dpbusd_epi32(dots[k],query,db);
            sums[k]=_mm512_dpbusd_epi32(sums[k],ones,db);
        }
    }
    std::array::from_fn(|k|_mm512_reduce_add_epi32(dots[k])-128*_mm512_reduce_add_epi32(sums[k]))
}
fn integers<const N:usize>(q:&[u8;768],rows:[&[u8];N],start:usize,len:usize)->[i32;N] {
    #[cfg(target_arch="x86_64")]
    {
        // SAFETY：启动检查与Query构造已确认VNNI，行长度和所有组范围已验证。
        // 整数累加及unsigned零点修正均在i32范围内，未使用饱和加法。
        unsafe {group_dots(q,rows,start,len)}
    }
    #[cfg(not(target_arch="x86_64"))]
    {unreachable!("启动阶段已拒绝不支持的CPU")}
}
fn number(row:&[u8],offset:usize)->f32 {f32::from_le_bytes(row[offset..offset+4].try_into().unwrap())}
pub(super) fn validate(row:&[u8],layout:Layout)->io::Result<()> {
    if row.len()<layout.payload() || row[..768].iter().any(|&code|code==128) {return Err(invalid("分组INT8行长度或编码无效"));}
    for block in 0..6 {
        let scale=number(row,768+block*8);
        let bias=number(row,772+block*8);
        if scale<=0. || !scale.is_finite() || !bias.is_finite() {return Err(invalid("分组INT8 scale/bias无效"));}
    }
    let norm=number(row,816);
    let ls=number(row,820);
    if norm<=0. || !norm.is_finite() {return Err(invalid("norm_factor必须有限且为正"));}
    if ls<=0. || !ls.is_finite() {return Err(invalid("LSfactor必须有限且为正"));}
    Ok(())
}
pub(super) fn score<const N:usize>(q:&Query,rows:[&[u8];N],stats:&Stats,layout:Layout)->io::Result<[f32;N]> {
    if q.group_size!=128 {return Err(invalid("query与固定group128不一致"));}
    for row in rows {validate(row,layout)?;}
    let mut scores=[0f32;N];
    for block in 0..6 {
        let dots=integers(&q.bytes,rows,block*128,128);
        for k in 0..N {
            // global的每块scale逐位等于父q.scale，运算和块累加顺序完全沿父group128。
            let base=(dots[k] as f32*q.scales[block])*number(rows[k],768+block*8);
            let bias=(q.sums[block] as f32*q.scales[block])*number(rows[k],772+block*8);
            let term=base+bias;
            scores[k]+=term;
        }
    }
    for k in 0..N {scores[k]*=number(rows[k],layout.factor_offset());}
    if scores.iter().any(|score|!score.is_finite()) {return Err(invalid("分组INT8恢复分数溢出"));}
    add(&stats.direct,N as u64);
    Ok(scores)
}
pub(super) fn self_check()->io::Result<()> {
    if !available() {return Err(io::Error::new(io::ErrorKind::Unsupported,"分组INT8要求AVX512VNNI"));}
    for pattern in 0..8 {
        let q:[u8;768]=std::array::from_fn(|i|match pattern {0=>0,1=>255,2=>128,_=>((i*73+pattern*29)&255) as u8});
        let rows:[[u8;868];4]=std::array::from_fn(|k|std::array::from_fn(|i|match pattern {
            0=>127,1=>128,2=>0,_=>((i*31+k*61+pattern*17)&255) as u8,
        }));
        let refs:[&[u8];4]=std::array::from_fn(|k|rows[k].as_slice());
        for start in (0..768).step_by(128) {
            let four=integers(&q,refs,start,128);
            for k in 0..4 {
                let expected=(start..start+128).map(|i|(q[i] as i64-128)*(rows[k][i] as i8 as i64)).sum::<i64>();
                let one=integers(&q,[rows[k].as_slice()],start,128)[0];
                if four[k] as i64!=expected || one!=four[k] {return Err(io::Error::other("refine VNNI与独立i64极值oracle不一致"));}
            }
        }
    }
    for query_mode in [QueryMode::Global,QueryMode::Local] {
        for scale_mode in [ScaleMode::Norm,ScaleMode::Ls] {
            for pattern in 0..4 {
                let layout=Layout {group_size:128,norm_correction:true,scale_mode,query_mode};
                let stats=Stats::default();
                let values:[f32;768]=std::array::from_fn(|i|match pattern {
                    0=>0.,1=>if i/128==0 {0.}else{0.5},
                    _=>if i%3==0 {-((i/128+1) as f32)}else{0.25},
                });
                let query=Query::new(&values,&stats,layout)?;
                let mut rows=[[0u8;868];4];
                for (k,row) in rows.iter_mut().enumerate() {
                    for i in 0..768 {
                        row[i]=if pattern<2 {0}else{(((i*17+k*23)%255) as i32-127) as i8 as u8};
                    }
                    for block in 0..6 {
                        let scale=(block+1) as f32*0.125;
                        let bias=k as f32-1.5;
                        row[768+block*8..772+block*8].copy_from_slice(&scale.to_le_bytes());
                        row[772+block*8..776+block*8].copy_from_slice(&bias.to_le_bytes());
                    }
                    row[816..820].copy_from_slice(&0.75f32.to_le_bytes());
                    row[820..824].copy_from_slice(&1.25f32.to_le_bytes());
                }
                let four:[f32;4]=score(&query,std::array::from_fn(|k|rows[k].as_slice()),&stats,layout)?;
                for k in 0..4 {
                    let one=score(&query,[rows[k].as_slice()],&stats,layout)?[0];
                    let mut expected=0f32;
                    for block in 0..6 {
                        let start=block*128;
                        let dot=(start..start+128).map(|i|(query.bytes[i] as i64-128)*(rows[k][i] as i8 as i64)).sum::<i64>();
                        let qsum=(start..start+128).map(|i|query.bytes[i] as i64-128).sum::<i64>();
                        let base=(dot as f32*query.scales[block])*number(&rows[k],768+block*8);
                        let bias=(qsum as f32*query.scales[block])*number(&rows[k],772+block*8);
                        expected+=base+bias;
                    }
                    expected*=number(&rows[k],layout.factor_offset());
                    if one.to_bits()!=four[k].to_bits() || one.to_bits()!=expected.to_bits() {
                        return Err(io::Error::other("refine两query模式与两factor的one/four恢复不一致"));
                    }
                }
            }
        }
    }
    Ok(())
}
