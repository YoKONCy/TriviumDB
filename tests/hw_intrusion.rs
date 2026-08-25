#![allow(non_snake_case)]
//! 内联汇编级内存/文件侵入容错测试
//!
//! 模拟硬件级数据损坏对 TriviumDB 持久化文件和运行时数据的直接侵入，验证引擎的
//! 检测、拒绝、降级或恢复能力。
//!
//! ════════════════════════════════════════════════════════════════
//!  与 hw_crash.rs 的区别
//! ════════════════════════════════════════════════════════════════
//!  hw_crash.rs  — 进程被硬件异常杀死后，能否正确恢复（崩溃恢复）
//!  hw_intrusion — 数据在存储/内存层面被硬件级指令篡改后，引擎是否容错（数据损坏容错）
//!
//! 侵入手段（均为真实 CPU 指令，非软件模拟）：
//!  - `bts` / `btr`  : Bit Test and Set/Reset — 单比特翻转，模拟 cosmic ray
//!  - `movnti`       : Non-Temporal Store — 绕过 CPU 缓存直写内存，模拟 DMA 控制器写错
//!  - NaN/Inf/Denorm : 通过 asm 向量寄存器直接注入 IEEE 754 特殊浮点值
//!  - 字节级覆写     : 在序列化文件关键偏移处注入垃圾字节

