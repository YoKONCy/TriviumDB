// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__svs-lvq4x8-g0"] = [
  {
    "id": "D03__svs-lvq4x8-g0|{\"ef\":32}",
    "config": "D03__svs-lvq4x8-g0",
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
        "recall": 0.9706989247311828,
        "qps": 5149.3693644410905,
        "batch": null,
        "samples": [
          5077.9448880771015,
          5084.52999016426,
          5031.825252568777,
          5295.097584366666,
          5257.44910702865
        ],
        "cv": 0.020559410085525573,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3974990844726562,
        "pss": 1.3874750137329102,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 116
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9706989247311828,
        "qps": 37542.99235296091,
        "batch": null,
        "samples": [
          37373.46420114828,
          37116.837960097444,
          37614.48594213032,
          38704.96776004955,
          36905.20590137895
        ],
        "cv": 0.016729597495485197,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3977165222167969,
        "pss": 1.3876752853393555,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 116
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9706989247311828,
        "qps": 61886.093648515074,
        "batch": null,
        "samples": [
          61144.29213155535,
          63408.87676188455,
          62232.80358052593,
          62599.28206116649,
          60045.21370744306
        ],
        "cv": 0.018964062051186163,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3979873657226562,
        "pss": 1.3879461288452148,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 116
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9706989247311828,
        "qps": 82857.62367683815,
        "batch": null,
        "samples": [
          83680.72064681766,
          81769.36936932645,
          83004.73525836968,
          81920.30438416866,
          83912.98872550833
        ],
        "cv": 0.010625825592648921,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.3984756469726562,
        "pss": 1.3884344100952148,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 114
        },
        "evidence": {
          "$annotation": 116
        }
      }
    },
    "targets": [
      {
        "target": 0.95,
        "note": "独立调参集达标"
      }
    ]
  }
];
