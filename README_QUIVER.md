# QuIVer — Artifact Documentation

> **Paper**: [QuIVer: Rethinking ANN Graph Topology via Training-Free Binary Quantization](https://arxiv.org/abs/2605.02171)
>
> **Venue**: PVLDB Vol. 20, 2027

This document provides the entry points and instructions for reproducing the main experiments reported in the paper.

---

## Table of Contents

- [Repository Layout](#repository-layout)
- [Quick Start](#quick-start)
- [Prerequisites](#prerequisites)
- [Step 1: Prepare Datasets](#step-1-prepare-datasets)
- [Step 2: Reproduce Main Results (§5.2, Table 4)](#step-2-reproduce-main-results)
- [Step 3: Reproduce Baseline Comparisons (§5.3, Table 5 & Figure 3)](#step-3-reproduce-baseline-comparisons)
- [Step 4: Reproduce Sensitivity Analysis (§5.4, Tables 6–8)](#step-4-reproduce-sensitivity-analysis)
- [Step 5: Reproduce Encoding Ablation (§5.5, Table 9)](#step-5-reproduce-encoding-ablation)
- [Step 6: Reproduce Cross-Dataset Boundary (§5.6, Table 10 & Figure 4)](#step-6-reproduce-cross-dataset-boundary)
- [Step 7: Reproduce Scalability (§5.7, Table 11)](#step-7-reproduce-scalability)
- [Step 8: Reproduce SSD Cold/Hot Experiment (§4, Tables 2–3)](#step-8-reproduce-ssd-coldhot-experiment)
- [Step 9: Reproduce BQ2 vs RaBitQ Precision (Supplemental)](#step-9-reproduce-bq2-vs-rabitq-precision)
- [Step 10: Reproduce Build Variance & Tail Latency (Supplemental)](#step-10-reproduce-build-variance--tail-latency)
- [Environment Variables Reference](#environment-variables-reference)
- [Data Format](#data-format)

---

## Repository Layout

```
TriviumDB/
├── src/index/
│   ├── bq.rs              # BQ binary quantization signatures (QuIVer foundation)
│   └── quiver.rs          # 🚀 QuIVer ANN index core implementation
├── benches/
│   ├── bench_cohere1m.rs          # Single-dataset Recall-QPS benchmark
│   ├── bench_sensitivity.rs       # Parameter sensitivity grid scan (§5.4)
│   ├── bench_encoding_ablation.rs # Encoding ablation: 1-bit/2-bit SM/SQ (§5.5)
│   ├── bench_ssd_cold_hot.rs      # SSD cold/hot separation experiment (§5.3)
│   ├── bench_variance.rs          # Build variance + tail latency (Appendix)
│   ├── bench_rbq2_precision.rs    # BQ2 vs RaBitQ top-K precision (Appendix)
│   ├── bench_recall_at_k.rs       # Recall@K for K=1,10,100,500
│   ├── bench_random1m.rs          # Synthetic-LR dataset generator
│   ├── bench_random_sphere.rs     # Random-Sphere dataset generator
│   ├── bench_baselines.py         # All baselines: hnswlib/FAISS/USearch/VSAG
│   ├── bench_rabitq_refine.py     # FAISS IVF+RaBitQ+Refine benchmark
│   ├── run_all_benchmarks.py      # Batch runner for all datasets
│   └── run_vibe_recall.py         # VIBE dataset batch runner
├── scripts/
│   ├── prepare_all.py             # Dataset download & preparation (one command)
│   ├── prepare_msmarco.py         # Deterministic MSMARCO 1M/5M preparation
│   ├── convert_hdf5_to_f32.py     # VIBE HDF5 → f32 format converter
│   └── convert_to_fbin.py         # f32 → DiskANN fbin format converter
└── Cargo.toml                     # Build configuration
```

---

## Quick Start

Run all commands from the repository root. Record the evaluated revision before starting:

```bash
git clone https://github.com/YoKONCy/TriviumDB.git
cd TriviumDB
git rev-parse HEAD
```

Verify the Rust toolchain and the complete QuIVer build/search path without downloading an external dataset:

```bash
cargo test --lib "index::quiver::tests::test_quiver_中等规模搜索" -- --exact
```

The smoke test succeeds when Cargo reports `1 passed; 0 failed`. It deterministically builds a 50-vector QuIVer index, checks graph statistics and Top-1 self-recall, deletes half of the nodes, and verifies that deleted IDs are excluded from subsequent results.

---

## Prerequisites

**Hardware** (paper configuration):
- **Main experiments (§5.1–5.6)**: AMD Ryzen 7 7840HS laptop (Zen 4, 8 cores / 16 threads, AVX-512 with VPOPCNTDQ), 32 GB DDR5-5600 RAM, Windows 11
- **Scalability (§5.7, 5M vectors)**: AMD EPYC 9T24 (Zen 4, 16 threads, AVX-512), 64 GB DDR5, Debian Linux

**Software**:
```bash
# Rust toolchain (stable)
rustup update stable

# Python dependencies (for baselines & data prep)
python -m pip install numpy tqdm h5py datasets huggingface-hub
python -m pip install hnswlib faiss-cpu usearch pyvsag
```

**Compiler flags** (as used in the paper):
- All Rust code compiled with `target-cpu=native` in release mode with LTO
- Bash: `export RUSTFLAGS="-C target-cpu=native"`
- PowerShell: `$env:RUSTFLAGS="-C target-cpu=native"`

---

## Step 1: Prepare Datasets

### 1a. Automated datasets

`prepare_all.py` handles download, sampling, L2-normalization, and ground truth computation for nine datasets. The paper's two synthetic datasets use their dedicated Rust generators in Step 1b.

```bash
python scripts/prepare_all.py                   # prepare all automated datasets
python scripts/prepare_all.py cohere minilm      # prepare specific ones
python scripts/prepare_all.py --list             # show status
```

| Script name | Paper name | Dim | Source | Notes |
|-------------|-----------|-----|--------|-------|
| `cohere` | Cohere-1M | 768 | [HF: YoKONCy/Cohere-1M-wikipedia-768d](https://huggingface.co/datasets/YoKONCy/Cohere-1M-wikipedia-768d) | Streaming, 1M sample |
| `minilm` | MiniLM-1M | 384 | [HF: maloyan/wikipedia-22-12-en-embeddings-all-MiniLM-L6-v2](https://huggingface.co/datasets/maloyan/wikipedia-22-12-en-embeddings-all-MiniLM-L6-v2) | Streaming, 1M sample |
| `bge_m3` | BGE-M3-1M | 1024 | [HF: Qdrant/BGE-m3-1-million-ads](https://huggingface.co/datasets/Qdrant/BGE-m3-1-million-ads) | Full download |
| `dbpedia1536` | DBpedia-1M (1536) | 1536 | [HF: Qdrant/dbpedia-entities-openai3-text-embedding-3-large-1536-1M](https://huggingface.co/datasets/Qdrant/dbpedia-entities-openai3-text-embedding-3-large-1536-1M) | Full download |
| `dbpedia3072` | DBpedia-3072 | 3072 | [HF: Qdrant/dbpedia-entities-openai3-text-embedding-3-large-3072-1M](https://huggingface.co/datasets/Qdrant/dbpedia-entities-openai3-text-embedding-3-large-3072-1M) | Full download |
| `wolt_clip` | Wolt-CLIP-1M | 512 | [HF: Qdrant/wolt-food-clip-ViT-B-32-embeddings](https://huggingface.co/datasets/Qdrant/wolt-food-clip-ViT-B-32-embeddings) | Streaming, 1M sample |
| `sift128` | SIFT-128 | 128 | [ann-benchmarks.com](http://ann-benchmarks.com/sift-128-euclidean.hdf5) | Download HDF5 to project root |
| `gist960` | GIST-960 | 960 | [ann-benchmarks.com](http://ann-benchmarks.com/gist-960-euclidean.hdf5) | Download HDF5 to project root |
| `glove100` | GloVe-100 | 100 | [ann-benchmarks.com](http://ann-benchmarks.com/glove-100-angular.hdf5) | Download HDF5 to project root |

> **ann-benchmarks HDF5**: Download these files manually and place them in the project root before running `prepare_all.py`:
> - `sift-128-euclidean.hdf5` → http://ann-benchmarks.com/sift-128-euclidean.hdf5
> - `gist-960-euclidean.hdf5` → http://ann-benchmarks.com/gist-960-euclidean.hdf5
> - `glove-100-angular.hdf5` → http://ann-benchmarks.com/glove-100-angular.hdf5

### 1b. Synthetic datasets

Use the dedicated deterministic generators. These are the authoritative generators for the paper datasets; `random-1m` denotes Synthetic-LR and `sphere-1m` denotes Random-Sphere.

```bash
cargo bench --bench bench_random1m
cargo bench --bench bench_random_sphere
```

### 1c. Manual datasets

These require manual preparation:

**RedCaps-1M** (512-d, CLIP ViT-B/32):
1. Download the pre-computed CLIP-embedded RedCaps dataset from [Zenodo: 13137120](https://zenodo.org/records/13137120)
2. Convert to f32 format and save as `redcaps_train.f32` (1M×512), `redcaps_test.f32` (10K×512), `redcaps_groundtruth.i32` (10K×10)

**MSMARCO-1M/5M** (1024-d, Cohere Embed v3, §5.7 scalability):

```bash
python scripts/prepare_msmarco.py --sizes 1000000 5000000
```

The script streams [HF: Cohere/msmarco-v2.1-embed-english-v3](https://huggingface.co/datasets/Cohere/msmarco-v2.1-embed-english-v3), uses the first 5M passage embeddings as the largest base set, and reserves the following 10K embeddings as a shared held-out query set. The 1M base is the exact prefix of the 5M base. It L2-normalizes all vectors, computes exact cosine Top-10 independently for each scale using bounded-memory chunks, and writes a manifest with source parameters and SHA-256 checksums. The current dataset schema exposes the 1024-dimensional `emb` field; use `--embedding-column NAME` if a future revision changes it.

### 1d. Output format

Each dataset produces 3 files in the project root:

| File | Format | Description |
|------|--------|-------------|
| `{prefix}_train.f32` | N×D float32 raw binary | Base vectors |
| `{prefix}_test.f32` | Q×D float32 raw binary | Query vectors |
| `{prefix}_groundtruth.i32` | Q×K int32 raw binary | Ground truth top-K IDs |

---

## Step 2: Reproduce Main Results

**Paper section**: §5.2, Table 4 (QuIVer on embedding datasets, 384–3072 dimensions)

```powershell
# Default: Cohere-1M (768-d), m=32, ef_c=128, α=1.2
cargo bench --bench bench_cohere1m

# Other datasets: set TRIVIUM_ANN_NAME
$env:TRIVIUM_ANN_NAME="minilm-384"
cargo bench --bench bench_cohere1m

$env:TRIVIUM_ANN_NAME="dbpedia-1536"
cargo bench --bench bench_cohere1m
```

Set `TRIVIUM_RESULT_PATH` to save a machine-readable JSON result:

```bash
TRIVIUM_ANN_NAME=msmarco-1m TRIVIUM_RESULT_PATH=results/msmarco1m.json cargo bench --bench bench_cohere1m
```

**Available dataset names for `bench_cohere1m`**:

| Name | Dim | Description |
|------|-----|-------------|
| `cohere-1m` (default) | 768 | Cohere Wikipedia embed-english-v2.0 |
| `minilm-384` | 384 | all-MiniLM-L6-v2 sentence embeddings |
| `dbpedia-1536` | 1536 | DBpedia OpenAI text-embedding-3-large |
| `redcaps-512` | 512 | RedCaps CLIP ViT-B/32 |
| `random-1m` | 768 | Synthetic-LR (low-rank + Zipf clusters) |
| `msmarco-1m` | 1024 | MSMARCO Cohere-v3 (1M subset) |
| `msmarco-5m` | 1024 | MSMARCO Cohere-v3 (5M, scalability) |

**Additional datasets for `bench_sensitivity`**: `wolt-clip-512`, `sift-128`, `gist-960`, `glove-100`, `bge-m3-1024`, `dbpedia-3072`, `sphere-1m`, plus all VIBE datasets (see Step 6)

---

## Step 3: Reproduce Baseline Comparisons

**Paper section**: §5.3, Table 5 (matched-recall throughput on Cohere-1M) & Figure 3 (Pareto curves)

All baselines are evaluated on Cohere-1M (768-d) with M=32, ef_c=128 (or DiskANN R=64, L_b=128, α=1.2).

```powershell
# Install baseline systems
python -m pip install hnswlib faiss-cpu usearch pyvsag

# Run all installed baselines on Cohere-1M
python benches/bench_baselines.py

# Run specific baselines only
$env:BASELINES="hnswlib,faiss_hnsw"
python benches/bench_baselines.py

# RaBitQ-specific benchmark (IVF+RaBitQfs+Refine)
python benches/bench_rabitq_refine.py
```

On Bash, select baselines with `BASELINES="hnswlib,faiss_hnsw" python benches/bench_baselines.py`.

**For DiskANN Rust**: Follow the [official DiskANN repository](https://github.com/microsoft/DiskANN) instructions. Convert data files first:
```bash
python scripts/convert_to_fbin.py   # converts cohere → DiskANN fbin format
```

---

## Step 4: Reproduce Sensitivity Analysis

**Paper section**: §5.4, Tables 6–8 (m, ef_c, α sensitivity on Cohere-1M)

```powershell
# Full sensitivity analysis on Cohere-1M (all sub-experiments)
$env:TRIVIUM_ANN_NAME="cohere-1m"
$env:TRIVIUM_SENSITIVITY_MODE="all"
cargo bench --bench bench_sensitivity

# Quick mode: only ef_search curve + Pareto (skips 1a/1b/1c parameter scans)
$env:TRIVIUM_SENSITIVITY_START="1d"
cargo bench --bench bench_sensitivity
```

Sub-experiments:
| Tag | Content | Paper Reference |
|-----|---------|----------------|
| 1a | m sweep (4–64, ef_c=128, α=1.2) | Table 6 |
| 1b | ef_c sweep (16–512, m=32, α=1.2) | Table 7 |
| 1c | α sweep (1.0–1.25, m=32, ef_c=128) | Table 8 |
| 1d | ef_search fine Recall-QPS curve | Figure 3 data points |
| 1e | Pareto curves across m values | Supplemental |
| 1f | m × α cross-experiment | Supplemental |

---

## Step 5: Reproduce Encoding Ablation

**Paper section**: §5.5, Table 9 (1-bit sign vs 2-bit SM vs 2-bit SQ on Cohere-100K)

```bash
cargo bench --bench bench_encoding_ablation --features ablation
```

This runs two phases:
1. **Phase 1**: Top-10 overlap (BQ-ranked vs float32 ground truth) + distance computation latency (ns/call)
2. **Phase 2**: Full graph search Recall@10 / QPS comparison across all three encoding schemes

---

## Step 6: Reproduce Cross-Dataset Boundary

**Paper section**: §5.6, Table 10 & Figure 4 (applicability boundary across 12 datasets)

The main 12-dataset comparison uses `bench_sensitivity` with sub-experiment 1d (ef_search Recall-QPS curve at default m=32, ef_c=128, α=1.2).

```powershell
# Batch runner for VIBE datasets (writes logs to research/bench_logs/)
python benches/run_vibe_recall.py

# Or run individually
$env:TRIVIUM_ANN_NAME="arxiv-nomic"
$env:TRIVIUM_SENSITIVITY_MODE="params"
$env:TRIVIUM_SENSITIVITY_START="1d"
$env:TRIVIUM_SENSITIVITY_END="1d"
cargo bench --bench bench_sensitivity
```

On Bash, run an individual dataset with:

```bash
TRIVIUM_ANN_NAME=arxiv-nomic TRIVIUM_SENSITIVITY_MODE=params TRIVIUM_SENSITIVITY_START=1d TRIVIUM_SENSITIVITY_END=1d cargo bench --bench bench_sensitivity
```

**VIBE dataset names**: `arxiv-nomic`, `ccnews-nomic`, `coco-nomic`, `codesearch-jina`, `gooaq-roberta`, `landmark-nomic`, `landmark-dino`

VIBE HDF5 files must first be converted and validated:

```bash
python scripts/convert_hdf5_to_f32.py --input-dir /path/to/vibe-hdf5
```

The converter requires `train`, `test`, and `neighbors` datasets, verifies finite vectors and Top-10 ID bounds, recomputes exact cosine ground truth when the source base is truncated, and emits a manifest with shapes and SHA-256 checksums. Use `--datasets FILE.hdf5 ...` for a subset and `--force` to replace complete existing artifacts.

---

## Step 7: Reproduce Scalability

**Paper section**: §5.7, Table 11 (MSMARCO Cohere-v3 at 1M and 5M vectors)

> Note: The 5M scalability experiment in the paper was conducted on a separate cloud server (AMD EPYC 9T24, 64 GB DDR5, Debian Linux).

```bash
# 1M subset
TRIVIUM_ANN_NAME=msmarco-1m RAYON_NUM_THREADS=16 cargo bench --bench bench_cohere1m

# 5M full scale
TRIVIUM_ANN_NAME=msmarco-5m RAYON_NUM_THREADS=16 cargo bench --bench bench_cohere1m
```

---

## Step 8: Reproduce SSD Cold/Hot Experiment

**Paper section**: §4, Tables 2–3 (hot/cold memory breakdown, cold-path storage tolerance)

```bash
# Requires administrator privileges (for page cache clearing)
# Also requires 'ablation' feature flag
cargo bench --bench bench_ssd_cold_hot --features ablation
```

> ⚠️ **Windows**: Run as Administrator for `NtSetSystemInformation` page cache clearing.
> **Linux**: Run as root for `/proc/sys/vm/drop_caches`.
> The benchmark exits with a non-zero status instead of reporting invalid SSD-cold measurements when page-cache clearing fails.

---

## Step 9: Reproduce BQ2 vs RaBitQ Precision

**Paper section**: Supplemental material (top-K overlap: BQ2 vs RaBitQ-sym vs RaBitQ-asym)

```bash
cargo bench --bench bench_rbq2_precision
```

Compares three quantization approaches on 100K Cohere vectors:
- **BQ2 (2-bit SM)**: Our approach, no rotation, weighted Hamming
- **RaBitQ-sym**: 4-round FHT-Kac rotation + 1-bit sign + Hamming
- **RaBitQ-asym**: 4-round rotation + 1-bit + f32 asymmetric + correction factors

---

## Step 10: Reproduce Build Variance & Tail Latency

**Paper section**: Supplemental material (construction variance RSD, P50/P95/P99 latency)

```bash
cargo bench --bench bench_variance --features ablation
```

Runs three phases:
1. **Build variance**: 5 independent builds with shuffled insertion order → recall RSD
2. **Tail latency**: Per-query P50/P95/P99/P99.9/Max distribution
3. **Concurrent throughput**: 1/2/4/8/16 parallel clients → QPS-latency curve

---

## Environment Variables Reference

| Variable | Default | Applies to | Description |
|----------|---------|------------|-------------|
| `TRIVIUM_ANN_NAME` | `cohere-1m` | Main, sensitivity | Dataset preset name |
| `TRIVIUM_ANN_DIM` | preset | Main, baselines | Vector dimension override |
| `TRIVIUM_ANN_TRAIN` | preset | Main, baselines | Training set file path |
| `TRIVIUM_ANN_TEST` | preset | Main, baselines | Query set file path |
| `TRIVIUM_ANN_GT` | preset | Main, baselines | Ground truth file path |
| `TRIVIUM_ANN_M` | `32` | Main | Graph max degree |
| `TRIVIUM_ANN_EF_CONSTRUCTION` | `128` | Main | Build-time beam width |
| `TRIVIUM_ANN_ALPHA` | `1.2` | Main | Pruning factor |
| `TRIVIUM_ANN_EF` | `64,128,256,512,1024` | Main | Search ef values (comma-separated) |
| `TRIVIUM_ANN_TRAIN_LIMIT` | `full` | Main | Limit training set size and recompute exact GT |
| `TRIVIUM_ANN_RERANK` | full ef | Main | Comma-separated rerank candidate limits |
| `TRIVIUM_ANN_INCREMENTAL` | `100` | Main | Percentage batch-built before incremental insertion |
| `TRIVIUM_RESULT_PATH` | unset | Main | Write build, exact-search, Recall, QPS, and latency results as JSON |
| `TRIVIUM_EAGER_PRUNE` | unset | QuIVer construction | Set to `1` for the eager-pruning A/B path |
| `TRIVIUM_SENSITIVITY_MODE` | `all` | Sensitivity | `all`, `params`, or `threads` |
| `TRIVIUM_SENSITIVITY_START` | `1a` | Sensitivity | Start from sub-experiment |
| `TRIVIUM_SENSITIVITY_END` | `1e` | Sensitivity | End at sub-experiment |
| `BASELINES` | all | Baselines | Comma-separated baseline names |
| `BASELINE_START` | first | Baselines | Resume from a named baseline |
| `TRIVIUM_MT_THREADS` | available CPUs | Baselines | Multi-thread query worker count |
| `TRIVIUM_BL_M` | `8,16,32,48` | Baselines | HNSW degree sweep |
| `TRIVIUM_BL_EFC` | `64,128,168` | Baselines | HNSW construction beam sweep |
| `TRIVIUM_BL_EF` | preset sweep | Baselines | HNSW search beam sweep |

---

## Data Format

All data files use **raw binary format** (no headers):

- **`.f32`**: Contiguous `float32` values, row-major. File size = N × D × 4 bytes.
- **`.i32`**: Contiguous `int32` values, row-major. File size = Q × K × 4 bytes.

To inspect file dimensions:
```python
import numpy as np
data = np.fromfile("cohere_train.f32", dtype=np.float32)
print(f"Vectors: {len(data) // 768} × 768")  # → 1000000 × 768
```
