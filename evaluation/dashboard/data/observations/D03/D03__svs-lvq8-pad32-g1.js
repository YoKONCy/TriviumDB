// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq8-pad32-g1"] = [
  {
    "id": "D03__svs-lvq8-pad32-g1|{\"calibration_target\":0.95,\"ef\":62,\"prefetch_lookahead\":2,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":62}",
    "config": "D03__svs-lvq8-pad32-g1",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 62,
      "svs_capacity": 62,
      "prefetch_lookahead": 2,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=62; svs_capacity=62; prefetch_lookahead=2; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9629032258064516,
        "qps": 3813.8540866505023,
        "batch": null,
        "samples": [
          3761.098158748963,
          3807.8197446066542,
          3778.9307859193746,
          3891.064128568397,
          3830.357615409122
        ],
        "cv": 0.011885007676410845,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9192390441894531,
        "pss": 0.9086952209472656,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 120
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9629032258064516,
        "qps": 25526.80421629296,
        "batch": null,
        "samples": [
          25294.2078537419,
          25424.63081533438,
          25567.42013397077,
          25600.543958509323,
          25747.21831990842
        ],
        "cv": 0.006074197956489351,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9193305969238281,
        "pss": 0.9087867736816406,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 120
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9629032258064516,
        "qps": 39183.25313968113,
        "batch": null,
        "samples": [
          40399.88819004351,
          39218.09860746812,
          39256.87352410848,
          38711.38015800307,
          38330.02521878248
        ],
        "cv": 0.017819132356198863,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9197120666503906,
        "pss": 0.9091682434082031,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 120
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9629032258064516,
        "qps": 45320.10644813421,
        "batch": null,
        "samples": [
          45330.2845610524,
          45054.49314476695,
          45000.346695747794,
          45930.056403172246,
          45285.351435931676
        ],
        "cv": 0.007292541744327878,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9202880859375,
        "pss": 0.9097442626953125,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 120
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
    "id": "D03__svs-lvq8-pad32-g1|{\"ef\":128}",
    "config": "D03__svs-lvq8-pad32-g1",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 128
    },
    "parameterText": "ef=128",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9790322580645161,
        "qps": 1945.948422185542,
        "batch": null,
        "samples": [
          1962.9948212311,
          1981.7296267716918,
          1919.68050119147,
          1906.1144807935373,
          1959.2226809399117
        ],
        "cv": 0.014578180184240163,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9191818237304688,
        "pss": 0.9086380004882812,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 120
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__svs-lvq8-pad32-g1|{\"ef\":32}",
    "config": "D03__svs-lvq8-pad32-g1",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9216397849462366,
        "qps": 6438.985170342801,
        "batch": null,
        "samples": [
          6604.018760041086,
          6474.724502588813,
          6387.8004502948625,
          6340.603012697471,
          6387.779126091772
        ],
        "cv": 0.014474858656704098,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.9190444946289062,
        "pss": 0.9085273742675781,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 118
        },
        "evidence": {
          "$annotation": 120
        }
      }
    },
    "targets": []
  }
];
