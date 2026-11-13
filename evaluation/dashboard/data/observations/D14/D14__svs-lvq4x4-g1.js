// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__svs-lvq4x4-g1"] = [
  {
    "id": "D14__svs-lvq4x4-g1|{\"calibration_target\":0.95,\"ef\":119,\"prefetch_lookahead\":5,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":119}",
    "config": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 119,
      "svs_capacity": 119,
      "prefetch_lookahead": 5,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=119; svs_capacity=119; prefetch_lookahead=5; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.951747311827957,
        "qps": 2470.748578696931,
        "batch": null,
        "samples": [
          2475.8524542961636,
          2425.0787252812365,
          2493.394100996253,
          2493.270359103811,
          2466.1472538071894
        ],
        "cv": 0.010163632351534937,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392307281494141,
        "pss": 4.382014274597168,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.951747311827957,
        "qps": 17496.07704806125,
        "batch": null,
        "samples": [
          17417.664497709782,
          17828.47630907522,
          17769.5726419234,
          17332.060714124258,
          17132.61107747358
        ],
        "cv": 0.01513161150776099,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392383575439453,
        "pss": 4.3820905685424805,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.951747311827957,
        "qps": 28836.72061508993,
        "batch": null,
        "samples": [
          28964.304587785948,
          29317.85813349444,
          28830.67640048824,
          28585.170282498177,
          28485.593671182844
        ],
        "cv": 0.010221194354047107,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392795562744141,
        "pss": 4.382502555847168,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.951747311827957,
        "qps": 34020.50769454264,
        "batch": null,
        "samples": [
          33953.31640852556,
          33818.71105602718,
          33703.13705443001,
          34389.32547983322,
          34238.04847389724
        ],
        "cv": 0.007543579551407731,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.393768310546875,
        "pss": 4.383475303649902,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
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
    "id": "D14__svs-lvq4x4-g1|{\"calibration_target\":0.98,\"ef\":316,\"prefetch_lookahead\":4,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":316}",
    "config": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 316,
      "svs_capacity": 316,
      "prefetch_lookahead": 4,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=316; svs_capacity=316; prefetch_lookahead=4; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9733870967741935,
        "qps": 939.1495429486891,
        "batch": null,
        "samples": [
          945.3461474769031,
          948.8589164554727,
          936.5735745313638,
          928.618275660078,
          936.350800619628
        ],
        "cv": 0.0076494254350529985,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.393596649169922,
        "pss": 4.383303642272949,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9733870967741935,
        "qps": 6702.926773318927,
        "batch": null,
        "samples": [
          6650.395904527477,
          6667.912085018061,
          6631.410294501818,
          6762.284882662538,
          6802.630699884741
        ],
        "cv": 0.010022181827053988,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.393672943115234,
        "pss": 4.383379936218262,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9733870967741935,
        "qps": 11138.28616268107,
        "batch": null,
        "samples": [
          11108.683996631804,
          11327.795212029512,
          11241.959014746713,
          11020.801196812914,
          10992.191393184403
        ],
        "cv": 0.011545386794927249,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.393840789794922,
        "pss": 4.383547782897949,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9733870967741935,
        "qps": 13298.573996985117,
        "batch": null,
        "samples": [
          13209.862828227639,
          13340.231709500815,
          13286.500706021257,
          13295.922092121567,
          13360.352649054306
        ],
        "cv": 0.003918057536328723,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.394378662109375,
        "pss": 4.384085655212402,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 95
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
    "id": "D14__svs-lvq4x4-g1|{\"ef\":256}",
    "config": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 256
    },
    "parameterText": "ef=256",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9700268817204301,
        "qps": 1165.200847438493,
        "batch": null,
        "samples": [
          1178.9838489715605,
          1165.3160935198296,
          1149.988527170939,
          1167.9929969070517,
          1163.7227706230838
        ],
        "cv": 0.007971575506129773,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392253875732422,
        "pss": 4.381960868835449,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 95
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__svs-lvq4x4-g1|{\"ef\":64}",
    "config": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9229838709677419,
        "qps": 4305.017345593839,
        "batch": null,
        "samples": [
          4342.249921548612,
          4336.358261470309,
          4343.764782132916,
          4249.182516661724,
          4253.531246155637
        ],
        "cv": 0.010198556180960558,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392063140869141,
        "pss": 4.381770133972168,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 95
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__svs-lvq4x4-g1|{\"ef\":96}",
    "config": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
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
        "recall": 0.9451612903225807,
        "qps": 3016.9625991321013,
        "batch": null,
        "samples": [
          2996.212174498706,
          3108.2672859883405,
          3052.093073291154,
          2985.238991398197,
          2943.0014704841087
        ],
        "cv": 0.01902813225566457,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 4.392192840576172,
        "pss": 4.381899833679199,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 95
        }
      }
    },
    "targets": []
  }
];
