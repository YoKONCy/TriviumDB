// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq4-g1"] = [
  {
    "id": "D03__svs-lvq4-g1|{\"ef\":128}",
    "config": "D03__svs-lvq4-g1",
    "dataset": "D03",
    "method": "SVS LVQ4",
    "family": "量化图",
    "parameters": {
      "ef": 128
    },
    "parameterText": "ef=128",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.8998655913978495,
        "qps": 2832.460328971419,
        "batch": null,
        "samples": [
          2793.898925447438,
          2793.1727282888537,
          2771.3746334476004,
          2877.9881313195874,
          2925.8672263536155
        ],
        "cv": 0.020922247420372197,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.5613746643066406,
        "pss": 0.5508918762207031,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 127
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__svs-lvq4-g1|{\"ef\":768}",
    "config": "D03__svs-lvq4-g1",
    "dataset": "D03",
    "method": "SVS LVQ4",
    "family": "量化图",
    "parameters": {
      "ef": 768
    },
    "parameterText": "ef=768",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9092741935483871,
        "qps": 470.412773371534,
        "batch": null,
        "samples": [
          463.64387678111797,
          465.06707260303995,
          468.6179739043706,
          482.126286122866,
          472.60865744627546
        ],
        "cv": 0.014089501706190076,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.5615463256835938,
        "pss": 0.5510025024414062,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 127
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9092741935483871,
        "qps": 3414.4379831160636,
        "batch": null,
        "samples": [
          3407.8040939721186,
          3352.3929573265727,
          3444.80681535368,
          3496.210327742239,
          3370.9757211857077
        ],
        "cv": 0.015156924909569464,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.5623435974121094,
        "pss": 0.5560102462768555,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 128
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9092741935483871,
        "qps": 5877.340423869673,
        "batch": null,
        "samples": [
          5798.963427333306,
          5646.904408987638,
          5865.575612708228,
          6146.033993270423,
          5929.22467704877
        ],
        "cv": 0.02788201661555823,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.5636978149414062,
        "pss": 0.5573644638061523,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 128
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9092741935483871,
        "qps": 9136.158982706324,
        "batch": null,
        "samples": [
          9015.112721239237,
          9250.572804721158,
          9223.597602917598,
          9030.972642925708,
          9160.539141727919
        ],
        "cv": 0.010617355298216674,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.5662117004394531,
        "pss": 0.5597267150878906,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 128
        }
      }
    },
    "targets": [
      {
        "target": null,
        "note": "原冻结最高预算诊断；未设置达标锚点"
      }
    ]
  }
];
