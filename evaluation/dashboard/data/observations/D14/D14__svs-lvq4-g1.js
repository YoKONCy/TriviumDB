// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__svs-lvq4-g1"] = [
  {
    "id": "D14__svs-lvq4-g1|{\"ef\":1024}",
    "config": "D14__svs-lvq4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4",
    "family": "量化图",
    "parameters": {
      "ef": 1024
    },
    "parameterText": "ef=1024",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.8952956989247312,
        "qps": 315.89201528072493,
        "batch": null,
        "samples": [
          316.1684860679955,
          319.2310001702195,
          321.4959212284064,
          313.18939817552285,
          309.3752707614804
        ],
        "cv": 0.013608647303356805,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.602855682373047,
        "pss": 2.592280387878418,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 106
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.8952956989247312,
        "qps": 2224.349737036164,
        "batch": null,
        "samples": [
          2202.3022246179185,
          2207.692535151789,
          2277.9840057768793,
          2243.0413204955307,
          2190.7285991387025
        ],
        "cv": 0.014386151260438282,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.6037254333496094,
        "pss": 2.594315528869629,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.8952956989247312,
        "qps": 3593.9843567371277,
        "batch": null,
        "samples": [
          3645.9956939902067,
          3689.324596608236,
          3559.004430807959,
          3529.597766131299,
          3545.9992961479365
        ],
        "cv": 0.01736142514327363,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.6060562133789062,
        "pss": 2.596632957458496,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.8952956989247312,
        "qps": 4385.738348083953,
        "batch": null,
        "samples": [
          4396.379305441207,
          4403.78838584932,
          4510.142591554762,
          4432.573768350227,
          4185.807689224247
        ],
        "cv": 0.024574820182776676,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.6087303161621094,
        "pss": 2.599307060241699,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 107
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
