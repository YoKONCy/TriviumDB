#![allow(non_snake_case)]
//! 内联汇编级硬件中断崩溃恢复测试
//!
//! 覆盖 6 种 CPU 异常向量：
//! - #UD (ud2)         — Undefined Instruction
//! - #BP (int3)        — Debug Breakpoint Trap
//! - #DE (div/0)       — Divide-by-Zero Error
//! - #GP (null deref)  — General Protection / Access Violation
//! - Register Poison   — 寄存器全毒化后崩溃
//! - CLFLUSH+MFENCE    — 强制 cache line 刷盘后崩溃
//!
//! 以及高级场景：
//! - 连续多次崩溃恢复（累积 WAL 回放）
//! - 大 Payload 压力崩溃（WAL BufWriter 边界）

use std::process::Command;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("hwcrash_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    crate::cleanup_files::database(path);
}

// ════════════════════════════════════════════════════════════════
//  内联汇编硬件中断原语（6 种异常向量）
// ════════════════════════════════════════════════════════════════

/// #UD — Undefined Instruction (x86: ud2 / arm: udf)
#[allow(unreachable_code)]
fn hw_crash_ud2() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!("ud2", options(noreturn));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("udf #0", options(noreturn));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

/// #BP — Debug Breakpoint Trap (x86: int3 / arm: brk)
#[allow(unreachable_code)]
fn hw_crash_int3() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!("int3", options(noreturn));
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!("brk #0", options(noreturn));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

