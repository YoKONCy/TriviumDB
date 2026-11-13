// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D05__mirage-anns-g1"] = [
  {
    "id": "D05__mirage-anns-g1|{\"ef\":128}",
    "config": "D05__mirage-anns-g1",
    "dataset": "D05",
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
        "recall": 0.9876026272577997,
        "qps": 500.1508057455194,
        "batch": null,
        "samples": [
          499.34310445107883,
          492.8593152467325,
          504.68386094340275,
          501.5753248831392,
          502.2924232032436
        ],
        "cv": 0.008046552362170332,
        "p50": 2.0465285,
        "p95": 2.4354508499999996,
        "p99": 2.7269362900000003,
        "p95ci": [
          2.427464,
          2.4425561537500005
        ],
        "p99ci": [
          2.6900753899999987,
          2.7569193155
        ],
        "resident": 6.105216979980469,
        "pss": 6.095303535461426,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9876026272577997,
        "qps": 2035.725733778665,
        "batch": null,
        "samples": [
          2058.7330326884608,
          2022.0341742414562,
          2036.4451931248216,
          2039.27738333453,
          2022.1388855040568
        ],
        "cv": 0.006643461850417858,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.105094909667969,
        "pss": 6.095181465148926,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9876026272577997,
        "qps": 2138.814990402082,
        "batch": null,
        "samples": [
          2129.6354148766604,
          2126.332066262037,
          2131.4938531120156,
          2159.8599576553556,
          2146.7536601043407
        ],
        "cv": 0.005912596189858909,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.105419158935547,
        "pss": 6.095505714416504,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9876026272577997,
        "qps": 2127.3332710321556,
        "batch": null,
        "samples": [
          2134.0931526085387,
          2122.005077985123,
          2127.966048231534,
          2130.763220125399,
          2121.838856210184
        ],
        "cv": 0.002268449195041913,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.105598449707031,
        "pss": 6.095685005187988,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
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
    "id": "D05__mirage-anns-g1|{\"ef\":32}",
    "config": "D05__mirage-anns-g1",
    "dataset": "D05",
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
        "recall": 0.923450328407225,
        "qps": 1467.6018833056166,
        "batch": null,
        "samples": [
          1479.5992280274775,
          1446.1821049438058,
          1474.1151983776374,
          1445.27628318136,
          1492.8366019978023
        ],
        "cv": 0.012857729925848672,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.102817535400391,
        "pss": 6.092904090881348,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 80
        },
        "evidence": {
          "$annotation": 91
        }
      }
    },
    "targets": []
  },
  {
    "id": "D05__mirage-anns-g1|{\"ef\":64}",
    "config": "D05__mirage-anns-g1",
    "dataset": "D05",
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
        "recall": 0.9664922003284072,
        "qps": 887.5773339577668,
        "batch": null,
        "samples": [
          887.8975180623585,
          887.3234141242067,
          881.8635848777595,
          888.0342329060564,
          892.7679198184525
        ],
        "cv": 0.0039017299420414544,
        "p50": 1.1690030000000002,
        "p95": 1.4061782,
        "p99": 1.6587287399999997,
        "p95ci": [
          1.40018447875,
          1.4123798187499996
        ],
        "p99ci": [
          1.6323954777499983,
          1.6854751074999959
        ],
        "resident": 6.10400390625,
        "pss": 6.094090461730957,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9664922003284072,
        "qps": 3654.3444184052873,
        "batch": null,
        "samples": [
          3631.788892358484,
          3606.213898122791,
          3689.5251906627127,
          3696.0956121382515,
          3648.0984987441984
        ],
        "cv": 0.00935629037633553,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.104549407958984,
        "pss": 6.094635963439941,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9664922003284072,
        "qps": 3878.6667279385056,
        "batch": null,
        "samples": [
          3871.4385663114404,
          3902.6932705489517,
          3875.1385866650066,
          3880.1214078747726,
          3863.941808292357
        ],
        "cv": 0.0033830697835624925,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.105030059814453,
        "pss": 6.09511661529541,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9664922003284072,
        "qps": 3853.340275329485,
        "batch": null,
        "samples": [
          3871.502458773685,
          3834.336670608854,
          3847.528932480818,
          3846.676953449186,
          3866.656361334882
        ],
        "cv": 0.0035705574109255833,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.1051788330078125,
        "pss": 6.0952653884887695,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 91
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
    "id": "D05__mirage-anns-g1|{\"ef\":96}",
    "config": "D05__mirage-anns-g1",
    "dataset": "D05",
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
        "recall": 0.9807881773399014,
        "qps": 639.6677137541587,
        "batch": null,
        "samples": [
          637.2655480219534,
          642.1992323964969,
          639.9601331969847,
          638.0276444957142,
          640.8860106596447
        ],
        "cv": 0.002834605275838058,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.105182647705078,
        "pss": 6.095269203186035,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 80
        },
        "evidence": {
          "$annotation": 91
        }
      }
    },
    "targets": []
  }
];