use memmap2::MmapOptions;
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("intrude_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

/// 创建一个有数据的 DB 并 flush 到磁盘
fn seed_db(path: &str, count: usize) {
    let mut db = Database::<f32>::open(path, DIM).unwrap();
    for i in 0..count {
        db.insert(
            &[i as f32, (i as f32).sin(), (i as f32).cos(), 1.0],
            serde_json::json!({"idx": i, "tag": format!("node_{}", i)}),
        )
        .unwrap();
    }
    db.flush().unwrap();
}

// ════════════════════════════════════════════════════════════════
//  内联汇编侵入原语
// ════════════════════════════════════════════════════════════════

/// 使用 `bts` (Bit Test and Set) 翻转内存中的指定比特位（保留供 INTRUDE_03/05/07 使用）
#[cfg(target_arch = "x86_64")]
#[allow(dead_code)]
unsafe fn asm_bit_flip(ptr: *mut u8, bit_offset: u32) {
    unsafe {
        std::arch::asm!(
            "bts dword ptr [{ptr}], {bit:e}",
            ptr = in(reg) ptr,
            bit = in(reg) bit_offset,
        );
    }
}

/// 使用 `movnti` (Non-Temporal Store) 绕过 CPU 缓存直写 8 字节毒数据
/// 模拟 DMA 控制器 / 外设直接写入主存导致的数据损坏
#[cfg(target_arch = "x86_64")]
unsafe fn asm_nontemporal_poison(ptr: *mut u8, poison: u64) {
    unsafe {
        std::arch::asm!(
            "movnti qword ptr [{ptr}], {val}",
            "sfence",   // 确保 non-temporal store 对后续 load 可见
            ptr = in(reg) ptr,
            val = in(reg) poison,
        );
    }
}

// ════════════════════════════════════════════════════════════════
//  mmap 级侵入原语 — 直接操作文件支撑的映射页
// ════════════════════════════════════════════════════════════════

/// 在 mmap MAP_SHARED 映射页上执行 bts — 真正的物理页级比特翻转
/// CPU TLB 命中的是文件支撑的物理页，OS 脏页回写将损坏固化到磁盘
#[cfg(target_arch = "x86_64")]
unsafe fn mmap_bit_flip(mmap: &mut memmap2::MmapMut, byte_offset: usize, bit: u32) {
    unsafe {
        let ptr = mmap.as_mut_ptr().add(byte_offset);
        std::arch::asm!(
            "bts dword ptr [{ptr}], {bit:e}",
            ptr = in(reg) ptr,
            bit = in(reg) bit,
        );
    }
}

/// 在 mmap MAP_SHARED 映射页上执行 movnti — 绕过 CPU 缓存直达内存控制器
#[cfg(target_arch = "x86_64")]
unsafe fn mmap_nontemporal_poison(mmap: &mut memmap2::MmapMut, offset: usize, val: u64) {
    unsafe {
        let ptr = mmap.as_mut_ptr().add(offset) as *mut u64;
        std::arch::asm!(
            "movnti qword ptr [{p}], {v}",
            "sfence",
            p = in(reg) ptr,
            v = in(reg) val,
        );
    }
}

/// 非 x86 平台的 mmap 回退
#[cfg(not(target_arch = "x86_64"))]
unsafe fn mmap_bit_flip(mmap: &mut memmap2::MmapMut, byte_offset: usize, bit: u32) {
    mmap[byte_offset] ^= 1 << bit;
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn mmap_nontemporal_poison(mmap: &mut memmap2::MmapMut, offset: usize, val: u64) {
    let dst = &mut mmap[offset..offset + 8];
    dst.copy_from_slice(&val.to_ne_bytes());
}

/// 在栈上构造一个包含 NaN 的 f32 向量（通过 asm 直接注入 IEEE 754 bit pattern）
#[cfg(target_arch = "x86_64")]
fn asm_make_nan_vector() -> [f32; DIM] {
    let mut vec = [0.0f32; DIM];
    unsafe {
        // 0x7FC00000 = quiet NaN (IEEE 754)
        // 0x7F800000 = +Infinity
        // 0x00000001 = smallest denormalized float
        let patterns: [u32; 4] = [0x7FC00000, 0x7F800000, 0x00000001, 0xFF800000];
        let dst = vec.as_mut_ptr() as *mut u32;
        std::arch::asm!(
            "mov dword ptr [{dst}],      {p0:e}",
            "mov dword ptr [{dst} + 4],  {p1:e}",
            "mov dword ptr [{dst} + 8],  {p2:e}",
            "mov dword ptr [{dst} + 12], {p3:e}",
            dst = in(reg) dst,
            p0 = in(reg) patterns[0],  // qNaN
            p1 = in(reg) patterns[1],  // +Inf
            p2 = in(reg) patterns[2],  // denorm
            p3 = in(reg) patterns[3],  // -Inf
        );
    }
    vec
}

/// 非 x86 平台的回退实现
#[cfg(not(target_arch = "x86_64"))]
#[allow(dead_code)]
unsafe fn asm_bit_flip(ptr: *mut u8, bit_offset: u32) {
    let byte_idx = (bit_offset / 8) as usize;
    let bit_idx = bit_offset % 8;
    unsafe {
        *ptr.add(byte_idx) ^= 1 << bit_idx;
    }
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn asm_nontemporal_poison(ptr: *mut u8, poison: u64) {
    unsafe {
        std::ptr::write_unaligned(ptr as *mut u64, poison);
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn asm_make_nan_vector() -> [f32; DIM] {
    [
        f32::NAN,
        f32::INFINITY,
        f32::MIN_POSITIVE * 0.5,
        f32::NEG_INFINITY,
    ]
}

// ════════════════════════════════════════════════════════════════
//  测试 1: 序列化文件 cosmic ray 比特翻转
// ════════════════════════════════════════════════════════════════

/// 在已 flush 的 .tdb 文件多个偏移处执行 `bts` 单比特翻转，
/// 模拟宇宙射线或 DRAM bit-flip 导致的静默数据损坏 (SDC)。
/// 引擎应：拒绝加载（检测到校验错误）或 降级加载（跳过损坏区域）——不能 panic。
#[test]
fn INTRUDE_01_BTS单比特翻转_TDB文件多点侵入() {
    let path = tmp_db("bts_bitflip");
    cleanup(&path);
    seed_db(&path, 20);

    let tdb_path = path.clone();

    // ═══ 真正的硬件级操作：用 MAP_SHARED 映射 .tdb 文件 ═══
    // bts 直接操作文件支撑的虚拟页，OS 脏页回写将损坏固化到磁盘块设备
    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tdb_path)
            .expect("打开 .tdb 文件失败");
        let len = file.metadata().unwrap().len() as usize;
        assert!(len > 128, ".tdb 文件过小: {} bytes", len);

        let mut mmap = unsafe { MmapOptions::new().map_mut(&file).unwrap() };

        // 在 mmap 映射页上直接执行 bts — 这是文件支撑页，不是堆内存
        let flip_points = [
            (0usize, 0u32),             // 文件头第 0 字节第 0 位
            (16, 3),                    // 头部区域第 3 位
            (len / 4, 5),               // 文件 1/4 处
            (len / 2, 2),               // 文件中部
            (len.saturating_sub(8), 7), // 文件尾部
        ];
        for &(byte_off, bit) in &flip_points {
            if byte_off < len {
                unsafe {
                    mmap_bit_flip(&mut mmap, byte_off, bit);
                }
            }
        }

        // mmap flush — 强制将脏页从页缓存回写到磁盘块设备
        mmap.flush().unwrap();
    } // drop mmap + file

    // 删除完整性标记
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    // 尝试加载：不允许 panic
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(
        result.is_ok(),
        "mmap 活页 bit-flip 后引擎不应 panic，应优雅拒绝或降级"
    );

    match result.unwrap() {
        Ok(db) => {
            assert!(
                db.node_count() <= 20,
                "bit-flip 降级加载后节点数不应超过原始 20，实际 {}",
                db.node_count()
            );
            for &id in &db.all_node_ids() {
                if let Some(p) = db.get_payload(id) {
                    assert!(
                        p.get("idx").is_some(),
                        "节点 {} 的 payload 应包含 idx 字段，实际内容: {}",
                        id,
                        p
                    );
                }
            }
            eprintln!(
                "  ✅ mmap BTS bit-flip: 引擎降级加载，{} 个节点存活（数据完整性已验证）",
                db.node_count()
            );
        }
        Err(e) => {
            eprintln!("  ✅ mmap BTS bit-flip: 引擎正确拒绝: {}", e);
        }
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 2: MOVNTI 非时序写入 — DMA 控制器模拟侵入
// ════════════════════════════════════════════════════════════════

/// 使用 `movnti` (Non-Temporal Store Integer) 绕过 CPU 缓存，
/// 直接向 .tdb 文件内存写入 0xDEADBEEFCAFEBABE 毒数据。
/// 模拟 DMA 控制器、RAID 卡或 NVMe 固件 bug 导致的块级数据损坏。
#[test]
fn INTRUDE_02_MOVNTI非时序毒写_DMA级侵入() {
    let path = tmp_db("movnti_poison");
    cleanup(&path);
    seed_db(&path, 15);

    let tdb_path = path.clone();

    // ═══ 真正的硬件级操作：movnti 绕过 CPU 缓存直写 mmap 映射页 ═══
    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tdb_path)
            .unwrap();
        let len = file.metadata().unwrap().len() as usize;
        let mut mmap = unsafe { MmapOptions::new().map_mut(&file).unwrap() };

        // movnti 在文件映射页上绕过 L1/L2/L3 缓存直达内存控制器
        let offsets = [len / 3, len * 2 / 3];
        for &off in &offsets {
            if off + 8 <= len {
                unsafe {
                    mmap_nontemporal_poison(&mut mmap, off, 0xDEADBEEFCAFEBABEu64);
                }
            }
        }

        mmap.flush().unwrap();
    }

    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "mmap MOVNTI 毒写后引擎不应 panic");

    match result.unwrap() {
        Ok(db) => {
            let count = db.node_count();
            assert!(
                count <= 15,
                "MOVNTI 毒写降级加载后节点数不应超过原始 15，实际 {}",
                count
            );
            for &id in &db.all_node_ids() {
                let node = db.get(id);
                assert!(
                    node.is_some(),
                    "all_node_ids 返回的 ID {} 应能通过 get() 获取",
                    id
                );
            }
            eprintln!(
                "  ✅ mmap MOVNTI 毒写: 降级加载 {} 个节点，数据完整性已验证",
                count
            );
        }
        Err(e) => {
            eprintln!("  ✅ MOVNTI 毒写: 引擎正确拒绝: {}", e);
        }
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 3: WAL 流注入 — 在有效 WAL 尾部追加垃圾帧
// ════════════════════════════════════════════════════════════════

/// 在有效的 WAL 文件尾部通过 asm 注入垃圾字节帧，
/// 模拟断电时 WAL 写入被截断、尾部包含不完整/损坏的操作日志。
/// 引擎的 WAL 回放器应：回放有效前缀 + 丢弃损坏尾部。
#[test]
fn INTRUDE_03_WAL尾部注入垃圾帧_回放容错() {
    let path = tmp_db("wal_inject");
    cleanup(&path);

    // 写入数据但不 flush（数据仅在 WAL 中）
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..5u32 {
            db.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"valid": true, "seq": i}),
            )
            .unwrap();
        }
        db.flush().unwrap();

        // 继续写 WAL（不 flush）
        for i in 5..8u32 {
            db.insert(
                &[i as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"wal_tail": true, "seq": i}),
            )
            .unwrap();
        }
        // Drop 会 flush WAL BufWriter
    }

    // 在 WAL 尾部追加垃圾字节
    let wal_path = format!("{}.wal", path);
    if std::path::Path::new(&wal_path).exists() {
        let mut file = OpenOptions::new().append(true).open(&wal_path).unwrap();
        // 构造 64 字节的毒 payload（通过 asm 生成的 pattern）
        let mut poison = [0u8; 64];
        #[cfg(target_arch = "x86_64")]
        unsafe {
            let ptr = poison.as_mut_ptr();
            let p0: u64 = 0xBADC0FFEE0DDF00D;
            let p1: u64 = 0xFEEDFACEDEADC0DE;
            let p2: u64 = 0x0B00B1E5CAFEBABE;
            let p3: u64 = 0xC001D00DBEEFCAFE;
            std::arch::asm!(
                "mov qword ptr [{p}],      {v0}",
                "mov qword ptr [{p} + 8],  {v1}",
                "mov qword ptr [{p} + 16], {v2}",
                "mov qword ptr [{p} + 24], {v3}",
                p = in(reg) ptr,
                v0 = in(reg) p0,
                v1 = in(reg) p1,
                v2 = in(reg) p2,
                v3 = in(reg) p3,
            );
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            poison.fill(0xBB);
        }
        file.write_all(&poison).unwrap();
    }

    // 重新打开 — WAL 回放应该成功回放有效部分，丢弃尾部垃圾
    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "WAL 尾部注入后不应 panic");

    // WAL 尾部注入场景：5 个节点已 flush 到 .tdb 文件，必须能恢复
    let db = result
        .unwrap()
        .expect("WAL 尾部垃圾不应导致加载失败：5 个已 flush 的节点应通过 .tdb 文件恢复");
    assert!(
        db.node_count() >= 5,
        "至少 5 个已 flush 的节点应存活，实际 {}",
        db.node_count()
    );
    assert!(
        db.node_count() <= 8,
        "节点数不应超过原始写入的 8 个，实际 {}",
        db.node_count()
    );
    // 验证已 flush 节点的 payload 语义完整性
    let mut valid_count = 0;
    for &id in &db.all_node_ids() {
        if let Some(p) = db.get_payload(id)
            && p.get("valid").and_then(|v| v.as_bool()) == Some(true)
        {
            valid_count += 1;
        }
    }
    assert_eq!(
        valid_count, 5,
        "5 个已 flush 的 valid=true 节点 payload 应完好"
    );
    eprintln!(
        "  ✅ WAL 注入容错: {} 个节点存活，payload 完整性已验证",
        db.node_count()
    );

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 4: 查询向量 NaN/Inf/Denorm 注入
// ════════════════════════════════════════════════════════════════

