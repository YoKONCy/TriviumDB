// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__D14-diskann-pq-only-strong-r64-l400"] = [
  {
    "id": "D14__D14-diskann-pq-only-strong-r64-l400|{\"ef\":128}",
    "config": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "parameters": {
      "ef": 128
    },
    "parameterText": "ef=128",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9100806451612903,
        "qps": 696.9712002402773,
        "batch": null,
        "samples": [
          705.0023719236253,
          720.7065657449587,
          693.1794686494163,
          687.7941891615852,
          678.1734057218008
        ],
        "cv": 0.021085100448206995,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.46271514892578,
        "pss": 16.460871696472168,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 80
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-diskann-pq-only-strong-r64-l400|{\"ef\":192}",
    "config": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "parameters": {
      "ef": 192
    },
    "parameterText": "ef=192",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9353494623655914,
        "qps": 517.1672644113248,
        "batch": null,
        "samples": [
          510.3661946644553,
          514.9127055721239,
          529.1757211748758,
          520.1165017201362,
          511.2651989250332
        ],
        "cv": 0.013370548799877336,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.462535858154297,
        "pss": 16.460741996765137,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 80
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-diskann-pq-only-strong-r64-l400|{\"ef\":384}",
    "config": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "parameters": {
      "ef": 384
    },
    "parameterText": "ef=384",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9657258064516129,
        "qps": 288.36715135436475,
        "batch": null,
        "samples": [
          287.3778297241016,
          288.3478450370987,
          292.84930290512096,
          287.1264023050156,
          286.13437680048685
        ],
        "cv": 0.00814653677957955,
        "p50": 3.435,
        "p95": 4.66,
        "p99": 5.922169999999997,
        "p95ci": [
          4.602,
          4.7340075
        ],
        "p99ci": [
          5.7474300000000005,
          6.18062
        ],
        "resident": 16.462783813476562,
        "pss": 16.46093463897705,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9657258064516129,
        "qps": 2226.467595643895,
        "batch": null,
        "samples": [
          2249.5747855781246,
          2222.76798567793,
          2231.6994057119464,
          2217.203267396853,
          2211.092533854621
        ],
        "cv": 0.0060165627460029595,
        "p50": 3.7755,
        "p95": 5.2090499999999995,
        "p99": 5.63463,
        "p95ci": [
          5.10814625,
          5.309049999999999
        ],
        "p99ci": [
          5.53267,
          5.71462
        ],
        "resident": 16.473033905029297,
        "pss": 16.47110366821289,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9657258064516129,
        "qps": 3952.086669177313,
        "batch": null,
        "samples": [
          3696.9081027344087,
          3970.373994252414,
          4011.8997780550058,
          4063.329029411872,
          4017.922441432864
        ],
        "cv": 0.033133976300047,
        "p50": 4.293,
        "p95": 5.58515,
        "p99": 6.274369999999996,
        "p95ci": [
          5.521882499999999,
          5.6500075
        ],
        "p99ci": [
          6.02748,
          6.724789999999992
        ],
        "resident": 16.48331069946289,
        "pss": 16.481480598449707,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9657258064516129,
        "qps": 6287.417423412266,
        "batch": null,
        "samples": [
          6271.2601074048325,
          6260.061335944779,
          6318.66144839113,
          6291.375909571365,
          6295.728315749222
        ],
        "cv": 0.0032364131868453565,
        "p50": 4.4065,
        "p95": 5.715049999999999,
        "p99": 6.162049999999999,
        "p95ci": [
          5.667,
          5.7883187499999975
        ],
        "p99ci": [
          6.0313825,
          6.385
        ],
        "resident": 16.500648498535156,
        "pss": 16.49882411956787,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
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
    "id": "D14__D14-diskann-pq-only-strong-r64-l400|{\"ef\":512}",
    "config": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "parameters": {
      "ef": 512
    },
    "parameterText": "ef=512",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9737903225806451,
        "qps": 227.31060395507419,
        "batch": null,
        "samples": [
          224.61597970885538,
          225.66169516350206,
          232.93938770715053,
          229.8130148474774,
          223.52294234838558
        ],
        "cv": 0.01552530462863439,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.462684631347656,
        "pss": 16.460841178894043,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 80
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-diskann-pq-only-strong-r64-l400|{\"ef\":768}",
    "config": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "parameters": {
      "ef": 768
    },
    "parameterText": "ef=768",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9825268817204301,
        "qps": 160.41153881370377,
        "batch": null,
        "samples": [
          157.918408550137,
          162.3703172991375,
          161.68709599477992,
          161.6703943511815,
          158.41147787328288
        ],
        "cv": 0.011583948874982538,
        "p50": 6.311,
        "p95": 8.5473,
        "p99": 9.512019999999996,
        "p95ci": [
          8.443048749999999,
          8.699024999999999
        ],
        "p99ci": [
          9.26863,
          9.77
        ],
        "resident": 16.46265411376953,
        "pss": 16.460830688476562,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9825268817204301,
        "qps": 1238.6488538942967,
        "batch": null,
        "samples": [
          1242.1953513975998,
          1228.47612861843,
          1225.714475740824,
          1243.8941243495535,
          1252.9641893650767
        ],
        "cv": 0.008199893754699253,
        "p50": 6.5135,
        "p95": 9.0301,
        "p99": 10.98934,
        "p95ci": [
          8.87897625,
          9.201399999999998
        ],
        "p99ci": [
          10.60053,
          11.158
        ],
        "resident": 16.480445861816406,
        "pss": 16.47859764099121,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9825268817204301,
        "qps": 2186.37445048846,
        "batch": null,
        "samples": [
          2150.995757459232,
          2181.960333014587,
          2188.703307550003,
          2209.622554904543,
          2200.5902995139336
        ],
        "cv": 0.009191195836920426,
        "p50": 7.5345,
        "p95": 10.517199999999999,
        "p99": 11.53567,
        "p95ci": [
          10.38519625,
          10.629516249999996
        ],
        "p99ci": [
          11.376009999999999,
          11.892
        ],
        "resident": 16.500957489013672,
        "pss": 16.49912738800049,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9825268817204301,
        "qps": 3448.8842729141847,
        "batch": null,
        "samples": [
          3447.7253295301684,
          3456.9417884792024,
          3443.586284009997,
          3426.919183705317,
          3469.248778846239
        ],
        "cv": 0.004083059627756826,
        "p50": 8.337,
        "p95": 11.058099999999998,
        "p99": 11.964449999999998,
        "p95ci": [
          10.9219825,
          11.1572
        ],
        "p99ci": [
          11.69211,
          12.52234
        ],
        "resident": 16.539249420166016,
        "pss": 16.53740119934082,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 80
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