/// #DE — Divide-by-Zero Error
/// x86: `div` with zero divisor → CPU #DE exception → SIGFPE / STATUS_INTEGER_DIVIDE_BY_ZERO
#[allow(unreachable_code)]
fn hw_crash_div_zero() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // xor ecx,ecx → ecx=0; xor edx,edx; mov eax,1; div ecx → 1/0 = #DE
        std::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "mov eax, 1",
            "div ecx",
            options(noreturn)
        );
    }
    #[cfg(target_arch = "aarch64")]
    // ARM 的 UDIV 除零不触发异常（返回0），回退到 udf
    unsafe {
        std::arch::asm!("udf #1", options(noreturn));
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

/// #GP / Access Violation — NULL pointer dereference via asm
/// 直接通过汇编写入地址 0x0，触发页错误
#[allow(unreachable_code)]
fn hw_crash_null_deref() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "xor rax, rax",             // rax = 0 (NULL)
            "mov qword ptr [rax], rax", // 写入 NULL → Access Violation
            options(noreturn)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "mov x0, #0",
            "str x0, [x0]", // 写入 NULL → Bus Error
            options(noreturn)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

/// Register Poison + UD2 — 全寄存器毒化后崩溃
/// 模拟硬件故障导致的寄存器状态全损坏场景
/// 将所有通用寄存器清零/毒化，然后触发 UD2
/// 验证：OS 在杀进程时不会因寄存器状态异常导致额外的文件损坏
#[allow(unreachable_code)]
fn hw_crash_register_poison() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            // 毒化所有 caller-saved 寄存器
            "xor rax, rax",
            "xor rcx, rcx",
            "xor rdx, rdx",
            "xor r8, r8",
            "xor r9, r9",
            "xor r10, r10",
            "xor r11, r11",
            // SSE 寄存器也全部清零
            "xorps xmm0, xmm0",
            "xorps xmm1, xmm1",
            "xorps xmm2, xmm2",
            "xorps xmm3, xmm3",
            // 毒化完成，硬件崩溃
            "ud2",
            options(noreturn)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "mov x0, #0",
            "mov x1, #0",
            "mov x2, #0",
            "mov x3, #0",
            "movi v0.4s, #0",
            "movi v1.4s, #0",
            "udf #2",
            options(noreturn)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

/// CLFLUSH + MFENCE + UD2 — 强制 cache line 写回后崩溃
/// 在崩溃前显式执行内存屏障和 cache line flush，
/// 确保所有 pending 的 store buffer 被排空到物理内存
/// 这模拟了「数据刚刚到达 DRAM 但 fsync 尚未完成」的时间窗口
#[allow(unreachable_code)]
fn hw_crash_mfence_ud2() -> ! {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        std::arch::asm!(
            "mfence", // 全序内存屏障 — 排空 store buffer
            "sfence", // store fence — 确保所有写操作对其他核可见
            "ud2",
            options(noreturn)
        );
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        std::arch::asm!(
            "dmb sy", // Data Memory Barrier (full system)
            "dsb sy", // Data Synchronization Barrier
            "udf #3",
            options(noreturn)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    std::process::abort();
}

// ════════════════════════════════════════════════════════════════
//  子进程入口与调度
// ════════════════════════════════════════════════════════════════

fn child_main() {
    let db_path = std::env::var("HW_CRASH_DB_PATH").expect("缺少 HW_CRASH_DB_PATH");
    let mode = std::env::var("HW_CRASH_MODE").expect("缺少 HW_CRASH_MODE");

    let mut db = triviumdb::Database::<f32>::open_with_config(
        &db_path,
        triviumdb::database::Config {
            dim: DIM,
            sync_mode: triviumdb::storage::wal::SyncMode::Normal,
            ..Default::default()
        },
    )
    .unwrap();

    // 通用的「flush 后追加 WAL 再崩溃」模式
    let write_base = std::env::var("HW_CRASH_BASE")
        .map(|value| value.parse::<u64>().expect("崩溃写入基数必须为整数"))
        .unwrap_or(0);
    let do_flush_then_wal = |db: &mut triviumdb::Database<f32>| {
        for i in 0..5u32 {
            let seq = write_base + u64::from(i);
            db.insert_with_id(
                seq + 1,
                &[seq as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"phase": "flushed", "seq": seq}),
            )
            .unwrap();
        }
        db.flush().unwrap();
        for i in 5..8u32 {
            let seq = write_base + u64::from(i);
            db.insert_with_id(
                seq + 1,
                &[seq as f32, 0.0, 0.0, 0.0],
                serde_json::json!({"phase": "wal_only", "seq": seq}),
            )
            .unwrap();
        }
    };

    match mode.as_str() {
        "ud2_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_ud2();
        }
        "int3_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_int3();
        }
        "div_zero_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_div_zero();
        }
        "null_deref_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_null_deref();
        }
        "regpoison_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_register_poison();
        }
        "mfence_after_flush" => {
            do_flush_then_wal(&mut db);
            hw_crash_mfence_ud2();
        }
        "ud2_mid_write" => {
            for i in 0..10u32 {
                db.insert(
                    &[i as f32, 1.0, 0.0, 0.0],
                    serde_json::json!({"phase": "unflushed", "seq": i}),
                )
                .unwrap();
            }
            std::mem::forget(db);
            hw_crash_ud2();
        }
        "big_payload_crash" => {
            // 大 Payload 压力：每个节点携带 ~8KB JSON，压测 WAL BufWriter
            let fat_value = "X".repeat(8000);
            for i in 0..5u32 {
                db.insert_with_id(
                    u64::from(i) + 1,
                    &[i as f32, 2.0, 0.0, 0.0],
                    serde_json::json!({"fat": fat_value, "seq": i}),
                )
                .unwrap();
            }
            db.flush().unwrap();
            for i in 5..8u32 {
                db.insert_with_id(
                    u64::from(i) + 1,
                    &[i as f32, 2.0, 0.0, 0.0],
                    serde_json::json!({"fat": fat_value, "seq": i}),
                )
                .unwrap();
            }
            hw_crash_ud2();
        }
        "multi_crash_round" => {
            // 多轮崩溃的单轮：插入 3 个节点到 WAL，不 flush，崩溃
            let round: u32 = std::env::var("HW_CRASH_ROUND")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .unwrap();
            let base = round * 3;
            for i in base..base + 3 {
                db.insert(
                    &[i as f32, 3.0, 0.0, 0.0],
                    serde_json::json!({"round": round, "seq": i}),
                )
                .unwrap();
            }
            // 不 flush，让 WAL 累积
            hw_crash_ud2();
        }
        _ => panic!("未知 HW_CRASH_MODE: {}", mode),
    }
}

fn run_crash_child(db_path: &str, mode: &str) -> std::process::ExitStatus {
    run_crash_child_env(db_path, mode, &[])
}

