//! 分组INT8文件校验与暖映射，只提供原生整数精排。
use super::*;
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) enum Encoding {Fp32,Int8}
impl Encoding {
    pub fn parse(c:&Value)->io::Result<Self> {match c["cold_encoding"].as_str().unwrap_or("int8") {
        "int8"=>Ok(Self::Int8),_=>Err(invalid("本轮数据库只允许分组INT8编码")),
    }}
    pub fn name(self)->&'static str {"int8"}
    pub fn payload(self,dim:usize,layout:direct_int8::Layout)->usize {
        if self==Self::Int8 {layout.payload()}else{dim*4}
    }
}
fn residency(map:&memmap2::Mmap)->io::Result<Value> {
    let mut pages=vec![0u8;map.len().div_ceil(OSPAGE)];
    // SAFETY：只读映射在调用期间有效，输出数组覆盖所有操作系统页。
    let result=unsafe {mincore(map.as_ptr() as *mut std::ffi::c_void,map.len(),pages.as_mut_ptr())};
    if result!=0 {return Err(io::Error::last_os_error());}
    let resident=pages.iter().filter(|&&page|page&1!=0).count();
    Ok(json!({"pages":pages.len(),"resident_pages":resident,"resident_fraction":resident as f64/pages.len() as f64}))
}
pub(super) fn checked_map(path:&Path,n:usize,layout:direct_int8::Layout)->io::Result<(memmap2::Mmap,Value)> {
    let file=fs::File::open(path)?;
    if file.metadata()?.len()!=n as u64*1024 {return Err(invalid("分组INT8文件长度须为节点数乘1024"));}
    // SAFETY：冻结输入在服务期间只读且长度固定，所有按行切片都在已验证文件长度内。
    let map=unsafe {memmap2::MmapOptions::new().map(&file)?};
    for row in map.chunks_exact(1024) {
        direct_int8::validate(row,layout)?;
        if row[layout.payload()..].iter().any(|&byte|byte!=0) {return Err(invalid("分组INT8元数据布局不匹配或填充字节非零"));}
    }
    for offset in (0..map.len()).step_by(OSPAGE) {
        // SAFETY：offset严格小于映射长度，每个系统页只读取一个有效字节。
        std::hint::black_box(unsafe {std::ptr::read_volatile(map.as_ptr().add(offset))});
    }
    let resident=residency(&map)?;
    if resident["resident_pages"]!=resident["pages"] {return Err(io::Error::other("分组INT8暖映射未完整驻留"));}
    Ok((map,resident))
}
pub(super) struct WarmSource<'a> {
    pub map:&'a memmap2::Mmap,pub n:usize,pub slots:Option<&'a mut Vec<usize>>,
    pub mode:direct_int8::Mode,pub query:Option<direct_int8::Query>,pub stats:&'a direct_int8::Stats,
    pub layout:direct_int8::Layout,
}
impl ColdVectorSource for WarmSource<'_> {
    fn prepare_query(&mut self,q:&[f32])->io::Result<()> {
        self.query=Some(direct_int8::Query::new(q,self.stats,self.layout)?);
        Ok(())
    }
    fn direct_one(&mut self,slot:usize)->io::Result<Option<f32>> {
        if slot>=self.n {return Err(invalid("暖分组INT8行号越界"));}
        if let Some(record)=self.slots.as_mut() {record.push(slot);}
        let query=self.query.as_ref().ok_or_else(||invalid("暖分组INT8 query尚未准备"))?;
        let row=&self.map[slot*1024..slot*1024+self.layout.payload()];
        Ok(Some(direct_int8::score(query,[row],self.stats,self.layout)?[0]))
    }
    fn direct_four(&mut self,slots:[usize;4])->io::Result<Option<[f32;4]>> {
        if slots.iter().any(|&slot|slot>=self.n) {return Err(invalid("暖分组INT8行号越界"));}
        if let Some(record)=self.slots.as_mut() {record.extend_from_slice(&slots);}
        let rows=std::array::from_fn(|k|&self.map[slots[k]*1024..slots[k]*1024+self.layout.payload()]);
        let query=self.query.as_ref().ok_or_else(||invalid("暖分组INT8 query尚未准备"))?;
        Ok(Some(direct_int8::score(query,rows,self.stats,self.layout)?))
    }
    fn fill(&mut self,_slot:usize,_buffer:&mut Vec<f32>)->io::Result<bool> {
        Err(invalid("分组INT8禁止解码候选为FP32"))
    }
}
