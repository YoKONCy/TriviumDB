// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D05__svs-lvq4x4-g1"] = [
  {
    "id": "D05__svs-lvq4x4-g1|{\"calibration_target\":0.95,\"ef\":45,\"prefetch_lookahead\":3,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":45}",
    "config": "D05__svs-lvq4x4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 45,
      "svs_capacity": 45,
      "prefetch_lookahead": 3,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=45; svs_capacity=45; prefetch_lookahead=3; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9516215106732349,
        "qps": 3086.1612111899585,
        "batch": null,
        "samples": [
          2901.3890249729066,
          3095.1924876661596,
          3121.3477263895456,
          3157.5362669586925,
          3155.340549962488
        ],
        "cv": 0.030856026395843995,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7328033447265625,
        "pss": 1.7223148345947266,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9516215106732349,
        "qps": 22461.314807617237,
        "batch": null,
        "samples": [
          23039.496479985162,
          22211.56673362099,
          21879.257550598,
          22644.01828624526,
          22532.23498763677
        ],
        "cv": 0.017515137719105074,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.732879638671875,
        "pss": 1.722391128540039,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9516215106732349,
        "qps": 36995.54091580733,
        "batch": null,
        "samples": [
          36759.90370409031,
          36946.30950049647,
          38145.54153120485,
          37290.69212806072,
          35835.25771518429
        ],
        "cv": 0.020277475221825814,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7337875366210938,
        "pss": 1.7232990264892578,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9516215106732349,
        "qps": 41424.79953314464,
        "batch": null,
        "samples": [
          40140.280789761906,
          41224.00131176407,
          42262.44609362138,
          41787.52939300557,
          41709.74007757024
        ],
        "cv": 0.017423502058792326,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7343330383300781,
        "pss": 1.7238445281982422,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
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
    "id": "D05__svs-lvq4x4-g1|{\"calibration_target\":0.98,\"ef\":135,\"prefetch_lookahead\":22,\"prefetch_step\":2,\"search_buffer_visited_set\":false,\"svs_capacity\":135}",
    "config": "D05__svs-lvq4x4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 135,
      "svs_capacity": 135,
      "prefetch_lookahead": 22,
      "prefetch_step": 2,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=135; svs_capacity=135; prefetch_lookahead=22; prefetch_step=2; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9847290640394089,
        "qps": 1197.062704568649,
        "batch": null,
        "samples": [
          1182.6703480915683,
          1197.579772670944,
          1205.8291644612727,
          1196.7768448430452,
          1202.4573927764147
        ],
        "cv": 0.006614195463486621,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7341079711914062,
        "pss": 1.7236194610595703,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9847290640394089,
        "qps": 8281.978686593007,
        "batch": null,
        "samples": [
          8326.96951811515,
          8123.654657361179,
          8130.785942934779,
          8398.71537644238,
          8429.767938111549
        ],
        "cv": 0.01578186134733285,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7342071533203125,
        "pss": 1.7237186431884766,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9847290640394089,
        "qps": 11727.33149728598,
        "batch": null,
        "samples": [
          11675.107378434732,
          11594.087021927002,
          11722.791909653375,
          11809.098042570153,
          11835.573133844644
        ],
        "cv": 0.007521653993381481,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7344474792480469,
        "pss": 1.723958969116211,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9847290640394089,
        "qps": 16797.84761822513,
        "batch": null,
        "samples": [
          16524.225957104118,
          16987.525001734564,
          16808.351201273614,
          16867.595622498764,
          16801.540308514577
        ],
        "cv": 0.009061109748675992,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.73516845703125,
        "pss": 1.724679946899414,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 103
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
    "id": "D05__svs-lvq4x4-g1|{\"ef\":32}",
    "config": "D05__svs-lvq4x4-g1",
    "dataset": "D05",
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
        "recall": 0.9307573891625616,
        "qps": 3389.7217770450898,
        "batch": null,
        "samples": [
          3321.1689933249486,
          3461.9275930375993,
          3373.976742683792,
          3397.211812902443,
          3394.3237432766664
        ],
        "cv": 0.013349527860579004,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7315216064453125,
        "pss": 1.7210865020751953,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 103
        }
      }
    },
    "targets": []
  },
  {
    "id": "D05__svs-lvq4x4-g1|{\"ef\":96}",
    "config": "D05__svs-lvq4x4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9794437602627257,
        "qps": 1289.6135148029464,
        "batch": null,
        "samples": [
          1292.6666738346532,
          1267.51299115741,
          1286.4197981612572,
          1285.2244246732885,
          1316.243686188123
        ],
        "cv": 0.012193609054588795,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7327613830566406,
        "pss": 1.7222728729248047,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 103
        }
      }
    },
    "targets": []
  }
];
