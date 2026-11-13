// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__binary-sign-fp32-g1"] = [
  {
    "id": "D03__binary-sign-fp32-g1|{\"candidates\":128,\"ef\":256}",
    "config": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
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
        "recall": 0.9337365591397849,
        "qps": 1396.827954684204,
        "batch": null,
        "samples": [
          1393.9850880405716,
          1387.1117239640214,
          1365.5358163146964,
          1415.7018587887478,
          1421.805286312983
        ],
        "cv": 0.014537604580920238,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.11309814453125,
        "pss": 3.1019201278686523,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 102
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__binary-sign-fp32-g1|{\"candidates\":192,\"ef\":384}",
    "config": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
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
        "recall": 0.9541666666666667,
        "qps": 939.0757301461472,
        "batch": null,
        "samples": [
          949.2133932922948,
          939.9088092661971,
          955.5804261998015,
          931.7807276363565,
          918.8952943360861
        ],
        "cv": 0.013772264926724747,
        "p50": 1.142928,
        "p95": 1.3231662999999996,
        "p99": 2.2884335399999993,
        "p95ci": [
          1.31365255,
          1.33987285
        ],
        "p99ci": [
          1.9063763799999998,
          2.554696
        ],
        "resident": 3.1510887145996094,
        "pss": 3.1399106979370117,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9541666666666667,
        "qps": 6437.606468520405,
        "batch": null,
        "samples": [
          6122.451918933001,
          6520.043554050742,
          6628.553724188571,
          6452.69561065498,
          6464.287534774729
        ],
        "cv": 0.026317509447729166,
        "p50": 1.2218740000000001,
        "p95": 1.3759660499999997,
        "p99": 1.8612386699999997,
        "p95ci": [
          1.3623779999999999,
          1.3894090000000001
        ],
        "p99ci": [
          1.490182239999999,
          2.1211617899999995
        ],
        "resident": 3.1519851684570312,
        "pss": 3.1408071517944336,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9541666666666667,
        "qps": 10561.705171605696,
        "batch": null,
        "samples": [
          10504.899675543867,
          10154.838105494231,
          10480.18113847817,
          10541.185648026994,
          11127.421290485217
        ],
        "cv": 0.029817052396876268,
        "p50": 1.249734,
        "p95": 16.179774499999976,
        "p99": 25.304497689999998,
        "p95ci": [
          12.279000149999868,
          17.239166349999998
        ],
        "p99ci": [
          25.22809823,
          25.40759914875
        ],
        "resident": 3.152118682861328,
        "pss": 3.1409406661987305,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9541666666666667,
        "qps": 13795.060541540244,
        "batch": null,
        "samples": [
          13743.976565854166,
          13576.760542618262,
          14106.492065482338,
          13664.269098959261,
          13883.804434787191
        ],
        "cv": 0.013454057273359767,
        "p50": 1.316747,
        "p95": 29.260493699999998,
        "p99": 41.48122932,
        "p95ci": [
          25.712851349999973,
          30.454825427499905
        ],
        "p99ci": [
          41.28572876999999,
          49.220705
        ],
        "resident": 3.15704345703125,
        "pss": 3.1458654403686523,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
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
    "id": "D03__binary-sign-fp32-g1|{\"candidates\":256,\"ef\":512}",
    "config": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 512,
      "candidates": 256
    },
    "parameterText": "ef=512; candidates=256",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9643817204301075,
        "qps": 719.8366472633105,
        "batch": null,
        "samples": [
          738.8942632151103,
          736.2733957867167,
          711.8405286119846,
          705.8528132133014,
          706.3222354894393
        ],
        "cv": 0.0203743179173038,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.174663543701172,
        "pss": 3.163485527038574,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 102
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__binary-sign-fp32-g1|{\"candidates\":384,\"ef\":768}",
    "config": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
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
        "recall": 0.975,
        "qps": 486.97734016525953,
        "batch": null,
        "samples": [
          492.9724665631953,
          492.0435090027999,
          483.91597161588146,
          479.37913704219017,
          486.5756166022309
        ],
        "cv": 0.010425294236207671,
        "p50": 2.151154,
        "p95": 2.43361565,
        "p99": 2.7156474,
        "p95ci": [
          2.4141833299999997,
          2.4591136000000002
        ],
        "p99ci": [
          2.614597,
          2.7909006277499997
        ],
        "resident": 3.1843528747558594,
        "pss": 3.1731748580932617,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.975,
        "qps": 3337.637151466744,
        "batch": null,
        "samples": [
          3388.913873623155,
          3335.5422036013847,
          3350.7974540511273,
          3343.564509067929,
          3269.367716990123
        ],
        "cv": 0.011606082342430083,
        "p50": 2.340104,
        "p95": 2.5955082,
        "p99": 2.7375814599999995,
        "p95ci": [
          2.5820447625000003,
          2.61529275
        ],
        "p99ci": [
          2.698935,
          2.772582023749999
        ],
        "resident": 3.1843910217285156,
        "pss": 3.173213005065918,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.975,
        "qps": 5679.534015161896,
        "batch": null,
        "samples": [
          5674.463480631706,
          5618.037763103388,
          5586.548632498229,
          5809.336536530012,
          5709.283663046141
        ],
        "cv": 0.013680941568695476,
        "p50": 2.4831329999999996,
        "p95": 18.60654735,
        "p99": 26.598627500000003,
        "p95ci": [
          18.56677805,
          18.669623499999997
        ],
        "p99ci": [
          26.5231125,
          27.591288639999988
        ],
        "resident": 3.184703826904297,
        "pss": 3.173525810241699,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.975,
        "qps": 7573.302052859647,
        "batch": null,
        "samples": [
          7573.534087960337,
          7619.846501101373,
          7468.385354879656,
          7614.372518747099,
          7590.371801609772
        ],
        "cv": 0.007268860137020415,
        "p50": 2.5165725,
        "p95": 34.424859299999994,
        "p99": 46.47297740999999,
        "p95ci": [
          34.366319299999994,
          34.4961419
        ],
        "p99ci": [
          42.606851049999996,
          50.25081238
        ],
        "resident": 3.187236785888672,
        "pss": 3.176058769226074,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 64
        },
        "evidence": {
          "$annotation": 102
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
    "id": "D03__binary-sign-fp32-g1|{\"candidates\":96,\"ef\":192}",
    "config": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
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
        "recall": 0.917741935483871,
        "qps": 1822.016883693193,
        "batch": null,
        "samples": [
          1789.7402821893431,
          1839.5387506473262,
          1857.126521539522,
          1812.4029861063525,
          1811.2758779834217
        ],
        "cv": 0.012963088326674218,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.075458526611328,
        "pss": 3.0642805099487305,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 100
        },
        "evidence": {
          "$annotation": 102
        }
      }
    },
    "targets": []
  }
];
