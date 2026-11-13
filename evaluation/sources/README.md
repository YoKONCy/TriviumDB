# Recorded experiment sources

| Directory | Recorded program |
| --- | --- |
| `main-cosine/` | Main normalized-vector measurement harness |
| `native-l2/` | Native-L2 measurement harness for SIFT and GIST |
| `scoring/` | Scoring, correction-position, exact-budget and engineering controls |
| `geometry/` | Construction/query precision and representation crossovers |
| `system/` | Parallel construction, storage and query-system controls |
| `components/` | Independent numerical analysis for selection and production experiments |
| `ssd/base/` | INT8 rows, direct I/O, codec and I/O-design experiment sources |
| `ssd/sharded/` | Changed I/O files for the independent-shard variant |
| `ssd/pipeline/` | Changed I/O files for the cross-query pipeline variant |
| `ssd/runner-main.rs` | Scale-experiment runner entry point |

The SSD shard and pipeline directories contain overlays on the base runner. Their recorded configuration and identity fields are in the corresponding result summaries.

The programs retain the experiment's data-layout and native-library interfaces. The scoring harness uses the relative `evaluation-data` root. The geometry helper also accepts `QUIVER_EVALUATION_DATA_ROOT`. Input matrices, graph files, per-query trace arrays and external ANN libraries are supplied through the experiment's recorded layout. The current TriviumDB source and the recorded experiment entry points have their respective version identities.

Original source hashes, exported-file hashes and path adaptations are listed in `../provenance/manifest.json`. The main experiment's input and source records are in `../protocols/`; the component and SSD summaries retain their own source and executable hashes.

