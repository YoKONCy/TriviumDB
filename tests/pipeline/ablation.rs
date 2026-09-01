use triviumdb::database::{Config, Database};

#[test]
fn test_fatigue_and_specificity_ablation() {
    let temp_dir = std::env::temp_dir().join(format!("triviumdb_ablation_{}", std::process::id()));
    if temp_dir.exists() {
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
    let db_path = temp_dir.join("fatigue.tdb");

    let config = Config {
        dim: 2,
        ..Default::default()
    };

    let mut db = Database::open_with_config(db_path.to_str().unwrap(), config).unwrap();

    let search_config = triviumdb::database::SearchConfig {
        expand_depth: 3,
        top_k: 20,
        enable_advanced_pipeline: true,
        enable_dpp: false,
        enable_refractory_fatigue: true, // 必须开启防死循环机制
        teleport_alpha: 0.1,             // 允许能量有效传播
        ..Default::default()
    };

    // 我们构建一个典型的“能量坍缩”场景：
    // 黑洞节点 blackhole_id 拥有大量入度
    let blackhole_id = db
        .insert(&[1.0, 0.0], serde_json::json!({"name": "blackhole"}))
        .unwrap();

    // 一个查询入口/辐射源 src_id
    let src_id = db
        .insert(&[0.8, 0.6], serde_json::json!({"name": "source"}))
        .unwrap();

    // 一个普通亚节点 leaf_id，与 src_id 直连，但平时可能被黑洞节点夺走注意
    let leaf_id = db
        .insert(&[0.6, 0.8], serde_json::json!({"name": "leaf"}))
        .unwrap();

    // 注入大量噪声来模拟万级记忆库中的全局噪音，多数都连向黑洞
    for i in 0..100 {
        let dummy = db
            .insert(
                &[0.0, 1.0],
                serde_json::json!({"name": format!("dummy_{}", i)}),
            )
            .unwrap();
        db.link(dummy, blackhole_id, "link", 1.0).unwrap();
    }

    // source 连接到 blackhole 和 leaf，初始权重平等
    db.link(src_id, blackhole_id, "link", 1.0).unwrap();
    db.link(src_id, leaf_id, "link", 1.0).unwrap();

    // =============== 第一轮：边特异性的检验 ===============
    // 进行一次基于起始点的爆搜（查询向量非常靠近 source）
    let hits1 = db.search_advanced(&[0.8, 0.6], &search_config).unwrap();

    let bh_score1 = hits1
        .iter()
        .find(|h| h.id == blackhole_id)
        .map(|h| h.score)
        .unwrap_or(0.0);
    let lf_score1 = hits1
        .iter()
        .find(|h| h.id == leaf_id)
        .map(|h| h.score)
        .unwrap_or(0.0);

    println!("=== Round 1 (Fresh Edge Specificity) ===");
    println!("Blackhole Score:  {}", bh_score1);
    println!("Leaf Score:       {}", lf_score1);

    // 尽管 blackhole 入度超过了 100，利用 powf(0.55) 的非线性衰减，
    // 其边特异性会比原本的 1.0/log10 大幅下降。这导致 leaf_id 有喘息的机会可以与之抗衡。

    // =============== 第二轮：疲劳不应期的检验 ===============
    // 在第一轮结束后，搜出来的 Top 15 肯定包含了 blackhole_id 和 src_id。它们被打上了“物理疲劳标记”。
    // 这意味着如果紧接着再次进行相同的搜索，向它们传导的能量将衰减 85%（fatigue_discount=0.15）。

    let hits2 = db.search_advanced(&[0.8, 0.6], &search_config).unwrap();

    let bh_score2 = hits2
        .iter()
        .find(|h| h.id == blackhole_id)
        .map(|h| h.score)
        .unwrap_or(0.0);
    let lf_score2 = hits2
        .iter()
        .find(|h| h.id == leaf_id)
        .map(|h| h.score)
        .unwrap_or(0.0);

    println!("=== Round 2 (Fatigue Engaged) ===");
    println!("Blackhole Score:  {}", bh_score2);
    println!("Leaf Score:       {}", lf_score2);

    // 我们断言，被触发降频保护的不应期机制，必然使黑洞的能量断崖式下跌
    assert!(
        bh_score2 < bh_score1 * 0.5,
        "Fatigue didn't drastically decay the blackhole score!"
    );

    // =============== 第三轮：不应期的解除检验 ===============
    // 因为第二轮命中后疲劳状态被“消耗消费”掉了，因此在第三轮，它应该恢复正常的原生效能。

    let hits3 = db.search_advanced(&[0.8, 0.6], &search_config).unwrap();
    let bh_score3 = hits3
        .iter()
        .find(|h| h.id == blackhole_id)
        .map(|h| h.score)
        .unwrap_or(0.0);

    println!("=== Round 3 (Fatigue Restored) ===");
    println!("Blackhole Score:  {}", bh_score3);

    // 不应期被消费后，黑洞节点的能量应回升至高于疲劳抑制时的水平
    assert!(
        bh_score3 > bh_score2,
        "不应期解除后黑洞得分应回升！Round2(疲劳抑制)={}, Round3(恢复)={}",
        bh_score2,
        bh_score3
    );

    // 清理
    let _ = std::fs::remove_dir_all(&temp_dir);
}
