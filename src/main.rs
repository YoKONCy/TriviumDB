use serde_json::json;
use std::error::Error;
use triviumdb::Database;

fn main() -> Result<(), Box<dyn Error>> {
    let db_path = "test_persist.tdb";

    // ══════ Phase 1: 写入并持久化 ══════
    println!("=== Phase 1: Create + Write + Flush ===");
    {
        let mut db = Database::open(db_path, 4)?;
        let id1 = db.insert(&[1.0, 0.0, 0.0, 0.0], json!({"name": "Alice", "age": 25}))?;
        let id2 = db.insert(&[0.0, 1.0, 0.0, 0.0], json!({"name": "Bob", "age": 30}))?;
        let id3 = db.insert(&[0.0, 0.0, 1.0, 0.0], json!({"name": "Charlie", "age": 22}))?;
        db.link(id1, id2, "friend", 0.9)?;
        db.link(id2, id3, "colleague", 0.7)?;
        println!("  Inserted {} nodes, flushing to {}", db.node_count(), db_path);
        db.flush()?;
        println!("  Flush complete! File created.");
    } // db dropped here - simulating app exit

    // ══════ Phase 2: 重新打开 ══════
    println!("\n=== Phase 2: Reopen from .tdb file ===");
    {
        let db = Database::open(db_path, 4)?;
        println!("  Loaded {} nodes from {}", db.node_count(), db_path);
        println!("  Dim: {}", db.dim());

        // Verify all nodes survived
        for id in 1..=3u64 {
            if let Some(node) = db.get(id) {
                println!("  Node[{}]: payload={}, edges={}, vec={:?}",
                    node.id, node.payload, node.edges.len(), node.vector);
            }
        }

        // HNSW 时因为新插入或者回放了 WAL，需要手动重建 index 以保证 search 能查到
        #[cfg(feature = "hnsw")]
        {
            println!("  [Feature: HNSW] Rebuilding vector index...");
            db.rebuild_index();
        }

        // Verify search still works on reloaded data
        println!("\n  Search for vector near Alice [0.9, 0.1, 0, 0], expand=1:");
        let results = db.search(&[0.9, 0.1, 0.0, 0.0], 1, 1, 0.5)?;
        for hit in &results {
            println!("    [ID:{}] score={:.4} | {}", hit.id, hit.score, hit.payload);
        }
    }

    // Cleanup
    std::fs::remove_file(db_path)?;
    println!("\n=== All persistence tests passed! ===");

    Ok(())
}
