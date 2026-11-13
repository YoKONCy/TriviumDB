# QuIVer evaluation archive

Materials accompanying **QuIVer — Efficient Graph Indexing with Two-Bit Construction and Navigation**.

[Paper PDF](paper/main.pdf) · [LaTeX source](paper/main.tex) · [arXiv](https://arxiv.org/abs/2605.02171) · [Complete measurements](results/all-operating-points.csv)

## Browse the results

The main evaluation contains **1,056 operating-point observations**, **116 index configurations**, and **423 parameter curves**. It covers QuIVer on 21 datasets and 30 external baseline modes on three core workloads. The core workloads are Cohere-1M-768, DBpedia-1536 and Cohere-5M-768.

| Resource | Contents |
| --- | --- |
| [All operating points](results/all-operating-points.csv) | Recall, 1/8/16/32-thread throughput, five-round QPS samples, available latency quantiles and intervals, preparation, loading and memory |
| [Index configurations](results/configurations.csv) | Construction settings, navigation components, separate reranking vectors, resident memory, file sizes and measurement notes |
| [Method coverage](results/methods.csv) | All 31 methods, their datasets, configurations and observed thread counts |
| [Datasets](results/datasets.csv) | Dataset size, dimensionality and metric |
| [Structured snapshot](results/benchmark-snapshot.json) | Complete records, selection identities, notes, hashes and retained route history |
| [Offline dashboard](dashboard/index.html) | Method comparisons, filtering, tables and interactive charts |

The complete tables include SVS LVQ4, two-level LVQ and RaBitQ with SQ8 refinement alongside the representative methods displayed in the paper. Download the complete `evaluation/` directory and open `dashboard/index.html` locally. The offline dashboard supports English and Chinese, including charts, exports and record notes. Data files are organized by dataset and index configuration, with separate shared annotations. Its [maintenance guide](dashboard/README.md) maps each module to its responsibility.

## Mechanism and system experiments

| Experiment | Results |
| --- | --- |
| Scoring rules, correction positions and candidate coverage | [CSV](results/ablations/scoring-and-retention.csv), [full records](results/ablations/scoring-and-retention.json) |
| Exact budgets and calibration robustness | [Budget and calibration](results/ablations/budget-and-calibration.json) |
| Construction/query precision crossovers and representation geometry | [Summary](results/ablations/construction-and-geometry.json), [CSV](results/ablations/construction-and-geometry.csv) |
| Selection direction, ordering and candidate discovery | [Component measurements](results/ablations/selection-and-representation.csv), [paired effects](results/ablations/paired-effects.csv) |
| Parallel construction, memory and query execution | [System experiments](results/ablations/parallelism-and-storage.json) |
| Production rebuilds and repeated queries | [JSON](results/ablations/production-rebuilds.json), [CSV](results/ablations/production-rebuilds.csv) |
| INT8 local scales and normalization | [JSON](results/ssd/local-quantization.json), [CSV](results/ssd/local-quantization.csv), [reranking quality](results/ssd/reranking-quality.json) |
| I/O design, sharding and query pipelines | [I/O designs](results/ssd/io-designs.json), [shards and pipelines](results/ssd/shards-and-pipeline.json) |
| SSD scale and CPU budgets | [1M/5M and 1/32-CPU observations](results/ssd/scale-1m-5m.json) |

## Read the measurements

The [measurement guide](METRICS.md) defines the timing, memory and query fields. Each record retains its configuration and evidence identity. Blank CSV cells correspond to unavailable or inapplicable measurements. Record-specific notes carry the native interface, timing boundary and component-accounting details.

The main comparisons share query IDs across methods. Search settings are selected on validation queries at the recorded recall targets. The tables retain actual evaluation recall and five-round mean throughput. The component, construction and SSD experiments preserve their own graph, query and hardware identities.

## Protocols and provenance

Frozen dataset, split and parameter records are under [protocols/](protocols/). Source and numerical identities are listed in [the manifest](provenance/manifest.json). [Table provenance](provenance/table-provenance.json) maps the paper's rows to configurations. [Figure provenance](provenance/figure-provenance.json) records plotted data.

[sources/](sources/) retains experiment entry points and analysis programs associated with these records. Their original content hashes bind them to the evaluated implementation. Source records preserve the distinction between the October main evaluation and the later component and SSD experiments.

The archive contains operating-point measurements, round statistics, available confidence intervals, component observations and their frozen identities. Dataset matrices, built graph files and full execution directories are maintained as separate input and run artifacts.

## Verify or rebuild the publication files

```bash
python evaluation/tools/verify_data.py
cd evaluation/paper
latexmk -xelatex main.tex
```

The verifier checks file identities and the equivalence of the structured snapshot, exported CSV and dashboard data. The publication copy adds a direct link to this archive in the experimental setup. Its original manuscript identity is retained in the manifest.

