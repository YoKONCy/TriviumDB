// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq8-g0"] = [
  {
    "id": "D03__svs-lvq8-g0|{\"ef\":32}",
    "config": "D03__svs-lvq8-g0",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9674731182795699,
        "qps": 3526.6038219488164,
        "batch": null,
        "samples": [
          3588.2253452147816,
          3546.901133130922,
          3487.8742862742038,
          3455.3943917743854,
          3554.6239533497887
        ],
        "cv": 0.01363743382361394,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0383834838867188,
        "pss": 1.0278005599975586,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 119
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__svs-lvq8-g0|{\"ef\":64}",
    "config": "D03__svs-lvq8-g0",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9829301075268817,
        "qps": 1942.7703490433173,
        "batch": null,
        "samples": [
          1930.3227992946217,
          1941.9648892613775,
          1953.5495397167492,
          1947.8077155130297,
          1940.206801430808
        ],
        "cv": 0.004011769585909061,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0385665893554688,
        "pss": 1.0279531478881836,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 119
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9829301075268817,
        "qps": 13184.250229820987,
        "batch": null,
        "samples": [
          13092.264605958704,
          13493.196018960341,
          13302.2425750733,
          13079.46499000631,
          12954.082959106285
        ],
        "cv": 0.01446245859170838,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0386695861816406,
        "pss": 1.0280561447143555,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 119
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9829301075268817,
        "qps": 19849.38978673153,
        "batch": null,
        "samples": [
          19490.305676441207,
          19550.782093785037,
          20313.594579950015,
          19993.220164523944,
          19899.046418957452
        ],
        "cv": 0.015226696451911584,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0389823913574219,
        "pss": 1.0283689498901367,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 119
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9829301075268817,
        "qps": 21973.390273614,
        "batch": null,
        "samples": [
          21889.14215257964,
          22063.699993906503,
          21814.24332006098,
          22116.41804646883,
          21983.447855054048
        ],
        "cv": 0.0050324407175714675,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.03961181640625,
        "pss": 1.0289983749389648,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 119
        }
      }
    },
    "targets": [
      {
        "target": 0.98,
        "note": "独立调参集达标"
      }
    ]
  }
];