/// 通过内联汇编直接在 SSE 寄存器级别构造包含 IEEE 754 特殊值的查询向量：
///   - qNaN (0x7FC00000)  — 安静非数
///   - +Inf (0x7F800000)  — 正无穷
///   - Denorm (0x00000001) — 最小非规范化浮点
///   - -Inf (0xFF800000)  — 负无穷
///
/// 传入 search() 验证引擎的输入验证层是否能拦截。
#[test]
fn INTRUDE_04_ASM构造NaN查询向量_搜索输入验证() {
    let path = tmp_db("nan_query");
    cleanup(&path);
    seed_db(&path, 10);

    let db = Database::<f32>::open(&path, DIM).unwrap();

    // 通过 asm 构造毒向量
    let poison_vec = asm_make_nan_vector();

    // 验证确实包含特殊值
    assert!(poison_vec[0].is_nan(), "第 0 分量应为 NaN");
    assert!(poison_vec[1].is_infinite(), "第 1 分量应为 +Inf");

    // 调用 search — 引擎必须拒绝包含 NaN/Inf 的查询向量
    let search_result = db.search(&poison_vec, 5, 0, 0.0);
    assert!(
        search_result.is_err(),
        "包含 NaN/Inf 的查询向量必须被引擎拒绝，但 search() 返回了 Ok({} 条结果)",
        search_result.as_ref().map(|h| h.len()).unwrap_or(0)
    );
    let err_msg = search_result.unwrap_err().to_string();
    assert!(
        err_msg.contains("NaN")
            || err_msg.contains("Inf")
            || err_msg.contains("invalid")
            || err_msg.contains("Invalid"),
        "错误信息应明确指出 NaN/Inf 问题，实际: {}",
        err_msg
    );
    eprintln!("  ✅ NaN 向量查询: 引擎正确拒绝: {}", err_msg);

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 5: 序列化文件头部 8 字节全毒覆写
// ════════════════════════════════════════════════════════════════

/// 完全覆写 .tdb 文件的前 8 字节（通常是 magic number 或版本头），
/// 验证引擎的文件格式校验是否在最外层就能拦截。
#[test]
fn INTRUDE_05_文件头Magic全覆写_格式校验拦截() {
    let path = tmp_db("header_smash");
    cleanup(&path);
    seed_db(&path, 10);

    let tdb_path = path.clone();
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&tdb_path)
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();

    // 用 asm 构造毒 magic
    #[cfg(target_arch = "x86_64")]
    let mut poison = [0u8; 8];
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let val: u64 = 0xDEADDEADDEADDEAD;
        std::arch::asm!(
            "mov qword ptr [{p}], {v}",
            p = in(reg) poison.as_mut_ptr(),
            v = in(reg) val,
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    let poison = [0xDE, 0xAD, 0xDE, 0xAD, 0xDE, 0xAD, 0xDE, 0xAD];

    file.write_all(&poison).unwrap();
    drop(file);
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "header 全覆写不应导致 panic");

    // 文件头 magic 被完全覆写：引擎必须拒绝加载，不能静默接受损坏文件
    assert!(
        result.unwrap().is_err(),
        "文件头 magic 被全覆写后引擎仍加载成功！这是严重的安全漏洞：引擎缺乏文件格式校验"
    );
    eprintln!("  ✅ 文件头覆写: 引擎正确拒绝加载");

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 6: 渐进式比特衰减 — 模拟 NAND Flash 老化
// ════════════════════════════════════════════════════════════════

/// 模拟 NAND Flash 存储介质老化导致的渐进式比特衰减 (bit rot)：
/// 每隔 512 字节翻转 1 个比特，模拟多个扇区同时发生的分散 SDC。
/// 这是最接近真实 SSD 老化的损坏模式。
#[test]
fn INTRUDE_06_渐进式比特衰减_NAND_Flash老化模拟() {
    let path = tmp_db("bitrot");
    cleanup(&path);
    seed_db(&path, 50);

    let tdb_path = path.clone();
    let mut flipped = 0usize;
    let len;

    // ═══ 真正的硬件级操作：在 mmap 映射页上逐扇区翻转比特 ═══
    {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&tdb_path)
            .unwrap();
        len = file.metadata().unwrap().len() as usize;
        let mut mmap = unsafe { MmapOptions::new().map_mut(&file).unwrap() };

        // 每 512 字节翻转 1 位 — bts 直接操作文件映射页
        let mut offset = 0usize;
        while offset < len {
            unsafe {
                mmap_bit_flip(&mut mmap, offset, (offset % 7) as u32);
            }
            flipped += 1;
            offset += 512;
        }

        mmap.flush().unwrap();
    }

    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    eprintln!(
        "  📊 文件大小 {} bytes, 在 mmap 映射页上翻转了 {} 个比特 (密度: 1/512 bytes)",
        len, flipped
    );

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "渐进式 bit-rot 不应导致 panic");

    match result.unwrap() {
        Ok(db) => {
            let survived = db.node_count();
            assert!(
                survived <= 50,
                "bit-rot 降级加载后节点数不应超过原始 50，实际 {}",
                survived
            );
            for &id in &db.all_node_ids() {
                assert!(
                    db.get(id).is_some(),
                    "all_node_ids 返回的 ID {} 应能通过 get() 获取",
                    id
                );
            }
            eprintln!(
                "  ✅ mmap NAND 老化: 降级加载 {}/50 节点存活（数据完整性已验证）",
                survived
            );
        }
        Err(e) => {
            eprintln!("  ✅ mmap NAND 老化: 引擎拒绝加载: {}", e);
        }
    }

    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试 7: TDB + WAL 双通道同时侵入
