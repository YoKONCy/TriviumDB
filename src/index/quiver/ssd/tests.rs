//! 真实文件、独立向量方向、代际发布和受控I/O边界验收。
use super::*;
use crate::test_support::TestDir;
use std::sync::Arc;

const SOURCE: [u8; 32] = [0x31; 32];

#[path = "runtime_tests.rs"]
mod runtime_tests;

#[test]
fn database_routing_borrow_fits_a_fixed_open_budget_and_keeps_row_leases_independent() {
    for encoding in [
        RerankEncoding::Int8,
        RerankEncoding::CenteredInt8,
        RerankEncoding::CenteredInt4,
    ] {
        let directory = TestDir::new("quiver-shared-routing");
        let path = directory.0.join("sample.qssd");
        let runtime = runtime();
        let dimension = 3072;
        let rows = 128;
        let vectors = (0..rows)
            .map(|row| {
                let mut vector = vec![0.; dimension];
                vector[dimension - 1] = match row % 4 {
                    0 => 1.,
                    1 => -1.,
                    _ => 0.,
                };
                vector
            })
            .collect::<Vec<_>>();
        let mut index = QuIVer::new(
            dimension,
            &super::super::QuIVerConfig {
                m: 2,
                ef_construction: 16,
                alpha: 1.2,
            },
        );
        index.batch_build(
            &vectors.iter().flatten().copied().collect::<Vec<_>>(),
            &(1..=rows as u64).collect::<Vec<_>>(),
            &(0..rows).collect::<Vec<_>>(),
        );
        let metadata = SsdQuiver::publish(
            &index,
            &path,
            SOURCE,
            SsdOptions {
                encoding,
                ..Default::default()
            },
            &runtime,
            |slot, output| {
                output.copy_from_slice(&vectors[slot]);
                Ok(())
            },
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let binding = (
            metadata.hot_bytes,
            u32::from_le_bytes(bytes[104..108].try_into().unwrap()),
        );
        // 128 KiB可容纳固定校验工作区和3072维中心，完整热区超过这个宿主额度。
        let options = SsdOptions {
            memory_limit_bytes: 128 * 1024,
            ..Default::default()
        };
        assert!(metadata.hot_bytes as usize > options.memory_limit_bytes);
        assert_eq!(
            SsdQuiver::open(&path, SOURCE, options, runtime.clone())
                .err()
                .unwrap()
                .details()["resource"],
            "quiver_ssd_memory_bytes"
        );
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        let mut reader =
            SsdQuiver::open_bound(&path, SOURCE, options, runtime.clone(), binding, &index)
                .unwrap();
        assert!(reader.stats().routing_shared);
        assert!(reader.stats().reserved_hot_bytes < 8 * 1024);
        let baseline = reader.stats().reserved_bytes;
        // 查询工作区仍独立准入，扩大宿主额度后可完整重试。
        let config = QuIVerSearchConfig {
            top_k: rows,
            ef_search: rows,
            rerank_limit: Some(rows),
        };
        assert_eq!(
            reader
                .search_bound(&vectors[0], &config, &|_| true, &index)
                .unwrap_err()
                .details()["resource"],
            "quiver_ssd_memory_bytes"
        );
        assert_eq!(reader.stats().reserved_bytes, baseline);
        reader.set_database_capacity(0);
        let hits = reader
            .search_bound(&vectors[0], &config, &|_| true, &index)
            .unwrap();
        let mut expected = (1..=rows as u64)
            .map(|id| {
                let score = match (id - 1) % 4 {
                    0 => 1f32,
                    1 => -1.,
                    _ => 0.,
                };
                (id, score)
            })
            .collect::<Vec<_>>();
        expected.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(hits, expected);
        let route = reader.routing(Some(&index)).unwrap();
        let held = reader.load_record_routed(0, route).unwrap();
        let query =
            RerankQuery::new(&vectors[0], QueryPrecision::Fp32, &held.center.values).unwrap();
        reader.close();
        drop(index);
        std::fs::rename(&path, directory.0.join("moved.qssd")).unwrap();
        assert_eq!(held.score_prepared(&query).unwrap(), 1.);
        drop(held);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}

#[test]
fn shared_routing_open_still_rejects_corrupt_hot_bytes() {
    let directory = TestDir::new("quiver-shared-corruption");
    let path = directory.0.join("sample.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut bytes = std::fs::read(&path).unwrap();
    let binding = (
        metadata.hot_bytes,
        u32::from_le_bytes(bytes[104..108].try_into().unwrap()),
    );
    bytes[160 + 16] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        SsdQuiver::open_bound(
            &path,
            SOURCE,
            SsdOptions::default(),
            runtime.clone(),
            binding,
            &index
        )
        .err()
        .unwrap()
        .code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    std::fs::rename(&path, directory.0.join("rejected.qssd")).unwrap();
}

#[test]
fn compressed_codecs_batch_all_candidates_and_keep_center_leases_across_close() {
    for dimension in [1, 129, 768, 769, 3072] {
        for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
            let directory = TestDir::new("quiver-centered");
            let path = directory.0.join("sample.qssd");
            let runtime = runtime();
            let (index, vectors) = fixture(dimension);
            SsdQuiver::publish(
                &index,
                &path,
                SOURCE,
                SsdOptions {
                    encoding,
                    ..Default::default()
                },
                &runtime,
                |slot, output| {
                    output.copy_from_slice(&vectors[slot]);
                    Ok(())
                },
            )
            .unwrap();
            let mut reader =
                SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
            let query = &vectors[1];
            for precision in [
                QueryPrecision::Fp32,
                QueryPrecision::Int8,
                QueryPrecision::Int4,
            ] {
                reader
                    .configure(SsdRuntimeOptions {
                        encoding,
                        query_precision: precision,
                        cache_bytes: 0,
                        ..Default::default()
                    })
                    .unwrap();
                let before = reader.stats();
                let hits = reader.search(query, &config()).unwrap();
                assert_eq!(
                    hits.iter().map(|x| x.0).collect::<Vec<_>>(),
                    [9, 9007199254740993, 40, 77]
                );
                for (hit, expected) in hits.iter().zip([1., 1., 0., -1.]) {
                    assert!((hit.1 - expected).abs() < 1e-6);
                }
                let after = reader.stats();
                assert_eq!(after.cold_read_calls - before.cold_read_calls, 4);
                // 连续四行被完整预取并合并成一个缓冲位置读取，CPU派发能力独立于该契约。
                assert_eq!(after.physical_read_calls - before.physical_read_calls, 1);
            }
            let held = reader.row(9).unwrap();
            let prepared =
                RerankQuery::new(query, QueryPrecision::Fp32, &held.center.values).unwrap();
            reader.close();
            std::fs::rename(&path, directory.0.join("moved.qssd")).unwrap();
            assert!(reader.stats().reserved_bytes >= dimension * 4 + 512);
            assert!((held.score_prepared(&prepared).unwrap() - 1.).abs() < 1e-6);
            drop(held);
            assert_eq!(reader.stats().reserved_bytes, 0);
        }
    }
}

#[test]
fn centered_artifact_checks_shared_center_before_query_and_rejects_unknown_codecs() {
    let directory = TestDir::new("quiver-center-crc");
    let path = directory.0.join("sample.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    let metadata = SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions {
            encoding: RerankEncoding::CenteredInt4,
            ..Default::default()
        },
        &runtime,
        |slot, output| {
            output.copy_from_slice(&vectors[slot]);
            Ok(())
        },
    )
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    for offset in [
        160 + metadata.hot_bytes as usize,
        160 + metadata.hot_bytes as usize + 515,
    ] {
        let mut damaged = original.clone();
        damaged[offset] ^= 1;
        std::fs::write(&path, damaged).unwrap();
        assert!(matches!(
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()),
            Err(TriviumError::CorruptedFile(_))
        ));
    }
    std::fs::write(&path, &original).unwrap();
    let mut unknown = original.clone();
    unknown[124..128].copy_from_slice(&99u32.to_le_bytes());
    let layout_crc = crc32fast::hash(&unknown[112..148]);
    unknown[148..152].copy_from_slice(&layout_crc.to_le_bytes());
    let header_crc = crc32fast::hash(&unknown[..156]);
    unknown[156..160].copy_from_slice(&header_crc.to_le_bytes());
    std::fs::write(&path, unknown).unwrap();
    assert!(matches!(
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()),
        Err(TriviumError::CorruptedFile(_))
    ));
    std::fs::write(&path, &original).unwrap();
    let mut reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime).unwrap();
    assert_eq!(
        reader.stats().metadata.encoding,
        RerankEncoding::CenteredInt4
    );
    reader.close();
}

