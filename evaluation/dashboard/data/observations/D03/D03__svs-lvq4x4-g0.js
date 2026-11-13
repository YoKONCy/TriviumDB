// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq4x4-g0"] = [
  {
    "id": "D03__svs-lvq4x4-g0|{\"ef\":32}",
    "config": "D03__svs-lvq4x4-g0",
    "dataset": "D03",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9650537634408602,
        "qps": 5101.8818975231325,
        "batch": null,
        "samples": [
          5011.983478420631,
          4927.702685109793,
          4989.972085123485,
          5280.942350338366,
          5298.808888623388
        ],
        "cv": 0.0305903702954365,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0401725769042969,
        "pss": 1.0298852920532227,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9650537634408602,
        "qps": 37000.589329804825,
        "batch": null,
        "samples": [
          36643.175986642156,
          36440.76021719277,
          36628.52731749345,
          37867.918897542535,
          37422.564230153206
        ],
        "cv": 0.014851878218400965,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0403366088867188,
        "pss": 1.0300493240356445,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9650537634408602,
        "qps": 61650.861515561686,
        "batch": null,
        "samples": [
          62371.27708229271,
          60717.45098768278,
          62170.13327892075,
          61372.4655028328,
          61622.980726079375
        ],
        "cv": 0.009563636325767236,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0405921936035156,
        "pss": 1.0303049087524414,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9650537634408602,
        "qps": 81459.63317051067,
        "batch": null,
        "samples": [
          81324.57994155456,
          81460.7482678959,
          81263.09175990566,
          81412.2282445572,
          81837.51763864003
        ],
        "cv": 0.0024669181156128078,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0410614013671875,
        "pss": 1.0307741165161133,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      }
    },
    "targets": [
      {
        "target": 0.95,
        "note": "独立调参集达标"
      }
    ]
  },
  {
    "id": "D03__svs-lvq4x4-g0|{\"ef\":64}",
    "config": "D03__svs-lvq4x4-g0",
    "dataset": "D03",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9815860215053763,
        "qps": 2681.610447360826,
        "batch": null,
        "samples": [
          2730.2916945840275,
          2700.327762816597,
          2670.02108453589,
          2658.302410321683,
          2649.1092845459334
        ],
        "cv": 0.011130112285890165,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0408859252929688,
        "pss": 1.0305986404418945,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9815860215053763,
        "qps": 20124.77585090359,
        "batch": null,
        "samples": [
          20105.74862593853,
          20140.979177659236,
          19828.20467099234,
          20262.340805190765,
          20286.605974737064
        ],
        "cv": 0.008125253236299111,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0409469604492188,
        "pss": 1.0306596755981445,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9815860215053763,
        "qps": 33508.936709753776,
        "batch": null,
        "samples": [
          33232.86026656076,
          33723.63714650768,
          33375.74317571632,
          33226.6258047915,
          33985.8171551926
        ],
        "cv": 0.008924633511876967,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.041107177734375,
        "pss": 1.0308198928833008,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9815860215053763,
        "qps": 44719.514344640105,
        "batch": null,
        "samples": [
          44988.45075532406,
          44762.5899293566,
          44227.390493997635,
          44621.3498084985,
          44997.79073602373
        ],
        "cv": 0.006351972163569924,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0416107177734375,
        "pss": 1.0313234329223633,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 115
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
