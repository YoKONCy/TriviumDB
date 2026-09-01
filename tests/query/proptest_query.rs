#![allow(non_snake_case, deprecated)]
//! 属性驱动测试 (Property-Based Testing) 对标 SQLite 的 Fuzzer 强度
//! 随机生成海量的畸形/极端查询语法树，检测 TQL Parser 健壮性与引擎 Panic 容忍度

use proptest::prelude::*;
use std::panic::catch_unwind;
use triviumdb::Database;

const DIM: usize = 4;

fn tmp_db(name: &str) -> String {
    let dir = std::env::temp_dir().join("triviumdb_test");
    std::fs::create_dir_all(&dir).ok();
    dir.join(format!("proptest_{}", name))
        .to_string_lossy()
        .to_string()
}

fn cleanup(path: &str) {
    for ext in &["", ".wal", ".vec", ".lock", ".flush_ok"] {
        std::fs::remove_file(format!("{}{}", path, ext)).ok();
    }
}

// TQL 语法片段生成器
prop_compose! {
    fn random_tql()(
        label in "[a-zA-Z0-9_]{0, 10}",
        cond_field in "[a-zA-Z_]{1, 5}",
        cond_op in "==|>|<|!=|>=|<=",
        cond_val in "[0-9]{1,5}|\"[a-z]{1,4}\"",
        depth in 0..5_usize
    ) -> String {
        let mut q = String::from("MATCH (a)");
        for i in 0..depth {
            let var = format!("n{}", i);
            q.push_str(&format!("-[:{}]->({})", label, var));
        }
        let target_var = if depth > 0 { format!("n{}", depth - 1) } else { "a".into() };
        q.push_str(&format!(" WHERE {}.{} {} {}", target_var, cond_field, cond_op, cond_val));
        q.push_str(" RETURN a");
        q
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    #[test]
    fn 测试_属性驱动_随机语法解析不恐慌(query in random_tql()) {
        let path = tmp_db("fuzz_query");
        cleanup(&path);

        let db = Database::<f32>::open(&path, DIM).unwrap();

        // 我们不关心是否报错 (Err)，只关心在这个乱七八糟的语法下会不会崩溃(Panic)
        let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = db.tql(&query);
        }));

        assert!(result.is_ok(), "TQL 解析器发生了致命 Panic，导致主线程崩溃！查询: {}", query);

        drop(db); // 修复: 在 Windows 平台，必须显式完全注销 Mmap 文件句柄
        cleanup(&path); // 之后再执行物理删除，防止底层读写冲突和死锁蓝屏
    }
}

// 再做一根完全乱码生成器
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]
    #[test]
    fn 测试_完全乱码_语法解析器绝不Panic(garbage in ".*") {
         let path = tmp_db("fuzz_garbage");
         let db = Database::<f32>::open(&path, DIM).unwrap();

         let result = catch_unwind(std::panic::AssertUnwindSafe(|| {
             let _ = db.tql(&garbage);
         }));

         assert!(result.is_ok(), "乱码解析诱发了 Panic: {}", garbage);
         drop(db); // 修复: Windows 显式内存解除映射
         cleanup(&path);
    }
}
