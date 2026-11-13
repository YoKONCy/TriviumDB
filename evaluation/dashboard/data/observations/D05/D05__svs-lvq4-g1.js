// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D05__svs-lvq4-g1"] = [
  {
    "id": "D05__svs-lvq4-g1|{\"calibration_target\":0.95,\"ef\":405,\"prefetch_lookahead\":27,\"prefetch_step\":4,\"search_buffer_visited_set\":false,\"svs_capacity\":405}",
    "config": "D05__svs-lvq4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4",
    "family": "量化图",
    "parameters": {
      "ef": 405,
      "svs_capacity": 405,
      "prefetch_lookahead": 27,
      "prefetch_step": 4,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=405; svs_capacity=405; prefetch_lookahead=27; prefetch_step=4; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9519293924466338,
        "qps": 499.3112128336002,
        "batch": null,
        "samples": [
          503.0684806366519,
          503.42998695488666,
          498.81539517022304,
          492.98914218606194,
          498.25305922017753
        ],
        "cv": 0.007621831354700527,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.02337646484375,
        "pss": 1.0132780075073242,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 100
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9519293924466338,
        "qps": 3518.447977111485,
        "batch": null,
        "samples": [
          3442.6124326845757,
          3596.1050377670654,
          3517.9919664591753,
          3541.0837237997266,
          3494.4467248468836
        ],
        "cv": 0.01441976214931928,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0237045288085938,
        "pss": 1.013606071472168,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 100
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9519293924466338,
        "qps": 5767.342214893885,
        "batch": null,
        "samples": [
          5716.135490985035,
          5883.526425393374,
          5874.952076329941,
          5572.5768415442735,
          5789.5202402168015
        ],
        "cv": 0.019940041123492402,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0239601135253906,
        "pss": 1.013249397277832,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 101
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9519293924466338,
        "qps": 7127.015126087744,
        "batch": null,
        "samples": [
          7120.777138887744,
          7097.874650767257,
          7168.371929655799,
          7147.831724830425,
          7100.220186297495
        ],
        "cv": 0.0038424596700554706,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0270843505859375,
        "pss": 1.0163297653198242,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 101
        }
      }
    },
    "targets": [
      {
        "target": 0.95,
        "note": "独立调参集达标"
      },
      {
        "target": 0.98,
        "note": "独立调参集未达目标"
      }
    ]
  },
  {
    "id": "D05__svs-lvq4-g1|{\"ef\":64}",
    "config": "D05__svs-lvq4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9324096880131363,
        "qps": 2729.5030477060327,
        "batch": null,
        "samples": [
          2753.541257281951,
          2719.5827522133113,
          2707.1317677016423,
          2746.2551684063164,
          2721.004292926943
        ],
        "cv": 0.006407935438064469,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.0222129821777344,
        "pss": 1.0121355056762695,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 100
        }
      }
    },
    "targets": []
  }
];