#[test]
fn centered_publication_checks_foreign_targets_before_calibration_reads() {
    let directory = TestDir::new("ssd_center_target");
    let path = directory.0.join("foreign.qssd");
    let original = b"fixture-foreign-content";
    std::fs::write(&path, original).unwrap();
    let (index, _) = fixture(129);
    let runtime = runtime();
    for encoding in [RerankEncoding::CenteredInt8, RerankEncoding::CenteredInt4] {
        let mut calls = 0;
        let error = SsdQuiver::publish(
            &index,
            &path,
            SOURCE,
            SsdOptions {
                encoding,
                ..Default::default()
            },
            &runtime,
            |_, _| {
                calls += 1;
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), "TDB_INVALID_INPUT");
        assert_eq!(calls, 0);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}

fn runtime() -> ResourceRuntime {
    ResourceRuntime::new(crate::ResourceLimits::default()).unwrap()
}

fn fixture(dim: usize) -> (QuIVer, Vec<Vec<f32>>) {
    // 按slot排列的四个方向及非顺序NodeId，第三个方向与查询正交。
    let mut positive = vec![0f32; dim];
    positive[dim - 1] = 1.;
    let negative = positive.iter().map(|&value| -value).collect::<Vec<_>>();
    let zero = vec![0f32; dim];
    let vectors = vec![negative, positive.clone(), zero, positive];
    let mut index = QuIVer::new(
        dim,
        &super::super::QuIVerConfig {
            m: 2,
            ef_construction: 16,
            alpha: 1.2,
        },
    );
    let flat = [1usize, 0, 3, 2]
        .into_iter()
        .flat_map(|slot| vectors[slot].clone())
        .collect::<Vec<_>>();
    index.batch_build(&flat, &[9007199254740993, 77, 9, 40], &[1, 0, 3, 2]);
    (index, vectors)
}

fn publish(
    path: &Path,
    index: &QuIVer,
    vectors: &[Vec<f32>],
    source: [u8; 32],
    runtime: &ResourceRuntime,
) -> SsdMetadata {
    SsdQuiver::publish(
        index,
        path,
        source,
        SsdOptions::default(),
        runtime,
        |slot, output| {
            output.copy_from_slice(&vectors[slot]);
            Ok(())
        },
    )
    .unwrap()
}

fn config() -> QuIVerSearchConfig {
    QuIVerSearchConfig {
        top_k: 4,
        ef_search: 4,
        rerank_limit: Some(4),
    }
}

fn expected() -> Vec<(u64, f32)> {
    vec![(9, 1.), (9007199254740993, 1.), (40, 0.), (77, -1.)]
}

#[cfg(feature = "test-hooks")]
fn inverted_fixture(dim: usize) -> (QuIVer, Vec<Vec<f32>>) {
    let (_, mut vectors) = fixture(dim);
    for vector in &mut vectors {
        for value in vector {
            *value = -*value;
        }
    }
    let mut index = QuIVer::new(
        dim,
        &super::super::QuIVerConfig {
            m: 2,
            ef_construction: 16,
            alpha: 1.2,
        },
    );
    let flat = [1usize, 0, 3, 2]
        .into_iter()
        .flat_map(|slot| vectors[slot].clone())
        .collect::<Vec<_>>();
    index.batch_build(&flat, &[9007199254740993, 77, 9, 40], &[1, 0, 3, 2]);
    (index, vectors)
}

#[cfg(feature = "test-hooks")]
fn inverted_expected() -> Vec<(u64, f32)> {
    vec![(77, 1.), (40, 0.), (9, -1.), (9007199254740993, -1.)]
}

#[test]
fn streaming_roundtrip_preserves_ids_slots_scores_and_tail_dimensions() {
    for dim in [1, 65, 128, 129, 384, 768, 769, 1536, 3072] {
        let directory = TestDir::new("ssd_roundtrip");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(dim);
        let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(metadata.dimension, dim);
        assert_eq!(metadata.rows, 4);
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        assert_eq!(reader.stats().metadata, metadata);
        assert_eq!(reader.stats().cold_read_bytes, 0);
        assert_eq!(
            reader.search(&vectors[1], &config()).unwrap(),
            expected(),
            "维度{dim}"
        );
        assert_eq!(
            reader.stats().cold_read_bytes,
            metadata.record_bytes as u64 * 4
        );
        let row = reader.row(9007199254740993).unwrap();
        let mut decoded = vec![0f32; dim];
        row.decode_into(&mut decoded).unwrap();
        for (actual, expected) in decoded.iter().zip(&vectors[1]) {
            assert!((actual - expected).abs() < 1e-7);
        }
        reader.close();
        assert!(reader.stats().closed);
        assert_eq!(
            row.score(&Int8Query::new(&vectors[1]).unwrap()).unwrap(),
            1.
        );
        assert_eq!(
            runtime.stats().reserved_workspace_bytes,
            metadata.record_bytes + 512
        );
        std::fs::rename(&path, directory.0.join("released.qssd")).unwrap();
        std::fs::remove_file(directory.0.join("released.qssd")).unwrap();
        drop(row);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().active_tasks, 0);
        assert!(matches!(
            reader.search(&vectors[1], &config()),
            Err(TriviumError::DatabaseClosed)
        ));
    }
}