fn run_crash_child_env(
    db_path: &str,
    mode: &str,
    extra_env: &[(&str, &str)],
) -> std::process::ExitStatus {
    let exe = std::env::current_exe().expect("无法获取当前可执行文件路径");
    let mut cmd = Command::new(exe);
    cmd.env("HW_CRASH_DB_PATH", db_path)
        .env("HW_CRASH_MODE", mode)
        .env("HW_CRASH_CHILD", "1")
        .arg("--test-threads=1")
        .arg("__hw_crash_child_entry")
        .arg("--exact")
        .arg("--nocapture");
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    {
        // 故障样本由操作系统直接接收汇编触发的信号；保留 ASan 内存插桩与其余选项。
        let signal_options = match mode {
            "div_zero_after_flush" => "handle_sigfpe=0",
            "null_deref_after_flush" => "handle_segv=0:handle_sigbus=0",
            _ => "",
        };
        if !signal_options.is_empty() {
            let mut options = std::env::var_os("ASAN_OPTIONS").unwrap_or_default();
            if !options.is_empty() {
                options.push(":");
            }
            options.push(signal_options);
            cmd.env("ASAN_OPTIONS", options);
        }
    }
    let status = cmd.status().expect("启动子进程失败");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            status.signal().is_some(),
            "崩溃子进程必须由信号终止：{status}"
        );
    }
    #[cfg(windows)]
    assert!(
        status.code().is_some_and(|code| code < 0),
        "崩溃子进程必须以 Windows 异常状态终止：{status}"
    );
    status
}

#[test]
fn __hw_crash_child_entry() {
    if std::env::var("HW_CRASH_CHILD").is_err() {
        return;
    }
    child_main();
}

// ════════════════════════════════════════════════════════════════
//  通用验证辅助
// ════════════════════════════════════════════════════════════════

fn assert_crash_and_recover(path: &str, mode: &str, label: &str) {
    let status = run_crash_child(path, mode);
    assert!(
        !status.success(),
        "{}: 子进程应因硬件异常而非正常退出",
        label
    );

    let db = triviumdb::Database::<f32>::open(path, DIM)
        .unwrap_or_else(|e| panic!("{}: 崩溃后重新打开数据库失败: {}", label, e));

    // Normal 契约要求已返回成功的 WAL 写入抵御进程崩溃；此处未模拟操作系统断电。
    let mut ids = db.all_node_ids().unwrap();
    ids.sort_unstable();
    assert_eq!(
        ids,
        (1..=8).collect::<Vec<_>>(),
        "{label}：确认写入必须完整恢复"
    );
    for id in 1..=8 {
        let seq = id - 1;
        let phase = if seq < 5 { "flushed" } else { "wal_only" };
        let node = db.get(id).unwrap().expect("恢复节点必须包含完整记录");
        assert_eq!(node.vector, vec![seq as f32, 0.0, 0.0, 0.0]);
        assert_eq!(node.payload, serde_json::json!({"phase":phase, "seq":seq}));
    }

    eprintln!(
        "  ✅ {}: {} 个节点存活（5 flush + {} WAL）",
        label,
        db.node_count().unwrap(),
        db.node_count().unwrap().saturating_sub(5)
    );
}

// ════════════════════════════════════════════════════════════════
//  测试用例 — 基础异常向量
// ════════════════════════════════════════════════════════════════

/// #UD (Undefined Instruction) — CPU 异常向量 #6
#[test]
fn HW01_UD2_未定义指令异常_崩溃恢复() {
    let path = tmp_db("hw01_ud2");
    cleanup(&path);
    assert_crash_and_recover(&path, "ud2_after_flush", "#UD ud2");
    cleanup(&path);
}

/// #BP (Breakpoint Trap) — CPU 异常向量 #3
#[test]
fn HW02_INT3_断点陷阱_崩溃恢复() {
    let path = tmp_db("hw02_int3");
    cleanup(&path);
    assert_crash_and_recover(&path, "int3_after_flush", "#BP int3");
    cleanup(&path);
}

/// #DE (Divide Error) — CPU 异常向量 #0
/// x86 上通过 `div ecx` (ecx=0) 触发整数除零异常
#[test]
fn HW03_DIV0_除零异常_崩溃恢复() {
    let path = tmp_db("hw03_div0");
    cleanup(&path);
    assert_crash_and_recover(&path, "div_zero_after_flush", "#DE div/0");
    cleanup(&path);
}