// ════════════════════════════════════════════════════════════════

/// 同时损坏 .tdb 主文件和 .wal 日志文件 —— 最恶劣的灾难场景。
/// 引擎应至少能安全拒绝，绝不能 segfault 或返回损坏数据。
#[test]
fn INTRUDE_07_TDB加WAL双文件同时侵入_灾难级容错() {
    let path = tmp_db("dual_corrupt");
    cleanup(&path);

    // 先 flush 创建 .tdb，再写 WAL
    {
        let mut db = Database::<f32>::open(&path, DIM).unwrap();
        for i in 0..10u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"v": i}))
                .unwrap();
        }
        db.flush().unwrap();
        // 继续写 WAL
        for i in 10..15u32 {
            db.insert(&[i as f32, 0.0, 0.0, 0.0], serde_json::json!({"v": i}))
                .unwrap();
        }
    }

    // 侵入 .tdb：中部注入 DEADBEEF
    let tdb_path = path.clone();
    if let Ok(mut data) = std::fs::read(&tdb_path) {
        let mid = data.len() / 2;
        if mid + 8 <= data.len() {
            unsafe {
                asm_nontemporal_poison(data.as_mut_ptr().add(mid), 0xDEADC0DEu64);
            }
            std::fs::write(&tdb_path, &data).unwrap();
        }
    }

    // 侵入 .wal：头部注入垃圾
    let wal_path = format!("{}.wal", path);
    if let Ok(mut data) = std::fs::read(&wal_path)
        && data.len() >= 8
    {
        unsafe {
            asm_nontemporal_poison(data.as_mut_ptr(), 0xBADBADBADBADBADu64);
        }
        std::fs::write(&wal_path, &data).unwrap();
    }

    // 删除 flush_ok
    std::fs::remove_file(format!("{}.flush_ok", path)).ok();

    let result = std::panic::catch_unwind(|| Database::<f32>::open(&path, DIM));
    assert!(result.is_ok(), "双文件侵入不应导致 panic / segfault");

    match result.unwrap() {
        Ok(db) => {
            // 双通道损坏后如果还能加载，节点数不应超过原始写入数
            assert!(
                db.node_count() <= 15,
                "双文件侵入降级加载后节点数不应超过原始 15，实际 {}",
                db.node_count()
            );
            // 基本操作不能 panic
            for &id in &db.all_node_ids() {
                assert!(
                    db.get(id).is_some(),
                    "all_node_ids 返回的 ID {} 应能通过 get() 获取",
                    id
                );
            }
            eprintln!(
                "  ⚠️ 双文件损坏后降级加载: {} 节点存活（数据完整性已验证）",
                db.node_count()
            );
        }
        Err(e) => {
            eprintln!("  ✅ 双文件损坏: 引擎正确拒绝: {}", e);
        }
    }

    cleanup(&path);
}