#[test]
fn old_readers_pin_their_generation_across_atomic_replacement() {
    let directory = TestDir::new("ssd_generations");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(129);
    let first = publish(&path, &index, &vectors, SOURCE, &runtime);
    let old = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    let second = publish(&path, &index, &vectors, [0x32; 32], &runtime);
    assert_ne!(first.generation, second.generation);
    let current =
        SsdQuiver::open(&path, [0x32; 32], SsdOptions::default(), runtime.clone()).unwrap();
    assert_eq!(old.stats().metadata, first);
    assert_eq!(current.stats().metadata, second);
    assert_eq!(old.search(&vectors[1], &config()).unwrap(), expected());
    assert_eq!(current.search(&vectors[1], &config()).unwrap(), expected());
    assert_eq!(
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
            .err()
            .unwrap()
            .code(),
        "TDB_IMMUTABLE_ARTIFACT"
    );
    drop(old);
    drop(current);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn failed_fill_and_budget_admission_preserve_the_published_generation() {
    let directory = TestDir::new("ssd_build_failure");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(65);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let before = std::fs::read(&path).unwrap();
    let error = SsdQuiver::publish(
        &index,
        &path,
        [0x32; 32],
        SsdOptions::default(),
        &runtime,
        |_, _| {
            Err(TriviumError::InvalidInput(
                "受控来源拒绝 (Controlled source rejection)".into(),
            ))
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        error.info().write.unwrap().outcome,
        crate::error::WriteOutcome::NotCommitted
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let error = SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions {
            memory_limit_bytes: 1,
            ..Default::default()
        },
        &runtime,
        |_, _| panic!("预算拒绝后来源回调不得执行"),
    )
    .unwrap_err();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn partial_source_vectors_are_rejected_before_publication() {
    let directory = TestDir::new("ssd_partial_source");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, _) = fixture(65);
    let error = SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions::default(),
        &runtime,
        |_, vector| {
            vector[0] = 1.;
            Ok(())
        },
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_VECTOR");
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn publication_rejects_foreign_targets_and_preserves_their_full_content() {
    let directory = TestDir::new("ssd_foreign_target");
    let path = directory.0.join("database.tdb");
    let before = b"TDB facts and unrelated host content";
    std::fs::write(&path, before).unwrap();
    let runtime = runtime();
    let (index, _) = fixture(65);
    let error = SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions::default(),
        &runtime,
        |_, _| panic!("外部文件目标拒绝后不得读取来源"),
    )
    .unwrap_err();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        error.info().target,
        Some(crate::error::ErrorTarget::Field {
            field: "path".into()
        })
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 1);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn reader_rejects_non_file_paths_and_releases_task_admission() {
    let directory = TestDir::new("ssd_directory_input");
    let runtime = runtime();
    let error = SsdQuiver::open(&directory.0, SOURCE, SsdOptions::default(), runtime.clone())
        .err()
        .unwrap();
    assert_eq!(error.code(), "TDB_INVALID_INPUT");
    assert_eq!(
        error.info().target,
        Some(crate::error::ErrorTarget::Field {
            field: "path".into()
        })
    );
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 0);
    assert_eq!(runtime.stats().active_tasks, 0);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn position_reads_are_independent_across_concurrent_queries() {
    let directory = TestDir::new("ssd_concurrent");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(769);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let reader =
        Arc::new(SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap());
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let reader = reader.clone();
        let query = vectors[1].clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..12 {
                assert_eq!(reader.search(&query, &config()).unwrap(), expected());
            }
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    // 并发请求允许合并同一冷行，在完整结果断言之外核对实际读取上下界。
    let bytes = reader.stats().cold_read_bytes;
    assert!(bytes >= reader.metadata.record_bytes as u64 * 4);
    assert!(bytes <= reader.metadata.record_bytes as u64 * 4 * 4 * 12);
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(runtime.stats().active_tasks, 0);
}

#[test]
fn read_only_open_and_queries_preserve_all_artifact_bytes() {
    let directory = TestDir::new("ssd_read_only");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(384);
    publish(&path, &index, &vectors, SOURCE, &runtime);
    let before = std::fs::read(&path).unwrap();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    assert_eq!(reader.search(&vectors[1], &config()).unwrap(), expected());
    assert_eq!(
        reader
            .row(77)
            .unwrap()
            .score(&Int8Query::new(&vectors[1]).unwrap())
            .unwrap(),
        -1.
    );
    drop(reader);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn damaged_cold_rows_fail_on_access_and_preserve_error_codes() {
    let directory = TestDir::new("ssd_corrupt");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(65);
    let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = quiver_cold::HEADER_BYTES + metadata.hot_bytes as usize;
    bytes[offset + 32] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    assert_eq!(reader.stats().cold_read_calls, 0);
    assert_eq!(
        reader.row(9007199254740993).err().unwrap().code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(
        reader.search(&vectors[1], &config()).unwrap_err().code(),
        "TDB_CORRUPTED_FILE"
    );
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

const GOLDEN: &[u8] = include_bytes!("../../../../tests/fixtures/quiver_ssd_v1/unit.qssd");

fn repair_header(bytes: &mut [u8]) {
    let crc = crc32fast::hash(&bytes[..156]);
    bytes[156..160].copy_from_slice(&crc.to_le_bytes());
}

fn repair_record(record: &mut [u8]) {
    let mut crc = crc32fast::Hasher::new();
    crc.update(&record[..28]);
    crc.update(&record[32..]);
    record[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
}

#[test]
fn independent_v1_fixture_decodes_complete_metadata_and_content() {
    let directory = TestDir::new("ssd_oracle");
    let path = directory.0.join("unit.qssd");
    std::fs::write(&path, GOLDEN).unwrap();
    let runtime = runtime();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    assert_eq!(
        reader.metadata,
        SsdMetadata {
            encoding: RerankEncoding::Int8,
            source: SOURCE,
            generation: [
                0, 17, 34, 51, 68, 85, 70, 119, 136, 153, 170, 187, 204, 221, 238, 255
            ],
            dimension: 1,
            rows: 1,
            hot_bytes: 198,
            cold_bytes: 96,
            record_bytes: 96
        }
    );
    let query_config = QuIVerSearchConfig {
        top_k: 1,
        ef_search: 4,
        rerank_limit: None,
    };
    assert_eq!(reader.search(&[1.], &query_config).unwrap(), [(7, 1.)]);
    assert_eq!(reader.search(&[-1.], &query_config).unwrap(), [(7, -1.)]);
    let mut vector = [0f32; 1];
    reader.row(7).unwrap().decode_into(&mut vector).unwrap();
    assert_eq!(vector, [1.]);
    assert_eq!(reader.row(8).err().unwrap().code(), "TDB_NODE_NOT_FOUND");
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn independent_v2_centered_fixtures_decode_and_check_every_byte() {
    // 在冻结v1热图和身份上手工填写v2字段，单位向量减中心0.25后残差为常量0.75。
    for (codec, encoding) in [
        (2u32, RerankEncoding::CenteredInt8),
        (3, RerankEncoding::CenteredInt4),
    ] {
        let directory = TestDir::new("ssd_v2_oracle");
        let path = directory.0.join("unit.qssd");
        let mut bytes = GOLDEN[..358].to_vec();
        bytes.extend_from_slice(&0.25f32.to_le_bytes());
        let mut record = GOLDEN[358..].to_vec();
        record[37..41].copy_from_slice(&0.75f32.to_le_bytes());
        let row_crc = crc32fast::hash(&record[32..45]);
        record[45..49].copy_from_slice(&row_crc.to_le_bytes());
        let mut checksum = crc32fast::Hasher::new();
        checksum.update(&record[..28]);
        checksum.update(&record[32..]);
        record[28..32].copy_from_slice(&checksum.finalize().to_le_bytes());
        bytes.extend_from_slice(&record);
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[80..88].copy_from_slice(&362u64.to_le_bytes());
        bytes[108..112].copy_from_slice(&crc32fast::hash(&0.25f32.to_le_bytes()).to_le_bytes());
        bytes[124..128].copy_from_slice(&codec.to_le_bytes());
        bytes[152..156].copy_from_slice(&4u32.to_le_bytes());
        let layout_crc = crc32fast::hash(&bytes[112..148]);
        bytes[148..152].copy_from_slice(&layout_crc.to_le_bytes());
        let header_crc = crc32fast::hash(&bytes[..156]);
        bytes[156..160].copy_from_slice(&header_crc.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        let runtime = runtime();
        let mut reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        assert_eq!(reader.stats().metadata.encoding, encoding);
        assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
        let mut decoded = [0.];
        reader.row(7).unwrap().decode_into(&mut decoded).unwrap();
        assert_eq!(decoded, [1.]);
        reader.close();
        for offset in 0..bytes.len() {
            std::fs::write(&path, &bytes[..offset]).unwrap();
            assert!(
                SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).is_err()
            );
            let mut damaged = bytes.clone();
            damaged[offset] ^= 1;
            std::fs::write(&path, damaged).unwrap();
            if offset < 362 {
                assert!(
                    SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).is_err(),
                    "头部、热图或中心损坏偏移{offset}"
                );
            } else {
                let reader =
                    SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
                assert_eq!(reader.row(7).err().unwrap().code(), "TDB_CORRUPTED_FILE");
            }
        }
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}

#[test]
fn every_artifact_byte_is_checked_at_open_or_first_access() {
    let directory = TestDir::new("ssd_mutations");
    let path = directory.0.join("unit.qssd");
    let runtime = runtime();
    for index in 0..GOLDEN.len() {
        let mut bytes = GOLDEN.to_vec();
        bytes[index] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        if index < 358 {
            let error = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
                .err()
                .unwrap();
            assert_eq!(error.code(), "TDB_CORRUPTED_FILE", "字节{index}");
        } else {
            let reader =
                SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
            assert_eq!(
                reader.row(7).err().unwrap().code(),
                "TDB_CORRUPTED_FILE",
                "字节{index}"
            );
        }
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0, "字节{index}");
    }
}

#[test]
fn every_truncation_and_extra_byte_is_rejected_before_hot_allocation() {
    let directory = TestDir::new("ssd_truncations");
    let path = directory.0.join("unit.qssd");
    let runtime = runtime();
    for length in 0..GOLDEN.len() {
        std::fs::write(&path, &GOLDEN[..length]).unwrap();
        assert_eq!(
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
                .err()
                .unwrap()
                .code(),
            "TDB_CORRUPTED_FILE",
            "长度{length}"
        );
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
    let mut extra = GOLDEN.to_vec();
    extra.push(0);
    std::fs::write(&path, extra).unwrap();
    assert_eq!(
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime)
            .err()
            .unwrap()
            .code(),
        "TDB_CORRUPTED_FILE"
    );
}

#[test]
fn forged_valid_checksums_do_not_bypass_geometry_or_row_identity() {
    let directory = TestDir::new("ssd_forged");
    let path = directory.0.join("unit.qssd");
    let runtime = runtime();
    for (offset, value) in [
        (64, 0u64),
        (72, u64::MAX),
        (80, u64::MAX),
        (88, 0),
        (88, u64::MAX),
        (96, u64::MAX),
    ] {
        let mut bytes = GOLDEN.to_vec();
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        repair_header(&mut bytes);
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
                .err()
                .unwrap()
                .code(),
            "TDB_CORRUPTED_FILE",
            "偏移{offset} 值{value}"
        );
    }
    for (offset, value) in [(0, 8u64), (8, 4), (16, 1)] {
        let mut bytes = GOLDEN.to_vec();
        bytes[358 + offset..366 + offset].copy_from_slice(&value.to_le_bytes());
        repair_record(&mut bytes[358..]);
        std::fs::write(&path, bytes).unwrap();
        let reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        assert_eq!(reader.row(7).err().unwrap().code(), "TDB_CORRUPTED_FILE");
    }
    let mut bytes = GOLDEN.to_vec();
    bytes[172..176].copy_from_slice(&u32::MAX.to_le_bytes());
    let hot_crc = crc32fast::hash(&bytes[160..358]);
    bytes[104..108].copy_from_slice(&hot_crc.to_le_bytes());
    repair_header(&mut bytes);
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
            .err()
            .unwrap()
            .code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn malformed_hot_tombstone_is_rejected_even_after_crc_recalculation() {
    let directory = TestDir::new("ssd_bad_tombstone");
    let path = directory.0.join("unit.qssd");
    let mut bytes = GOLDEN.to_vec();
    // 独立单节点QUIV布局中，墓碑位于热区104字节处。
    bytes[160 + 104] = 2;
    let hot_crc = crc32fast::hash(&bytes[160..358]);
    bytes[104..108].copy_from_slice(&hot_crc.to_le_bytes());
    repair_header(&mut bytes);
    std::fs::write(&path, bytes).unwrap();
    let runtime = runtime();
    assert_eq!(
        SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone())
            .err()
            .unwrap()
            .code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn complete_row_swaps_are_rejected_even_with_valid_row_checksums() {
    let directory = TestDir::new("ssd_row_swap");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (index, vectors) = fixture(65);
    let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
    let mut bytes = std::fs::read(&path).unwrap();
    let offset = 160 + metadata.hot_bytes as usize;
    let size = metadata.record_bytes;
    let first = bytes[offset..offset + size].to_vec();
    let second = bytes[offset + size..offset + size * 2].to_vec();
    bytes[offset..offset + size].copy_from_slice(&second);
    bytes[offset + size..offset + size * 2].copy_from_slice(&first);
    std::fs::write(&path, bytes).unwrap();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime).unwrap();
    assert_eq!(
        reader.row(9007199254740993).err().unwrap().code(),
        "TDB_CORRUPTED_FILE"
    );
    assert_eq!(reader.row(77).err().unwrap().code(), "TDB_CORRUPTED_FILE");
}

#[test]
fn truncating_an_open_file_returns_structured_short_read() {
    let directory = TestDir::new("ssd_live_short_read");
    let path = directory.0.join("unit.qssd");
    std::fs::write(&path, GOLDEN).unwrap();
    let runtime = runtime();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(358)
        .unwrap();
    assert_eq!(reader.row(7).err().unwrap().code(), "TDB_CORRUPTED_FILE");
    assert_eq!(
        reader.search(&[1.], &config()).unwrap_err().code(),
        "TDB_CORRUPTED_FILE"
    );
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn runtime_budget_rejection_releases_workspaces_and_allows_retry() {
    let directory = TestDir::new("ssd_quota");
    let path = directory.0.join("unit.qssd");
    std::fs::write(&path, GOLDEN).unwrap();
    let runtime = ResourceRuntime::new(crate::ResourceLimits {
        memory_limit: 1024 * 1024,
        ..Default::default()
    })
    .unwrap();
    let error = SsdQuiver::open(
        &path,
        SOURCE,
        SsdOptions {
            memory_limit_bytes: 1,
            ..Default::default()
        },
        runtime.clone(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code(), "QueueFull");
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    let hot = runtime.stats().reserved_workspace_bytes;
    let available = runtime.stats().available_foreground_bytes.unwrap();
    let blocker = runtime.reserve_workspace(available - 1).unwrap();
    assert_eq!(
        reader.search(&[1.], &config()).unwrap_err().code(),
        "QueueFull"
    );
    assert_eq!(reader.row(7).err().unwrap().code(), "QueueFull");
    drop(blocker);
    assert_eq!(runtime.stats().reserved_workspace_bytes, hot);
    assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn reader_budget_counts_all_retained_row_leases_through_close() {
    let directory = TestDir::new("ssd_row_quota");
    let path = directory.0.join("unit.qssd");
    std::fs::write(&path, GOLDEN).unwrap();
    let runtime = runtime();
    let limit = 80 * 1024;
    let mut reader = SsdQuiver::open(
        &path,
        SOURCE,
        SsdOptions {
            memory_limit_bytes: limit,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    let hot = reader.stats().reserved_bytes;
    // 每行额度含记录缓冲与512字节缓存/租约目录预算。
    let charge = reader.metadata.record_bytes + 512;
    // 单行读取另需4 KiB批次目录和一条传输副本，持有行租约仅保留charge。
    let count = (limit - hot - 4096 - reader.metadata.record_bytes) / charge;
    assert!(count > 0, "样例额度必须能够持有多条记录");
    let mut rows = (0..count)
        .map(|_| reader.row(7).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        reader.row(7).err().unwrap().details()["resource"],
        "quiver_ssd_memory_bytes"
    );
    assert_eq!(reader.stats().reserved_bytes, hot + count * charge);
    drop(rows.pop().unwrap());
    let replacement = reader.row(7).unwrap();
    let mut decoded = [0f32];
    replacement.decode_into(&mut decoded).unwrap();
    assert_eq!(decoded, [1.]);
    reader.close();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(reader.stats().reserved_bytes, count * charge);
    drop(rows);
    drop(replacement);
    assert_eq!(reader.stats().reserved_bytes, 0);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
}

#[test]
fn staged_loading_admission_checks_decode_growth_and_releases_failed_reservations() {
    let directory = TestDir::new("ssd_loading_quota");
    let path = directory.0.join("unit.qssd");
    std::fs::write(&path, GOLDEN).unwrap();
    let runtime = runtime();
    // 独立样例热区198字节、单节点无上层，读取准入65734字节，解码准入66386字节。
    let error = SsdQuiver::open(
        &path,
        SOURCE,
        SsdOptions {
            memory_limit_bytes: 66000,
            ..Default::default()
        },
        runtime.clone(),
    )
    .err()
    .unwrap();
    assert_eq!(error.details()["resource"], "quiver_ssd_memory_bytes");
    assert_eq!(error.details()["used"], 65734);
    assert_eq!(error.details()["requested"], 652);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    let reader = SsdQuiver::open(
        &path,
        SOURCE,
        SsdOptions {
            memory_limit_bytes: 67000,
            ..Default::default()
        },
        runtime.clone(),
    )
    .unwrap();
    let mut vector = [0f32];
    reader.row(7).unwrap().decode_into(&mut vector).unwrap();
    assert_eq!(vector, [1.]);
    drop(reader);
    assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    assert_eq!(std::fs::read(&path).unwrap(), GOLDEN);
}

#[test]
fn tombstones_never_read_a_reused_slot_or_enter_results() {
    let directory = TestDir::new("ssd_tombstone");
    let path = directory.0.join("index.qssd");
    let runtime = runtime();
    let (mut index, vectors) = fixture(129);
    index.soft_delete(9007199254740993);
    SsdQuiver::publish(
        &index,
        &path,
        SOURCE,
        SsdOptions::default(),
        &runtime,
        |slot, output| {
            assert_ne!(slot, 1, "墓碑slot已经复用，来源回调必须跳过该slot");
            output.copy_from_slice(&vectors[slot]);
            Ok(())
        },
    )
    .unwrap();
    let reader = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
    assert_eq!(
        reader.search(&vectors[1], &config()).unwrap(),
        [(9, 1.), (40, 0.), (77, -1.)]
    );
    assert_eq!(
        reader.row(9007199254740993).err().unwrap().code(),
        "TDB_NODE_NOT_FOUND"
    );
    assert_eq!(
        reader.stats().cold_read_bytes,
        reader.metadata.record_bytes as u64 * 3
    );
}

#[cfg(feature = "test-hooks")]
mod hooks {
    use super::*;
    use crate::test_hooks::{ConcurrencyPoint, IoPoint};

    #[test]
    fn cancelled_shared_open_releases_fixed_workspace_and_can_retry() {
        let directory = TestDir::new("ssd-cancel-shared-open");
        let path = directory.0.join("sample.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(129);
        let index = Arc::new(index);
        let metadata = publish(&path, &index, &vectors, SOURCE, &runtime);
        let bytes = std::fs::read(&path).unwrap();
        let binding = (
            metadata.hot_bytes,
            u32::from_le_bytes(bytes[104..108].try_into().unwrap()),
        );
        let context =
            crate::request::RequestContext::new("ssd-shared-open", Default::default(), false)
                .unwrap();
        let gate = crate::test_hooks::pause_at_path(
            ConcurrencyPoint::QuiverColdAfterHotRead,
            path.to_str().unwrap(),
        );
        let worker_path = path.clone();
        let worker_runtime = runtime.clone();
        let worker_index = index.clone();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            worker_context.run(|| {
                SsdQuiver::open_bound(
                    &worker_path,
                    SOURCE,
                    SsdOptions::default(),
                    worker_runtime,
                    binding,
                    &worker_index,
                )
            })
        });
        gate.wait_until_arrived();
        assert!(runtime.stats().reserved_workspace_bytes > 0);
        context.cancel();
        drop(gate);
        assert_eq!(
            worker.join().unwrap().err().unwrap().code(),
            "TDB_QUERY_CANCELLED"
        );
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().active_tasks, 0);
        let moved = directory.0.join("released.qssd");
        std::fs::rename(&path, &moved).unwrap();
        let mut reader = SsdQuiver::open_bound(
            &moved,
            SOURCE,
            SsdOptions::default(),
            runtime.clone(),
            binding,
            &index,
        )
        .unwrap();
        assert_eq!(
            reader
                .search_bound(&vectors[1], &config(), &|_| true, &index)
                .unwrap(),
            expected()
        );
        reader.close();
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }

    #[test]
    fn cancelled_open_releases_the_loaded_hot_buffer_and_file() {
        let directory = TestDir::new("ssd_cancel_open");
        let path = directory.0.join("unit.qssd");
        std::fs::write(&path, GOLDEN).unwrap();
        let runtime = runtime();
        let context = crate::request::RequestContext::new(
            "ssd-open",
            crate::query::tql_executor::QueryControl::default(),
            false,
        )
        .unwrap();
        let gate = crate::test_hooks::pause_at_path(
            ConcurrencyPoint::QuiverColdAfterHotRead,
            path.to_str().unwrap(),
        );
        let worker_path = path.clone();
        let worker_runtime = runtime.clone();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            worker_context.run(|| {
                SsdQuiver::open(&worker_path, SOURCE, SsdOptions::default(), worker_runtime)
            })
        });
        gate.wait_until_arrived();
        assert!(runtime.stats().reserved_workspace_bytes > 0);
        context.cancel();
        drop(gate);
        assert_eq!(
            worker.join().unwrap().err().unwrap().code(),
            "TDB_QUERY_CANCELLED"
        );
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(runtime.stats().active_tasks, 0);
        std::fs::rename(&path, directory.0.join("released.qssd")).unwrap();
        std::fs::remove_file(directory.0.join("released.qssd")).unwrap();
    }

    #[test]
    fn concurrent_queries_share_the_reader_budget_and_rejected_work_can_retry() {
        let directory = TestDir::new("ssd_query_quota");
        let path = directory.0.join("unit.qssd");
        std::fs::write(&path, GOLDEN).unwrap();
        let runtime = runtime();
        let reader = Arc::new(
            SsdQuiver::open(
                &path,
                SOURCE,
                SsdOptions {
                    memory_limit_bytes: 100 * 1024,
                    ..Default::default()
                },
                runtime.clone(),
            )
            .unwrap(),
        );
        let gate = crate::test_hooks::pause_at_path(
            ConcurrencyPoint::QuiverColdBeforeRead,
            path.to_str().unwrap(),
        );
        let worker_reader = reader.clone();
        let worker = std::thread::spawn(move || worker_reader.search(&[1.], &config()));
        gate.wait_until_arrived();
        let error = reader.search(&[1.], &config()).unwrap_err();
        assert_eq!(error.details()["resource"], "quiver_ssd_memory_bytes");
        drop(gate);
        assert_eq!(worker.join().unwrap().unwrap(), [(7, 1.)]);
        assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
        drop(reader);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }

    #[test]
    fn read_io_errors_keep_their_category_and_release_query_workspaces() {
        let directory = TestDir::new("ssd_read_failure");
        let path = directory.0.join("unit.qssd");
        std::fs::write(&path, GOLDEN).unwrap();
        let runtime = runtime();
        let reader =
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        let hot = reader.stats().reserved_bytes;
        let fault = crate::test_hooks::fail_io_at(IoPoint::QuiverColdRead);
        assert_eq!(
            reader.search(&[1.], &config()).unwrap_err().code(),
            "TDB_IO"
        );
        drop(fault);
        assert_eq!(reader.stats().reserved_bytes, hot);
        assert_eq!(runtime.stats().active_tasks, 0);
        assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
    }

    #[test]
    fn overlapping_publishers_produce_complete_generations_and_existing_readers_keep_their_files() {
        let directory = TestDir::new("ssd_publish_order");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index_a, vectors_a) = fixture(65);
        publish(&path, &index_a, &vectors_a, SOURCE, &runtime);
        let old = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        let (arrived_tx, arrived_rx) = std::sync::mpsc::channel();
        let (continue_tx, continue_rx) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        let worker_runtime = runtime.clone();
        let slow = std::thread::spawn(move || {
            let mut first = true;
            SsdQuiver::publish(
                &index_a,
                &worker_path,
                [0x32; 32],
                SsdOptions::default(),
                &worker_runtime,
                |slot, output| {
                    if first {
                        first = false;
                        arrived_tx.send(()).unwrap();
                        continue_rx.recv().unwrap();
                    }
                    output.copy_from_slice(&vectors_a[slot]);
                    Ok(())
                },
            )
            .unwrap()
        });
        arrived_rx.recv().unwrap();
        let (index_b, vectors_b) = inverted_fixture(65);
        let fast_metadata = publish(&path, &index_b, &vectors_b, [0x33; 32], &runtime);
        let fast =
            SsdQuiver::open(&path, [0x33; 32], SsdOptions::default(), runtime.clone()).unwrap();
        continue_tx.send(()).unwrap();
        let slow_metadata = slow.join().unwrap();
        let current =
            SsdQuiver::open(&path, [0x32; 32], SsdOptions::default(), runtime.clone()).unwrap();
        let mut query = vec![0.; 65];
        query[64] = 1.;
        assert_eq!(current.stats().metadata, slow_metadata);
        assert_eq!(fast.stats().metadata, fast_metadata);
        assert_eq!(old.search(&query, &config()).unwrap(), expected());
        assert_eq!(fast.search(&query, &config()).unwrap(), inverted_expected());
        assert_eq!(current.search(&query, &config()).unwrap(), expected());
        drop(old);
        drop(fast);
        drop(current);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
    }

    struct KillChild(std::process::Child);
    impl Drop for KillChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "由提交点强杀父用例显式运行，要求独立子进程环境"]
    fn ssd_power_child() {
        use std::io::Write;
        let path = std::path::PathBuf::from(
            std::env::var_os("TDB_SSD_POWER_PATH").expect("缺少子进程目标路径"),
        );
        let after = std::env::var("TDB_SSD_POWER_STAGE").expect("缺少子进程阶段") == "after";
        let point = if after {
            ConcurrencyPoint::QuiverColdAfterPublish
        } else {
            ConcurrencyPoint::QuiverColdBeforePublish
        };
        let gate = crate::test_hooks::pause_at_path(point, path.to_str().unwrap());
        let worker = std::thread::spawn(move || {
            let (index, vectors) = inverted_fixture(65);
            publish(&path, &index, &vectors, [0x32; 32], &runtime());
        });
        gate.wait_until_arrived();
        println!("SSD_POWER_READY");
        std::io::stdout().flush().unwrap();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).unwrap();
        drop(gate);
        worker.join().unwrap();
        panic!("父进程应在提交点强杀子进程");
    }

    #[test]
    fn killed_publishers_leave_the_old_or_complete_new_generation_and_support_republication() {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        for after in [false, true] {
            let directory = TestDir::new("ssd_power");
            let path = directory.0.join("index.qssd");
            let runtime = runtime();
            let (index, vectors) = fixture(65);
            publish(&path, &index, &vectors, SOURCE, &runtime);
            let original = std::fs::read(&path).unwrap();
            let mut child = KillChild(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "index::quiver::ssd::tests::hooks::ssd_power_child",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("TDB_SSD_POWER_PATH", &path)
                    .env(
                        "TDB_SSD_POWER_STAGE",
                        if after { "after" } else { "before" },
                    )
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap(),
            );
            let stdout = child.0.stdout.take().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            let output = std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    let line = line.unwrap();
                    if line.contains("SSD_POWER_READY") {
                        tx.send(()).unwrap();
                        return;
                    }
                }
            });
            // 超时仅约束子进程失联清理，完成同步由明确的提交点通知提供。
            rx.recv_timeout(std::time::Duration::from_secs(60))
                .expect("子进程未到达指定提交点");
            child.0.kill().unwrap();
            assert!(!child.0.wait().unwrap().success());
            output.join().unwrap();
            let source = if after { [0x32; 32] } else { SOURCE };
            let reader =
                SsdQuiver::open(&path, source, SsdOptions::default(), runtime.clone()).unwrap();
            assert_eq!(
                reader.search(&vectors[1], &config()).unwrap(),
                if after {
                    inverted_expected()
                } else {
                    expected()
                }
            );
            if !after {
                assert_eq!(std::fs::read(&path).unwrap(), original);
            }
            assert_eq!(
                SsdQuiver::collect_temporary(&path).unwrap(),
                usize::from(!after)
            );
            assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
            let next = publish(&path, &index, &vectors, [0x33; 32], &runtime);
            let reopened =
                SsdQuiver::open(&path, [0x33; 32], SsdOptions::default(), runtime.clone()).unwrap();
            assert_eq!(reopened.stats().metadata, next);
            assert_eq!(reopened.search(&vectors[1], &config()).unwrap(), expected());
            drop(reader);
            drop(reopened);
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
    }

    #[test]
    fn failed_publication_steps_preserve_old_bytes_and_retry_successfully() {
        for point in [
            IoPoint::QuiverColdWrite,
            IoPoint::QuiverColdSync,
            IoPoint::QuiverColdPublish,
        ] {
            let directory = TestDir::new("ssd_io_failure");
            let path = directory.0.join("index.qssd");
            let runtime = runtime();
            let (index, vectors) = fixture(65);
            let original = publish(&path, &index, &vectors, SOURCE, &runtime);
            let before = std::fs::read(&path).unwrap();
            let fault = crate::test_hooks::fail_io_at(point);
            let error = SsdQuiver::publish(
                &index,
                &path,
                [0x32; 32],
                SsdOptions::default(),
                &runtime,
                |slot, output| {
                    output.copy_from_slice(&vectors[slot]);
                    Ok(())
                },
            )
            .unwrap_err();
            drop(fault);
            assert_eq!(error.code(), "TDB_IO");
            assert_eq!(
                error.info().write.unwrap().outcome,
                crate::error::WriteOutcome::NotCommitted
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let old =
                SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
            assert_eq!(old.stats().metadata, original);
            assert_eq!(old.search(&vectors[1], &config()).unwrap(), expected());
            let updated = publish(&path, &index, &vectors, [0x32; 32], &runtime);
            let new =
                SsdQuiver::open(&path, [0x32; 32], SsdOptions::default(), runtime.clone()).unwrap();
            assert_eq!(new.stats().metadata, updated);
            assert_eq!(new.search(&vectors[1], &config()).unwrap(), expected());
            assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
            drop(old);
            drop(new);
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
    }

    #[test]
    fn directory_sync_failure_reports_unknown_and_leaves_a_readable_new_generation() {
        let directory = TestDir::new("ssd_directory_sync");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(65);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let old = SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap();
        let fault = crate::test_hooks::fail_io_at(IoPoint::QuiverColdDirectorySync);
        let error = SsdQuiver::publish(
            &index,
            &path,
            [0x32; 32],
            SsdOptions::default(),
            &runtime,
            |slot, output| {
                output.copy_from_slice(&vectors[slot]);
                Ok(())
            },
        )
        .unwrap_err();
        drop(fault);
        assert_eq!(
            error.info().write.unwrap().outcome,
            crate::error::WriteOutcome::Unknown
        );
        let current =
            SsdQuiver::open(&path, [0x32; 32], SsdOptions::default(), runtime.clone()).unwrap();
        assert_eq!(old.search(&vectors[1], &config()).unwrap(), expected());
        assert_eq!(current.search(&vectors[1], &config()).unwrap(), expected());
        drop(old);
        drop(current);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }

    #[test]
    fn cancellation_before_and_after_read_preserves_codes_and_releases_resources() {
        for point in [
            ConcurrencyPoint::QuiverColdBeforeRead,
            ConcurrencyPoint::QuiverColdAfterRead,
        ] {
            let directory = TestDir::new("ssd_cancel");
            let path = directory.0.join("unit.qssd");
            std::fs::write(&path, GOLDEN).unwrap();
            let runtime = runtime();
            let reader = Arc::new(
                SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap(),
            );
            let before = runtime.stats().reserved_workspace_bytes;
            let context = crate::request::RequestContext::new(
                "ssd-cancel",
                crate::query::tql_executor::QueryControl::default(),
                false,
            )
            .unwrap();
            let gate = crate::test_hooks::pause_at_path(point, path.to_str().unwrap());
            let worker_reader = reader.clone();
            let worker_context = context.clone();
            let worker = std::thread::spawn(move || {
                worker_context.run(|| worker_reader.search(&[1.], &config()))
            });
            gate.wait_until_arrived();
            context.cancel();
            drop(gate);
            assert_eq!(
                worker.join().unwrap().unwrap_err().code(),
                "TDB_QUERY_CANCELLED"
            );
            assert_eq!(runtime.stats().reserved_workspace_bytes, before);
            assert_eq!(runtime.stats().active_tasks, 0);
            assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
            drop(reader);
            assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
        }
    }

    #[test]
    fn queued_work_uses_current_task_admission_and_can_retry() {
        let directory = TestDir::new("ssd_task_quota");
        let path = directory.0.join("unit.qssd");
        std::fs::write(&path, GOLDEN).unwrap();
        let runtime = ResourceRuntime::new(crate::ResourceLimits {
            max_active_tasks: 1,
            ..Default::default()
        })
        .unwrap();
        let reader = Arc::new(
            SsdQuiver::open(&path, SOURCE, SsdOptions::default(), runtime.clone()).unwrap(),
        );
        let hot = runtime.stats().reserved_workspace_bytes;
        let gate = crate::test_hooks::pause_at_path(
            ConcurrencyPoint::QuiverColdBeforeRead,
            path.to_str().unwrap(),
        );
        let worker_reader = reader.clone();
        let worker = std::thread::spawn(move || worker_reader.search(&[1.], &config()));
        gate.wait_until_arrived();
        assert_eq!(
            reader.search(&[1.], &config()).unwrap_err().code(),
            "QueueFull"
        );
        drop(gate);
        assert_eq!(worker.join().unwrap().unwrap(), [(7, 1.)]);
        assert_eq!(runtime.stats().reserved_workspace_bytes, hot);
        assert_eq!(reader.search(&[1.], &config()).unwrap(), [(7, 1.)]);
        drop(reader);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }

    #[test]
    fn cancelled_prepared_publication_keeps_the_old_generation() {
        let directory = TestDir::new("ssd_cancel_publish");
        let path = directory.0.join("index.qssd");
        let runtime = runtime();
        let (index, vectors) = fixture(65);
        publish(&path, &index, &vectors, SOURCE, &runtime);
        let before = std::fs::read(&path).unwrap();
        let context = crate::request::RequestContext::new(
            "ssd-publish",
            crate::query::tql_executor::QueryControl::default(),
            true,
        )
        .unwrap();
        let gate = crate::test_hooks::pause_at_path(
            ConcurrencyPoint::QuiverColdBeforePublish,
            path.to_str().unwrap(),
        );
        let worker_path = path.clone();
        let worker_runtime = runtime.clone();
        let worker_context = context.clone();
        let worker = std::thread::spawn(move || {
            worker_context.run(|| {
                SsdQuiver::publish(
                    &index,
                    &worker_path,
                    [0x32; 32],
                    SsdOptions::default(),
                    &worker_runtime,
                    |slot, output| {
                        output.copy_from_slice(&vectors[slot]);
                        Ok(())
                    },
                )
            })
        });
        gate.wait_until_arrived();
        context.cancel();
        drop(gate);
        let error = worker.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "TDB_QUERY_CANCELLED");
        assert_eq!(
            error.info().write.unwrap().outcome,
            crate::error::WriteOutcome::NotCommitted
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read_dir(&directory.0).unwrap().count(), 2);
        assert_eq!(runtime.stats().reserved_workspace_bytes, 0);
    }
}
