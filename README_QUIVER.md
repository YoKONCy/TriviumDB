# QuIVer — Reviewer and Reproducibility Guide

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/YoKONCy/TriviumDB)

The repository provides the ICDE evaluation archive and the production QuIVer runtime in TriviumDB `0.9.1`. Each experiment retains its source, graph, query, metric, and hardware identities. Production interfaces and resource contracts are documented in the [QuIVer runtime guide](docs/quiver.md).

---

## ICDE evaluation archive

The archive accompanies **QuIVer — Efficient Graph Indexing with Two-Bit Construction and Navigation**. It contains 1,056 operating-point observations, 116 index configurations, and 423 parameter curves, covering QuIVer on 21 datasets and 30 external baseline modes on three core workloads. Those workloads are Cohere-1M-768, DBpedia-1536, and Cohere-5M-768.

| Resource | Contents |
| --- | --- |
| [Archive guide](evaluation/README.md) | Directory map, experiment groups, and preparation instructions |
| [Paper PDF](evaluation/paper/main.pdf) | Publication copy with a link to the archive |
| [LaTeX source](evaluation/paper/main.tex) | Manuscript source and associated figures |
| [Complete operating points](evaluation/results/all-operating-points.csv) | Recall, throughput rounds, available latency observations, loading, construction, and memory |
| [Index configurations](evaluation/results/configurations.csv) | Construction parameters, navigation and reranking components, resident memory, and file sizes |
| [Structured snapshot](evaluation/results/benchmark-snapshot.json) | Complete observations, notes, selection identities, and route history |
| [Frozen protocols](evaluation/protocols/) | Dataset splits, metrics, parameters, component inputs, and SSD configurations |
| [Source archive](evaluation/sources/) | Experiment entry points and analysis programs with original content identities |
| [Provenance manifest](evaluation/provenance/manifest.json) | File hashes and source identities |
| [Measurement definitions](evaluation/METRICS.md) | Timing, memory, preparation, and query-accounting boundaries |
| [Offline data browser](evaluation/dashboard/index.html) | Dataset, configuration, and method exploration |

The main comparisons share query IDs and select search parameters on validation queries. Complete observations retain actual evaluation recall and five-round throughput samples. Component and SSD experiments retain their own graph, query, source, and hardware records. Prepare raw vector matrices, built indexes, and full execution directories using the archived input protocols.

Verify the distributed files and their exported data with

```bash
python evaluation/tools/verify_data.py
```

The verifier checks content identities and consistency between the structured snapshot, complete CSV, and offline data copies. Build the publication PDF with the LaTeX dependencies specified by the manuscript source.

```bash
cd evaluation/paper
latexmk -xelatex main.tex
```

---

## Production runtime

Current database queries use BQ graph construction, directed-connectivity protection, training-free SM-L2 navigation, and budgeted original-vector cosine reranking. Persisted default SM snapshots bind the graph to database facts and stable ID/slot mappings. QuIVer supports dimensions `1..=3072`, including partial tail blocks. Higher-dimensional database queries use the applicable exact path.

Explicit INT8 retrieval uses the versioned QVSSD v1 artifact and codec 1, group128 affine quantization, normalized vectors, and reconstruction-norm correction. Database and Reader entry points expose SSD/Hot residency, bounded Ready-row caching, Pending coalescing, bounded batches, native Direct I/O, reusable aligned windows, cancellation draining, and explicit resource release. Runtime I/O alignment is selected through the opened file handle. Configuration, source binding, budgets, and backup requirements are described in the [SSD sections](docs/quiver.md#ssd-索引制品与-reader).

The [kernel/SDK guide](docs/kernel-sdk.md) documents responsibility boundaries. Storage Core owns files, publication, native I/O, draining, and resource lifetimes. SDK adapters provide language representations, request borrowing, and structured errors.

---

## Production measurements

The current production measurement entry point exercises the delivered SSD runtime.

```bash
cargo bench --bench bench_quiver_ssd
```

It uses fixed-seed uniform, clustered, and sparse datasets at dimensions 128, 129, and 768. It compares buffered reads, Direct I/O with several Ready-cache capacities, and Hot residency. It checks complete result equality across configurations and recall against an independent cosine reference, while recording logical reads, system-returned bytes, retained resources, and observed latency. Default sizes, parameters, environment overrides, and output files are listed in the [benchmark guide](benches/README.md#生产-quiver-ssd-测量).

Record the source revision, build identity, artifact digest, dataset and query identities, CPU/thread budget, cache initial state, backend, actual alignment, and operating system with each measurement. Report syscall-returned bytes and device-side traffic using their respective accounting boundaries. Performance observations retain their workload and platform conditions.

---

## Core acceptance and citation

Current correctness entry points and their assertion scope are maintained in [testing practices](docs/testing.md) and the [QuIVer guide](docs/quiver.md#检查与复现入口). Coverage includes independent score expectations, persistence, malformed inputs, source identity, memory admission, cancellation, generation pinning, and release of files and worker threads. Each platform and sanitizer conclusion identifies the commit and actual execution scope.

Use [CITATION.cff](evaluation/CITATION.cff) for the archive's title, authors, and source URL. The associated preprint is available on [arXiv](https://arxiv.org/abs/2605.02171). Version-specific production changes are recorded in the [bilingual release notes](docs/changes/0.9.1.md).
