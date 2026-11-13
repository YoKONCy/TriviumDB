// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D05__svs-lvq4x8-g1"] = [
  {
    "id": "D05__svs-lvq4x8-g1|{\"calibration_target\":0.95,\"ef\":46,\"prefetch_lookahead\":25,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":46}",
    "config": "D05__svs-lvq4x8-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 46,
      "svs_capacity": 46,
      "prefetch_lookahead": 25,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=46; svs_capacity=46; prefetch_lookahead=25; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9551108374384236,
        "qps": 3312.103630588872,
        "batch": null,
        "samples": [
          3273.9441168216113,
          3367.5975434913094,
          3380.133087975181,
          3268.584230435786,
          3270.259174220473
        ],
        "cv": 0.01528137258127765,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.4396324157714844,
        "pss": 2.4290218353271484,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9551108374384236,
        "qps": 24108.90042674692,
        "batch": null,
        "samples": [
          23582.917234641747,
          24457.40275133071,
          24488.25018518042,
          24384.586929292625,
          23631.345033289108
        ],
        "cv": 0.017062531676717497,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.4397048950195312,
        "pss": 2.4290943145751953,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9551108374384236,
        "qps": 33420.15959371482,
        "batch": null,
        "samples": [
          33065.043880299665,
          34607.48171802311,
          33249.26264216043,
          32747.587243473,
          33431.42248461793
        ],
        "cv": 0.019007113840797937,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.4406394958496094,
        "pss": 2.4300289154052734,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9551108374384236,
        "qps": 44294.93696748194,
        "batch": null,
        "samples": [
          44360.78382444709,
          44751.957852235486,
          44093.32428533189,
          44802.17065304181,
          43466.44822235342
        ],
        "cv": 0.01104852557900242,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.4410629272460938,
        "pss": 2.430452346801758,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
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
    "id": "D05__svs-lvq4x8-g1|{\"calibration_target\":0.98,\"ef\":102,\"prefetch_lookahead\":27,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":102}",
    "config": "D05__svs-lvq4x8-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 102,
      "svs_capacity": 102,
      "prefetch_lookahead": 27,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=102; svs_capacity=102; prefetch_lookahead=27; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9839285714285714,
        "qps": 1629.2103292306879,
        "batch": null,
        "samples": [
          1621.5817339250393,
          1651.7985174964872,
          1631.0315435648256,
          1629.3501909588258,
          1612.2896602082612
        ],
        "cv": 0.008040785522428977,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.44085693359375,
        "pss": 2.430246353149414,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9839285714285714,
        "qps": 11666.526866359192,
        "batch": null,
        "samples": [
          11484.750169041698,
          11883.197633176458,
          11665.656785198385,
          11676.551345288137,
          11622.478399091282
        ],
        "cv": 0.010978957429779432,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.440906524658203,
        "pss": 2.430295944213867,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9839285714285714,
        "qps": 19325.921462374616,
        "batch": null,
        "samples": [
          19750.592642251257,
          18983.337078014156,
          19475.56816091165,
          18944.007147951113,
          19476.102282744894
        ],
        "cv": 0.01617388917523108,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.44110107421875,
        "pss": 2.430490493774414,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9839285714285714,
        "qps": 23524.470204422247,
        "batch": null,
        "samples": [
          22867.058538992496,
          23666.725230007127,
          23840.009314684154,
          23493.133898106516,
          23755.424040320933
        ],
        "cv": 0.014802792221965731,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.441761016845703,
        "pss": 2.431150436401367,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 104
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
    "id": "D05__svs-lvq4x8-g1|{\"ef\":32}",
    "config": "D05__svs-lvq4x8-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9327380952380953,
        "qps": 4463.655289535645,
        "batch": null,
        "samples": [
          4482.683267016546,
          4445.487875366952,
          4454.590968008214,
          4509.856907444844,
          4425.657429841671
        ],
        "cv": 0.0066100401542239715,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.4384841918945312,
        "pss": 2.4279022216796875,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 104
        }
      }
    },
    "targets": []
  },
  {
    "id": "D05__svs-lvq4x8-g1|{\"ef\":96}",
    "config": "D05__svs-lvq4x8-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9828509852216749,
        "qps": 1724.242283770888,
        "batch": null,
        "samples": [
          1741.2196397930074,
          1724.961036137776,
          1715.8371238095933,
          1715.7782626871613,
          1723.4153564269018
        ],
        "cv": 0.005389102975529653,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 2.439586639404297,
        "pss": 2.428976058959961,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 104
        }
      }
    },
    "targets": []
  }
];
