# Measurement fields

## Identity and configuration

`dataset_id`, `config_id`, `curve_id`, `parameters` and `threads` identify an operating point. `route` records the evaluated scoring route. `measurement_domain` records the native interface domain. The `evidence` field retains result, protocol, index and archive hashes when available.

`methods.csv` lists all measured method labels and their coverage. `configurations.csv` retains each index's construction settings. The structured snapshot also retains validation-target selections.

## Recall, throughput and latency

`recall_at_10` is a fraction in [0, 1]. `qps` is the arithmetic mean of five throughput rounds, and `qps_rounds` contains those round values. `qps_cv` is their coefficient of variation. `batch_qps` retains a separately reported batch-throughput value where present.

Latency fields use milliseconds. `p50_ms`, `p95_ms` and `p99_ms` retain native service latency measurements. The `p95_ci_*` and `p99_ci_*` columns retain query-cluster confidence intervals. SVS records carry the available throughput measurements and their latency-availability notes.

`evaluation_queries` records the number of queries in the corresponding evaluation set. Throughput and latency use their separately recorded measurement intervals. Record-specific timing notes are in the final `notes` column.

## Preparation and loading

`build_s` is the recorded native construction interval. `sm_install_s` is QuIVer's representation-installation interval. `index_preparation_s` adds these two intervals for QuIVer and retains the recorded preparation interval for the other methods.

The build boundaries follow each native implementation. QuIVer includes graph construction and SM installation. SymphonyQG, HNSWlib, VSAG and RaBitQ use their build calls. SVS includes LVQ quantization and graph construction. Lucene spans insertion, merge and commit. NGT-QG includes the build commands' internal persistence, and DiskANN includes its recorded PQ-training work.

`save_s`, `save_wrapper_s`, `load_s`, `warm_load_s`, `training_s` and `graph_insert_s` preserve separately available intervals. Each configuration's notes describe the measured boundaries.

## Memory and files

Memory and file-size columns use GiB, where 1 GiB equals 2^30 bytes.

| Field | Definition |
| --- | --- |
| `navigation_gib` | Persistent navigation components, including graph capacity, codes, factors and mappings |
| `rerank_gib` | Separate original-vector store used for final reranking |
| `total_logical_gib` | Sum of navigation and separate reranking components |
| `query_rss_gib` | Recorded resident memory for the query process at this point |
| `query_pss_gib` | Recorded proportional resident memory at this point |
| `config_resident_gib` | Configuration-level residency record |
| `index_file_gib` | Recorded index-file size |
| `external_vector_file_gib` | Separate vector-file size |
| `deployment_files_gib` | Recorded deployment-file total |

Configuration component columns retain code, graph, scalar-factor and auxiliary-navigation subtotals. The notes specify the component-accounting method, runtime overhead and residency sampling boundary. Logical components and observed RSS have separate columns throughout the export.

The evaluated QuIVer layout retains the BQ store, SM rows, adjacency capacity and mappings described in the paper. The SSD observations retain their separate steady-state process RSS and actual bytes read per query.

## Supplementary experiments

The JSON and CSV files under `results/ablations/` retain the component experiments' own score settings, exact budgets, graph identities, query populations and repetition records. The files under `results/ssd/` retain stream, cache, CPU and I/O settings. Their units and measurement intervals are recorded in each summary.

`protocols/` carries input hashes, dataset dimensions, query splits, source identities and frozen parameter records. Machine-private locations have been rewritten as relative recorded locations. The manifest retains original-file hashes and documents these export transformations.

