// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__mirage-anns-g0"] = [
  {
    "id": "D03__mirage-anns-g0|{\"ef\":128}",
    "config": "D03__mirage-anns-g0",
    "dataset": "D03",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 128
    },
    "parameterText": "ef=128",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9842741935483871,
        "qps": 972.7705035308418,
        "batch": null,
        "samples": [
          960.027696555942,
          985.4403805970245,
          983.7027192842078,
          972.6739890965979,
          962.0077321204369
        ],
        "cv": 0.010862373569212052,
        "p50": 1.0827419999999999,
        "p95": 1.2993953999999999,
        "p99": 1.4392427199999995,
        "p95ci": [
          1.2836315,
          1.3176436499999995
        ],
        "p99ci": [
          1.4047034900000002,
          1.48662856825
        ],
        "resident": 3.1905860900878906,
        "pss": 3.1804323196411133,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9842741935483871,
        "qps": 4230.830560214802,
        "batch": null,
        "samples": [
          4229.003929334488,
          4216.710563836345,
          4181.476703436642,
          4265.099756007181,
          4261.861848459356
        ],
        "cv": 0.007304893976820376,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.190502166748047,
        "pss": 3.1803483963012695,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9842741935483871,
        "qps": 4662.821824632834,
        "batch": null,
        "samples": [
          4629.508377150416,
          4683.72697157586,
          4692.77256683203,
          4650.306544694873,
          4657.794662910994
        ],
        "cv": 0.004914664747016856,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.190601348876953,
        "pss": 3.180447578430176,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9842741935483871,
        "qps": 4657.715207269685,
        "batch": null,
        "samples": [
          4678.299388452359,
          4679.9409083233595,
          4644.902055185522,
          4642.983928697331,
          4642.449755689853
        ],
        "cv": 0.0037580187356012575,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1907691955566406,
        "pss": 3.1806154251098633,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
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
    "id": "D03__mirage-anns-g0|{\"ef\":32}",
    "config": "D03__mirage-anns-g0",
    "dataset": "D03",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.916263440860215,
        "qps": 3022.1443858268017,
        "batch": null,
        "samples": [
          2966.833923944528,
          3040.169507906418,
          3017.2529525123723,
          3046.693956645968,
          3039.7715881247227
        ],
        "cv": 0.00972782934321153,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1901817321777344,
        "pss": 3.180027961730957,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 107
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__mirage-anns-g0|{\"ef\":64}",
    "config": "D03__mirage-anns-g0",
    "dataset": "D03",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9603494623655914,
        "qps": 1757.2369753199612,
        "batch": null,
        "samples": [
          1769.9407465932652,
          1771.7434905213363,
          1743.5795007809293,
          1769.7876775978373,
          1731.1334611064374
        ],
        "cv": 0.009513143592876928,
        "p50": 0.6108415,
        "p95": 0.7771635499999996,
        "p99": 0.96435458,
        "p95ci": [
          0.7630950037499998,
          0.7947215499999998
        ],
        "p99ci": [
          0.8861176609999998,
          1.0373119999999998
        ],
        "resident": 3.1902999877929688,
        "pss": 3.1801462173461914,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9603494623655914,
        "qps": 7831.582420228548,
        "batch": null,
        "samples": [
          7881.166985648666,
          7870.321871704904,
          7817.565814331814,
          7821.366179841014,
          7767.491249616338
        ],
        "cv": 0.005223642361735374,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1902084350585938,
        "pss": 3.1800546646118164,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9603494623655914,
        "qps": 8635.463605538822,
        "batch": null,
        "samples": [
          8671.505342440942,
          8607.080150969003,
          8580.39230494865,
          8646.130437442922,
          8672.209791892597
        ],
        "cv": 0.004207769437434356,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1903114318847656,
        "pss": 3.1801576614379883,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9603494623655914,
        "qps": 8619.370729305885,
        "batch": null,
        "samples": [
          8611.039934790831,
          8576.783727146058,
          8636.466532640967,
          8656.220308134936,
          8616.343143816632
        ],
        "cv": 0.0030880687221397874,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1904945373535156,
        "pss": 3.1803407669067383,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 108
        },
        "evidence": {
          "$annotation": 107
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
    "id": "D03__mirage-anns-g0|{\"ef\":96}",
    "config": "D03__mirage-anns-g0",
    "dataset": "D03",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.975268817204301,
        "qps": 1256.9661567953895,
        "batch": null,
        "samples": [
          1266.5586588080969,
          1278.0596826211613,
          1262.2317722276252,
          1242.7155258512933,
          1235.265144468771
        ],
        "cv": 0.012521992491748244,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1905479431152344,
        "pss": 3.180394172668457,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 107
        }
      }
    },
    "targets": []
  }
];
