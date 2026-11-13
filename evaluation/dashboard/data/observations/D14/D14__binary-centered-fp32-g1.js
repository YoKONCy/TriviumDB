// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__binary-centered-fp32-g1"] = [
  {
    "id": "D14__binary-centered-fp32-g1|{\"candidates\":1024,\"ef\":2048}",
    "config": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 2048,
      "candidates": 1024
    },
    "parameterText": "ef=2048; candidates=1024",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9827956989247312,
        "qps": 153.64659747129397,
        "batch": null,
        "samples": [
          156.41729507110284,
          153.86648068571225,
          153.80498700515764,
          152.77666707377333,
          151.36755752072375
        ],
        "cv": 0.010773783205211312,
        "p50": 6.709488,
        "p95": 8.237307999999999,
        "p99": 11.13795331999999,
        "p95ci": [
          8.112092348749998,
          8.396984549999999
        ],
        "p99ci": [
          10.402009730499993,
          11.745949000000001
        ],
        "resident": 14.140312194824219,
        "pss": 14.128740310668945,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9827956989247312,
        "qps": 1135.7886773699463,
        "batch": null,
        "samples": [
          1130.3510664160417,
          1149.185235397572,
          1129.4673984550998,
          1116.1882424873324,
          1153.7514440936857
        ],
        "cv": 0.012173297677076393,
        "p50": 7.4111934999999995,
        "p95": 8.567384749999999,
        "p99": 8.993753349999999,
        "p95ci": [
          8.5122529,
          8.63948519
        ],
        "p99ci": [
          8.84002025,
          9.10668480725
        ],
        "resident": 14.13949966430664,
        "pss": 14.127927780151367,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9827956989247312,
        "qps": 1816.016076251536,
        "batch": null,
        "samples": [
          1811.345626884546,
          1835.1956455884228,
          1810.6380099944242,
          1818.8603641762477,
          1804.0407346140396
        ],
        "cv": 0.005879867461366184,
        "p50": 9.5864525,
        "p95": 32.03482585,
        "p99": 38.930246129999986,
        "p95ci": [
          31.7658,
          32.406322149999994
        ],
        "p99ci": [
          36.039009,
          41.19253198999999
        ],
        "resident": 14.158313751220703,
        "pss": 14.14674186706543,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9827956989247312,
        "qps": 2453.6198501647095,
        "batch": null,
        "samples": [
          2528.479543381511,
          2451.0327243842607,
          2465.0673955489615,
          2397.238793557134,
          2426.2807939516797
        ],
        "cv": 0.017921533154210394,
        "p50": 32.85115450000001,
        "p95": 60.65365804999998,
        "p99": 84.91644278,
        "p95ci": [
          58.241453265000004,
          64.88070387875
        ],
        "p99ci": [
          80.72032092999996,
          89.85792500000001
        ],
        "resident": 14.121944427490234,
        "pss": 14.110372543334961,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
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
    "id": "D14__binary-centered-fp32-g1|{\"candidates\":192,\"ef\":384}",
    "config": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
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
        "recall": 0.9404569892473118,
        "qps": 837.2524358532144,
        "batch": null,
        "samples": [
          854.3553081556769,
          822.6924197920944,
          820.5390669916503,
          833.3034509376694,
          855.3719333889809
        ],
        "cv": 0.01793762474833298,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 10.458763122558594,
        "pss": 10.447261810302734,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 82
        },
        "evidence": {
          "$annotation": 85
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__binary-centered-fp32-g1|{\"candidates\":384,\"ef\":768}",
    "config": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
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
        "recall": 0.9650537634408602,
        "qps": 429.4616011914421,
        "batch": null,
        "samples": [
          436.70914103434063,
          425.30228082430256,
          422.43620774052505,
          433.1205883984301,
          429.7397879596124
        ],
        "cv": 0.01199338689295602,
        "p50": 2.5939094999999996,
        "p95": 3.21177755,
        "p99": 5.033183960000001,
        "p95ci": [
          3.1612272925,
          3.2684461174999986
        ],
        "p99ci": [
          4.473133749999998,
          5.220131
        ],
        "resident": 11.88177490234375,
        "pss": 11.870203018188477,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9650537634408602,
        "qps": 2957.246328260857,
        "batch": null,
        "samples": [
          3022.7732396007314,
          2962.9114889072393,
          2918.157938866768,
          2934.577434426677,
          2947.811539502869
        ],
        "cv": 0.012151549718496505,
        "p50": 2.8689485,
        "p95": 3.3188996499999996,
        "p99": 3.4940263899999997,
        "p95ci": [
          3.2952352,
          3.3505453750000003
        ],
        "p99ci": [
          3.4506819999999996,
          3.559643
        ],
        "resident": 11.891288757324219,
        "pss": 11.879716873168945,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9650537634408602,
        "qps": 4708.693573622424,
        "batch": null,
        "samples": [
          4495.244153572351,
          4802.548758822248,
          4788.9267837878315,
          4779.943566200723,
          4676.804605728969
        ],
        "cv": 0.024565666834745797,
        "p50": 3.8162065000000003,
        "p95": 20.274539249999997,
        "p99": 28.250956640000002,
        "p95ci": [
          20.079273583750002,
          20.602373276249974
        ],
        "p99ci": [
          27.894426389999996,
          31.656193
        ],
        "resident": 11.914520263671875,
        "pss": 11.902948379516602,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9650537634408602,
        "qps": 5906.937666875664,
        "batch": null,
        "samples": [
          5715.660088049478,
          5882.290651799907,
          5963.035466043151,
          6039.243151023099,
          5934.458977462684
        ],
        "cv": 0.018328004420175015,
        "p50": 4.447771,
        "p95": 40.002290199999976,
        "p99": 60.93806518999997,
        "p95ci": [
          37.504542,
          43.5989278
        ],
        "p99ci": [
          55.87333747449999,
          67.97289418999999
        ],
        "resident": 11.879512786865234,
        "pss": 11.867940902709961,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 85
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
    "id": "D14__binary-centered-fp32-g1|{\"candidates\":512,\"ef\":1024}",
    "config": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
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
        "recall": 0.9711021505376344,
        "qps": 320.733605951302,
        "batch": null,
        "samples": [
          326.43419606498895,
          321.58476825364096,
          312.70167228784936,
          321.20513146496563,
          321.7422616850651
        ],
        "cv": 0.013871666784361051,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 12.566036224365234,
        "pss": 12.554464340209961,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 82
        },
        "evidence": {
          "$annotation": 85
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__binary-centered-fp32-g1|{\"candidates\":96,\"ef\":192}",
    "config": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
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
        "recall": 0.9010752688172043,
        "qps": 1641.3719524178348,
        "batch": null,
        "samples": [
          1658.6799522898718,
          1655.5750341476132,
          1606.4801670585807,
          1622.2557511583318,
          1663.8688574347768
        ],
        "cv": 0.01386693846334475,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 9.542255401611328,
        "pss": 9.530754089355469,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 82
        },
        "evidence": {
          "$annotation": 85
        }
      }
    },
    "targets": []
  }
];
