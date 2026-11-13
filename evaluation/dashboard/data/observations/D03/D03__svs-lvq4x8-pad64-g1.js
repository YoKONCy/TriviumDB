// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq4x8-pad64-g1"] = [
  {
    "id": "D03__svs-lvq4x8-pad64-g1|{\"calibration_target\":0.98,\"ef\":121,\"prefetch_lookahead\":3,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":121}",
    "config": "D03__svs-lvq4x8-pad64-g1",
    "dataset": "D03",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 121,
      "svs_capacity": 121,
      "prefetch_lookahead": 3,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=121; svs_capacity=121; prefetch_lookahead=3; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.980241935483871,
        "qps": 2563.864414464828,
        "batch": null,
        "samples": [
          2522.69353683461,
          2559.4700941226383,
          2601.337097769336,
          2554.61675344823,
          2581.204590149325
        ],
        "cv": 0.010326394723960622,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3080673217773438,
        "pss": 1.2978525161743164,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 117
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.980241935483871,
        "qps": 18442.271261917555,
        "batch": null,
        "samples": [
          18578.549786227548,
          18749.65087624687,
          17960.857166690927,
          17849.96452985485,
          19072.33395056758
        ],
        "cv": 0.025347500220934537,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3081550598144531,
        "pss": 1.2979402542114258,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 117
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.980241935483871,
        "qps": 31017.029375284492,
        "batch": null,
        "samples": [
          30926.320288447856,
          30210.582989020048,
          31484.004403630446,
          31861.645444780926,
          30602.593750543187
        ],
        "cv": 0.019137754407956797,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3086280822753906,
        "pss": 1.2984132766723633,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 117
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.980241935483871,
        "qps": 43351.12286840525,
        "batch": null,
        "samples": [
          43251.598685189085,
          42903.856104945226,
          43637.9460562149,
          43685.65406714023,
          43276.55942853679
        ],
        "cv": 0.006603023669647555,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3095054626464844,
        "pss": 1.299290657043457,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 117
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
    "id": "D03__svs-lvq4x8-pad64-g1|{\"ef\":32}",
    "config": "D03__svs-lvq4x8-pad64-g1",
    "dataset": "D03",
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
        "recall": 0.9317204301075269,
        "qps": 8745.385356682522,
        "batch": null,
        "samples": [
          8819.675570720277,
          8743.54179718958,
          8777.084675015512,
          8510.582513888377,
          8876.042226598862
        ],
        "cv": 0.014348568759916747,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3077507019042969,
        "pss": 1.2975664138793945,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 117
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__svs-lvq4x8-pad64-g1|{\"ef\":64}",
    "config": "D03__svs-lvq4x8-pad64-g1",
    "dataset": "D03",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9626344086021505,
        "qps": 4624.84179075357,
        "batch": null,
        "samples": [
          4543.5944618092035,
          4588.985218437031,
          4619.928977085794,
          4722.270442119708,
          4649.429854316115
        ],
        "cv": 0.012976253647241066,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3079376220703125,
        "pss": 1.2977228164672852,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 117
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__svs-lvq4x8-pad64-g1|{\"ef\":96}",
    "config": "D03__svs-lvq4x8-pad64-g1",
    "dataset": "D03",
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
        "recall": 0.9754032258064517,
        "qps": 3146.872832309565,
        "batch": null,
        "samples": [
          3158.9582727853663,
          3181.299113906978,
          3169.7326537919475,
          3120.8629402997835,
          3103.51118076375
        ],
        "cv": 0.009438043120058375,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3080101013183594,
        "pss": 1.297795295715332,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 117
        }
      }
    },
    "targets": []
  }
];
