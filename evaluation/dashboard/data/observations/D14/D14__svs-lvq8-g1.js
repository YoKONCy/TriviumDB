// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__svs-lvq8-g1"] = [
  {
    "id": "D14__svs-lvq8-g1|{\"calibration_target\":0.95,\"ef\":102,\"prefetch_lookahead\":31,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":102}",
    "config": "D14__svs-lvq8-g1",
    "dataset": "D14",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 102,
      "svs_capacity": 102,
      "prefetch_lookahead": 31,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=102; svs_capacity=102; prefetch_lookahead=31; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9424731182795699,
        "qps": 2405.5848315384883,
        "batch": null,
        "samples": [
          2389.7958241178926,
          2374.4789034917894,
          2386.5207079031743,
          2432.8807353123552,
          2444.2479868672294
        ],
        "cv": 0.011490486310668018,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.391788482666016,
        "pss": 4.3796539306640625,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9424731182795699,
        "qps": 16046.404203350741,
        "batch": null,
        "samples": [
          15707.017587870187,
          16459.725310387195,
          16333.830440500184,
          15896.863341803792,
          15834.584336192353
        ],
        "cv": 0.018399734834126272,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.3918914794921875,
        "pss": 4.379756927490234,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9424731182795699,
        "qps": 21182.208857983016,
        "batch": null,
        "samples": [
          21632.84560109697,
          21527.46995472041,
          21029.644417493095,
          20981.190781794125,
          20739.893534810482
        ],
        "cv": 0.01610119413416686,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392208099365234,
        "pss": 4.380073547363281,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9424731182795699,
        "qps": 21030.606029030358,
        "batch": null,
        "samples": [
          20892.53760569108,
          21058.23512855427,
          21005.7178790435,
          21107.95144938264,
          21088.58808248029
        ],
        "cv": 0.003670268193843718,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392902374267578,
        "pss": 4.380767822265625,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
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
    "id": "D14__svs-lvq8-g1|{\"calibration_target\":0.98,\"ef\":322,\"prefetch_lookahead\":25,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":322}",
    "config": "D14__svs-lvq8-g1",
    "dataset": "D14",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 322,
      "svs_capacity": 322,
      "prefetch_lookahead": 25,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=322; svs_capacity=322; prefetch_lookahead=25; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9731182795698925,
        "qps": 823.471984760465,
        "batch": null,
        "samples": [
          822.7582425334587,
          838.6983092480999,
          820.5954689519216,
          817.0475878299384,
          818.2603152389067
        ],
        "cv": 0.00954744003873374,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.3927459716796875,
        "pss": 4.380611419677734,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9731182795698925,
        "qps": 4391.84144044545,
        "batch": null,
        "samples": [
          4378.293586007495,
          4357.248665228655,
          4370.581845866839,
          4442.517288276987,
          4410.565816847271
        ],
        "cv": 0.007018633607251602,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.3928070068359375,
        "pss": 4.380672454833984,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9731182795698925,
        "qps": 6267.768947859189,
        "batch": null,
        "samples": [
          6214.951894789397,
          6233.104424307391,
          6366.816337379337,
          6333.50339645563,
          6190.468686364193
        ],
        "cv": 0.011076245007207207,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.39306640625,
        "pss": 4.380931854248047,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9731182795698925,
        "qps": 6701.785895277241,
        "batch": null,
        "samples": [
          6670.384692809456,
          6717.473445643381,
          6707.46628598887,
          6711.057902198494,
          6702.547149746004
        ],
        "cv": 0.0024526991545445226,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.394035339355469,
        "pss": 4.381900787353516,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 98
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
    "id": "D14__svs-lvq8-g1|{\"ef\":256}",
    "config": "D14__svs-lvq8-g1",
    "dataset": "D14",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 256
    },
    "parameterText": "ef=256",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9702956989247312,
        "qps": 846.4330896138863,
        "batch": null,
        "samples": [
          845.700156096898,
          852.364090255139,
          844.903995226092,
          835.1316723626239,
          854.0655341286788
        ],
        "cv": 0.007908130666890854,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.391731262207031,
        "pss": 4.379596710205078,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 98
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__svs-lvq8-g1|{\"ef\":64}",
    "config": "D14__svs-lvq8-g1",
    "dataset": "D14",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.923252688172043,
        "qps": 3045.673919337732,
        "batch": null,
        "samples": [
          3121.48968578348,
          3104.9609485576743,
          3001.567472332932,
          2979.714842133304,
          3020.63664788127
        ],
        "cv": 0.018680952839224014,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.39154052734375,
        "pss": 4.379437446594238,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 98
        }
      }
    },
    "targets": []
  }
];
