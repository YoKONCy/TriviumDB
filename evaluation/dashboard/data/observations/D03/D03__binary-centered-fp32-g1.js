// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__binary-centered-fp32-g1"] = [
  {
    "id": "D03__binary-centered-fp32-g1|{\"candidates\":128,\"ef\":256}",
    "config": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 256,
      "candidates": 128
    },
    "parameterText": "ef=256; candidates=128",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9361559139784946,
        "qps": 1380.1337890654481,
        "batch": null,
        "samples": [
          1393.6386768851253,
          1395.5384070410832,
          1389.676478055271,
          1359.2303002702267,
          1362.5850830755348
        ],
        "cv": 0.011482325510544492,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1563644409179688,
        "pss": 3.145254135131836,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 101
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__binary-centered-fp32-g1|{\"candidates\":192,\"ef\":384}",
    "config": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 384,
      "candidates": 192
    },
    "parameterText": "ef=384; candidates=192",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9522849462365591,
        "qps": 938.3198961868143,
        "batch": null,
        "samples": [
          929.1924797525093,
          956.3019196042726,
          938.0827797217876,
          937.8681561493177,
          930.1541457061844
        ],
        "cv": 0.010372206252697904,
        "p50": 1.1068434999999999,
        "p95": 1.2530811,
        "p99": 1.34189337,
        "p95ci": [
          1.2429400000000002,
          1.2634683
        ],
        "p99ci": [
          1.321494,
          1.36191903
        ],
        "resident": 3.172710418701172,
        "pss": 3.161600112915039,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9522849462365591,
        "qps": 6259.646850259336,
        "batch": null,
        "samples": [
          6107.691043626736,
          5770.71395068422,
          6540.299521227468,
          6598.726555733639,
          6280.803180024615
        ],
        "cv": 0.04826216724045944,
        "p50": 1.2298995,
        "p95": 1.4033588499999998,
        "p99": 1.7353227099999984,
        "p95ci": [
          1.3940754825,
          1.413055086249999
        ],
        "p99ci": [
          1.51920864,
          2.0809334499999994
        ],
        "resident": 3.172771453857422,
        "pss": 3.161661148071289,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9522849462365591,
        "qps": 8662.189107927688,
        "batch": null,
        "samples": [
          8668.68085731065,
          8848.176598912365,
          8612.244936625073,
          8621.735638420436,
          8560.107508369916
        ],
        "cv": 0.011449957591116874,
        "p50": 1.2645410000000001,
        "p95": 17.128593400000003,
        "p99": 25.27026811,
        "p95ci": [
          13.457544277499885,
          17.2293119
        ],
        "p99ci": [
          25.18024304,
          25.329938
        ],
        "resident": 3.1732139587402344,
        "pss": 3.1621036529541016,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9522849462365591,
        "qps": 13439.449419885988,
        "batch": null,
        "samples": [
          13718.294999442556,
          13566.380169642529,
          13020.037892195185,
          13366.50867444892,
          13526.025363700754
        ],
        "cv": 0.017689290566693126,
        "p50": 1.329623,
        "p95": 25.468574399999998,
        "p99": 41.518077639999994,
        "p95ci": [
          25.383778341250004,
          26.6361221712498
        ],
        "p99ci": [
          41.26621,
          45.69400278999999
        ],
        "resident": 3.1789093017578125,
        "pss": 3.1677989959716797,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
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
    "id": "D03__binary-centered-fp32-g1|{\"candidates\":384,\"ef\":768}",
    "config": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 768,
      "candidates": 384
    },
    "parameterText": "ef=768; candidates=384",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9701612903225807,
        "qps": 483.55126717366596,
        "batch": null,
        "samples": [
          488.86276568731233,
          481.8590930043204,
          479.24741624842414,
          487.92295587351623,
          479.86410505475675
        ],
        "cv": 0.008390478216782625,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.189075469970703,
        "pss": 3.1779651641845703,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 101
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__binary-centered-fp32-g1|{\"candidates\":512,\"ef\":1024}",
    "config": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 1024,
      "candidates": 512
    },
    "parameterText": "ef=1024; candidates=512",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9790322580645161,
        "qps": 362.8930358674825,
        "batch": null,
        "samples": [
          364.5723114457704,
          369.675191561204,
          358.46148375505476,
          354.83416645235593,
          366.9220261230275
        ],
        "cv": 0.01507481809165142,
        "p50": 2.8498164999999998,
        "p95": 3.27240435,
        "p99": 3.6066287799999994,
        "p95ci": [
          3.24602,
          3.3012078000000002
        ],
        "p99ci": [
          3.518065084749998,
          3.82335
        ],
        "resident": 3.1895599365234375,
        "pss": 3.1784496307373047,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9790322580645161,
        "qps": 2502.5591098522636,
        "batch": null,
        "samples": [
          2575.4854292361388,
          2521.9160190911825,
          2549.8740068593484,
          2428.1950977335378,
          2437.3249963411104
        ],
        "cv": 0.023786326750074086,
        "p50": 3.176858,
        "p95": 3.7728418999999995,
        "p99": 5.799415819999997,
        "p95ci": [
          3.7243618999999994,
          3.820516
        ],
        "p99ci": [
          5.379478079999998,
          6.6444766907499995
        ],
        "resident": 3.189594268798828,
        "pss": 3.1784839630126953,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9790322580645161,
        "qps": 4180.264558119432,
        "batch": null,
        "samples": [
          4089.3313553500852,
          4102.92103297821,
          4255.566087295777,
          4266.080254063707,
          4187.42406090938
        ],
        "cv": 0.01768825188894632,
        "p50": 3.2678445000000003,
        "p95": 19.573806549999997,
        "p99": 27.59910811,
        "p95ci": [
          19.5031332,
          19.786786949999996
        ],
        "p99ci": [
          27.42310887,
          31.030539329999986
        ],
        "resident": 3.1901397705078125,
        "pss": 3.1790294647216797,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9790322580645161,
        "qps": 5861.176453402844,
        "batch": null,
        "samples": [
          5924.248587851635,
          5892.223517555371,
          5872.616078128044,
          5819.453617132187,
          5797.340466346983
        ],
        "cv": 0.007961930286304203,
        "p50": 3.6031210000000002,
        "p95": 35.6255633,
        "p99": 47.416760180000004,
        "p95ci": [
          35.53942572124999,
          35.694655
        ],
        "p99ci": [
          43.56651078,
          51.033809
        ],
        "resident": 3.1928672790527344,
        "pss": 3.1817569732666016,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 101
        }
      }
    },
    "targets": [
      {
        "target": 0.98,
        "note": "独立调参集达标"
      }
    ]
  },
  {
    "id": "D03__binary-centered-fp32-g1|{\"candidates\":96,\"ef\":192}",
    "config": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 192,
      "candidates": 96
    },
    "parameterText": "ef=192; candidates=96",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9198924731182796,
        "qps": 1795.6476100348239,
        "batch": null,
        "samples": [
          1780.1588765608183,
          1796.0378197080304,
          1803.5808045044853,
          1798.3764956943962,
          1800.0840537063889
        ],
        "cv": 0.004525100134782681,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1423377990722656,
        "pss": 3.131227493286133,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 101
        }
      }
    },
    "targets": []
  }
];