/// #PF / #GP (Page Fault / General Protection) — 空指针写入
/// 通过 `mov [0], rax` 触发内存访问违例 (STATUS_ACCESS_VIOLATION)
#[test]
fn HW04_NULL_DEREF_空指针写入异常_崩溃恢复() {
    let path = tmp_db("hw04_null");
    cleanup(&path);
    assert_crash_and_recover(&path, "null_deref_after_flush", "#GP null-deref");
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试用例 — 寄存器/微架构级
// ════════════════════════════════════════════════════════════════

/// 寄存器全毒化 — GPR + SSE 寄存器全部清零后崩溃
/// 模拟硬件瞬态故障（cosmic ray / bit-flip）导致的寄存器状态全损
/// 验证：OS 清理进程时不会因寄存器状态异常导致额外文件损坏
#[test]
fn HW05_寄存器毒化_GPR和SSE全清零后崩溃恢复() {
    let path = tmp_db("hw05_regpoison");
    cleanup(&path);
    assert_crash_and_recover(&path, "regpoison_after_flush", "RegPoison GPR+SSE");
    cleanup(&path);
}

/// MFENCE + SFENCE 内存屏障 — 排空 store buffer 后崩溃
/// 模拟「CPU store buffer 已经排空到 DRAM，但 OS 的 fsync 尚未完成」的
/// 精确时间窗口。这是现实中断电数据丢失的最常见根因之一。
#[test]
fn HW06_MFENCE_内存屏障后崩溃恢复() {
    let path = tmp_db("hw06_mfence");
    cleanup(&path);
    assert_crash_and_recover(&path, "mfence_after_flush", "MFENCE+SFENCE barrier");
    cleanup(&path);
}

// ════════════════════════════════════════════════════════════════
//  测试用例 — 高级场景
// ════════════════════════════════════════════════════════════════

/// 写入中途崩溃 — mem::forget 绕过 Drop + UD2
/// 最极端场景：BufWriter 未 flush、Drop 未执行
#[test]
fn HW07_写入中途UD2崩溃_数据库仍可打开() {
    let path = tmp_db("hw07_mid");
    cleanup(&path);

    let status = run_crash_child(&path, "ud2_mid_write");
    assert!(!status.success());

    let result = triviumdb::Database::<f32>::open(&path, DIM);
    match &result {
        Ok(db) => eprintln!("  ✅ 中途崩溃恢复：{} 个节点存活", db.node_count().unwrap()),
        Err(_) if !std::path::Path::new(&path).exists() => {
            eprintln!("  ✅ 数据库文件不存在（从未 flush），预期行为");
        }
        Err(e) => panic!("数据库文件存在但打开失败: {}", e),
    }
    cleanup(&path);
}

/// 大 Payload 压力崩溃 — 每节点 ~8KB JSON，压测 WAL BufWriter 边界
/// 验证：大块 WAL 写入被中断时不会产生不完整的反序列化帧
#[test]
fn HW08_大Payload_8KB_WAL缓冲压力崩溃恢复() {
    let path = tmp_db("hw08_bigpay");
    cleanup(&path);

    let status = run_crash_child(&path, "big_payload_crash");
    assert!(!status.success(), "子进程应异常退出");
    #[cfg(all(unix, any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(4), "子进程必须实际触发非法指令信号");
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    assert_eq!(
        status.code().map(|code| code as u32),
        Some(0xC000001D),
        "子进程必须实际触发 Windows 非法指令异常"
    );

    let db = triviumdb::Database::<f32>::open(&path, DIM).expect("大 Payload 崩溃后打开失败");

    // 五条进入检查点，三条由 Normal WAL 确认；八条均须抵御本次进程崩溃。
    let mut ids = db.all_node_ids().unwrap();
    ids.sort_unstable();
    assert_eq!(
        ids,
        (1..=8).collect::<Vec<_>>(),
        "确认的大 Payload 写入必须完整恢复"
    );
    // 8000 字节跨过 WAL 缓冲边界；每条恢复记录都须保持完整正文、序号及向量。
    for id in 1..=8 {
        let seq = id - 1;
        let node = db.get(id).unwrap().expect("恢复节点必须包含完整记录");
        assert_eq!(node.vector, vec![seq as f32, 2.0, 0.0, 0.0]);
        assert_eq!(
            node.payload,
            serde_json::json!({"fat": "X".repeat(8000), "seq": seq})
        );
    }
    eprintln!(
        "  ✅ 大 Payload 崩溃恢复：{} 个节点，数据完整",
        db.node_count().unwrap()
    );
    drop(db);
    cleanup(&path);
}

/// 连续 3 次崩溃恢复 — 累积 WAL 回放压力测试
/// 模拟真实生产环境中的反复断电重启：
/// 轮次 0: 写入 3 节点 → UD2 崩溃
/// 轮次 1: WAL 回放 + 写入 3 节点 → UD2 崩溃
/// 轮次 2: WAL 回放 + 写入 3 节点 → UD2 崩溃
/// 最终验证：所有 9 个节点通过累积 WAL 回放恢复
#[test]
fn HW09_连续三次崩溃_累积WAL回放恢复() {
    let path = tmp_db("hw09_multi");
    cleanup(&path);

    for round in 0..3u32 {
        let status = run_crash_child_env(
            &path,
            "multi_crash_round",
            &[("HW_CRASH_ROUND", &round.to_string())],
        );
        assert!(!status.success(), "轮次 {} 子进程应异常退出", round);

        // 每轮崩溃后验证数据库仍可打开
        let db = triviumdb::Database::<f32>::open(&path, DIM)
            .unwrap_or_else(|e| panic!("轮次 {} 崩溃后打开失败: {}", round, e));
        eprintln!("  轮次 {}: {} 个节点存活", round, db.node_count().unwrap());
    }

    // 最终验证：3 轮各写入 3 个节点，全部 9 个已确认提交的节点必须通过 WAL 回放恢复
    let db = triviumdb::Database::<f32>::open(&path, DIM).unwrap();
    assert_eq!(
        db.node_count().unwrap(),
        9,
        "连续 3 轮崩溃-写入后，WAL 回放应恢复全部 9 个已确认提交的节点，实际恢复 {} 个",
        db.node_count().unwrap()
    );
    // 验证恢复的节点数据可读
    for &id in &db.all_node_ids().unwrap() {
        assert!(
            db.get(id).unwrap().is_some(),
            "WAL 恢复的节点 {} 应能通过 get() 获取",
            id
        );
    }
    eprintln!(
        "  ✅ 连续 3 次崩溃后最终恢复：{} 个节点（数据完整性已验证）",
        db.node_count().unwrap()
    );

    cleanup(&path);
}

/// 同一数据库跨六种进程异常累积写入，每轮八条确认记录均须保留完整内容。
#[test]
fn HW10_全异常向量交叉轰炸_同一DB连续恢复() {
    let path = tmp_db("hw10_crossfire");
    cleanup(&path);

    let modes = [
        ("ud2_after_flush", "#UD"),
        ("div_zero_after_flush", "#DE"),
        ("null_deref_after_flush", "#GP"),
        ("int3_after_flush", "#BP"),
        ("regpoison_after_flush", "RegPoison"),
        ("mfence_after_flush", "MFENCE"),
    ];

    for (i, (mode, label)) in modes.iter().enumerate() {
        let base = (i * 8).to_string();
        let status = run_crash_child_env(&path, mode, &[("HW_CRASH_BASE", &base)]);
        assert!(
            !status.success(),
            "轮次 {} ({}): 子进程应异常退出",
            i,
            label
        );

        let db = triviumdb::Database::<f32>::open(&path, DIM)
            .unwrap_or_else(|e| panic!("轮次 {} ({}) 崩溃后打开失败: {}", i, label, e));

        let expected_count = ((i + 1) * 8) as u64;
        let mut ids = db.all_node_ids().unwrap();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (1..=expected_count).collect::<Vec<_>>(),
            "轮次 {i} ({label}) 的累计写入不完整"
        );
        for id in 1..=expected_count {
            let seq = id - 1;
            let phase = if seq % 8 < 5 { "flushed" } else { "wal_only" };
            let node = db.get(id).unwrap().expect("累计恢复的节点必须存在");
            assert_eq!(node.vector, vec![seq as f32, 0.0, 0.0, 0.0]);
            assert_eq!(node.payload, serde_json::json!({"phase":phase,"seq":seq}));
        }

        eprintln!(
            "  ✅ 轮次 {} [{}]: {} 个节点恢复",
            i,
            label,
            db.node_count().unwrap()
        );
    }

    cleanup(&path);
}

#[path = "../support/cleanup.rs"]
mod cleanup_files;
