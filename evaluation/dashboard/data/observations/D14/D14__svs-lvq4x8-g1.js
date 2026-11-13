// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__svs-lvq4x8-g1"] = [
  {
    "id": "D14__svs-lvq4x8-g1|{\"calibration_target\":0.95,\"ef\":103,\"prefetch_lookahead\":5,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":103}",
    "config": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 103,
      "svs_capacity": 103,
      "prefetch_lookahead": 5,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=103; svs_capacity=103; prefetch_lookahead=5; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.953494623655914,
        "qps": 2879.2780119635804,
        "batch": null,
        "samples": [
          2855.739191213499,
          2916.1536478991193,
          2909.7162358520104,
          2869.398926550216,
          2845.382058303058
        ],
        "cv": 0.009929530773093352,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.179656982421875,
        "pss": 6.169040679931641,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.953494623655914,
        "qps": 20288.93802423372,
        "batch": null,
        "samples": [
          20758.822720304946,
          20180.366308732617,
          20223.444405306727,
          20106.856335000863,
          20175.200351823445
        ],
        "cv": 0.011725120911796689,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.179737091064453,
        "pss": 6.169120788574219,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.953494623655914,
        "qps": 33835.85953563292,
        "batch": null,
        "samples": [
          33531.21815643772,
          34303.252313921155,
          34086.50446680104,
          33656.210910273105,
          33602.111830731556
        ],
        "cv": 0.008974014706835214,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.180095672607422,
        "pss": 6.1694793701171875,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.953494623655914,
        "qps": 41571.83387720813,
        "batch": null,
        "samples": [
          42469.049648946486,
          41859.086229525376,
          41433.008879407236,
          40934.211694115875,
          41163.81293404571
        ],
        "cv": 0.013086426756329535,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.180789947509766,
        "pss": 6.169820785522461,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
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
    "id": "D14__svs-lvq4x8-g1|{\"calibration_target\":0.98,\"ef\":237,\"prefetch_lookahead\":9,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":237}",
    "config": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 237,
      "svs_capacity": 237,
      "prefetch_lookahead": 9,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=237; svs_capacity=237; prefetch_lookahead=9; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9774193548387097,
        "qps": 1262.9782540746398,
        "batch": null,
        "samples": [
          1245.8408673835347,
          1273.7742339852766,
          1274.9204039481515,
          1267.8518402475197,
          1252.5039248087164
        ],
        "cv": 0.009276474331337058,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.180606842041016,
        "pss": 6.169990539550781,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9774193548387097,
        "qps": 7083.534633210533,
        "batch": null,
        "samples": [
          7279.111415934313,
          6956.121697381933,
          7063.080755106702,
          7061.599871767552,
          7057.759425862164
        ],
        "cv": 0.014946715692485363,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.180667877197266,
        "pss": 6.170051574707031,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9774193548387097,
        "qps": 12066.396411479669,
        "batch": null,
        "samples": [
          12124.98843352335,
          12092.501732453378,
          12087.532361604606,
          12053.871919540155,
          11973.08761027686
        ],
        "cv": 0.004294263369904035,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.180927276611328,
        "pss": 6.170310974121094,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9774193548387097,
        "qps": 18017.207883299274,
        "batch": null,
        "samples": [
          17848.874425728027,
          18129.470403753417,
          18138.481747943482,
          17805.6011503085,
          18163.61168876295
        ],
        "cv": 0.008664715001604656,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.18182373046875,
        "pss": 6.171207427978516,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 94
        },
        "evidence": {
          "$annotation": 97
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
    "id": "D14__svs-lvq4x8-g1|{\"ef\":192}",
    "config": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "parameters": {
      "ef": 192
    },
    "parameterText": "ef=192",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9716397849462366,
        "qps": 1556.4383238760256,
        "batch": null,
        "samples": [
          1561.0377345203144,
          1558.5795220901064,
          1528.7106762651392,
          1572.455377036427,
          1561.4083094681405
        ],
        "cv": 0.009424331792334022,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.179599761962891,
        "pss": 6.168983459472656,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 97
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__svs-lvq4x8-g1|{\"ef\":64}",
    "config": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
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
        "recall": 0.9262096774193549,
        "qps": 4347.477977258687,
        "batch": null,
        "samples": [
          4391.5518705168415,
          4371.665092039748,
          4288.923377617506,
          4366.6800013961765,
          4318.569544723162
        ],
        "cv": 0.008704658466187497,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.179409027099609,
        "pss": 6.1688232421875,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 97
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__svs-lvq4x8-g1|{\"ef\":96}",
    "config": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
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
        "recall": 0.9509408602150538,
        "qps": 3047.4515138256106,
        "batch": null,
        "samples": [
          3064.7883458776623,
          3081.968473449411,
          3021.588706965463,
          2987.866839110953,
          3081.0452037245627
        ],
        "cv": 0.012136627015336564,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 6.179542541503906,
        "pss": 6.168926239013672,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 97
        }
      }
    },
    "targets": []
  }
];
