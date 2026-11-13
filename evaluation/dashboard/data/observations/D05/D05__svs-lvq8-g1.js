// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D05__svs-lvq8-g1"] = [
  {
    "id": "D05__svs-lvq8-g1|{\"calibration_target\":0.95,\"ef\":47,\"prefetch_lookahead\":1,\"prefetch_step\":2,\"search_buffer_visited_set\":false,\"svs_capacity\":47}",
    "config": "D05__svs-lvq8-g1",
    "dataset": "D05",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 47,
      "svs_capacity": 47,
      "prefetch_lookahead": 1,
      "prefetch_step": 2,
      "search_buffer_visited_set": false,
      "calibration_target": 0.95
    },
    "parameterText": "ef=47; svs_capacity=47; prefetch_lookahead=1; prefetch_step=2; search_buffer_visited_set=False; calibration_target=0.95",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9555418719211822,
        "qps": 2692.2459102629396,
        "batch": null,
        "samples": [
          2605.496419267561,
          2680.2167773044616,
          2627.9692044399953,
          2722.161426361817,
          2825.3857239408635
        ],
        "cv": 0.028973434949168007,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7325286865234375,
        "pss": 1.7222099304199219,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9555418719211822,
        "qps": 16502.34894025347,
        "batch": null,
        "samples": [
          16656.69890676451,
          16368.171846728215,
          16589.483247428732,
          16801.945491772254,
          16095.44520857363
        ],
        "cv": 0.014960291073366442,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7326011657714844,
        "pss": 1.7222824096679688,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9555418719211822,
        "qps": 20867.60834667426,
        "batch": null,
        "samples": [
          21218.38141103214,
          20582.892105720173,
          21032.97035064669,
          20927.143793314943,
          20576.654072657348
        ],
        "cv": 0.012116541602137565,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7334938049316406,
        "pss": 1.723175048828125,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9555418719211822,
        "qps": 21546.745020534094,
        "batch": null,
        "samples": [
          21707.093709357665,
          21565.591204164462,
          21709.040188370014,
          21632.233176929876,
          21119.766823848448
        ],
        "cv": 0.010210890667518216,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7339515686035156,
        "pss": 1.7236328125,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
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
    "id": "D05__svs-lvq8-g1|{\"calibration_target\":0.98,\"ef\":131,\"prefetch_lookahead\":1,\"prefetch_step\":1,\"search_buffer_visited_set\":false,\"svs_capacity\":131}",
    "config": "D05__svs-lvq8-g1",
    "dataset": "D05",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 131,
      "svs_capacity": 131,
      "prefetch_lookahead": 1,
      "prefetch_step": 1,
      "search_buffer_visited_set": false,
      "calibration_target": 0.98
    },
    "parameterText": "ef=131; svs_capacity=131; prefetch_lookahead=1; prefetch_step=1; search_buffer_visited_set=False; calibration_target=0.98",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9846982758620689,
        "qps": 1072.6457747195782,
        "batch": null,
        "samples": [
          1074.8510067009247,
          1070.2710071780914,
          1076.9602090593007,
          1087.0800137346287,
          1054.066636924945
        ],
        "cv": 0.010063800036284196,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7337379455566406,
        "pss": 1.723419189453125,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9846982758620689,
        "qps": 6957.8865861590775,
        "batch": null,
        "samples": [
          7110.352965555758,
          6828.147464391947,
          6806.422899751239,
          7079.114794669722,
          6965.394806426722
        ],
        "cv": 0.0179246080545986,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7338066101074219,
        "pss": 1.7234878540039062,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9846982758620689,
        "qps": 8813.038676827066,
        "batch": null,
        "samples": [
          8752.946899476065,
          8734.577637209051,
          9000.801276893611,
          8883.152900284165,
          8693.714670272433
        ],
        "cv": 0.012855122824046328,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7341346740722656,
        "pss": 1.72381591796875,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9846982758620689,
        "qps": 8985.037256161362,
        "batch": null,
        "samples": [
          9028.559406864491,
          9015.128605888302,
          8953.841701756679,
          8897.154596634422,
          9030.501969662919
        ],
        "cv": 0.005796644998153504,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7348136901855469,
        "pss": 1.7244949340820312,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 99
        },
        "evidence": {
          "$annotation": 105
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
    "id": "D05__svs-lvq8-g1|{\"ef\":32}",
    "config": "D05__svs-lvq8-g1",
    "dataset": "D05",
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
        "recall": 0.9314244663382595,
        "qps": 3818.2471305608788,
        "batch": null,
        "samples": [
          3793.6063380164946,
          3849.917332780077,
          3830.2976824259727,
          3821.219247496084,
          3796.195052085765
        ],
        "cv": 0.005556378279484151,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7313156127929688,
        "pss": 1.720865249633789,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 105
        }
      }
    },
    "targets": []
  },
  {
    "id": "D05__svs-lvq8-g1|{\"ef\":96}",
    "config": "D05__svs-lvq8-g1",
    "dataset": "D05",
    "method": "SVS LVQ8",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "SDK批量",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9796079638752052,
        "qps": 1420.069175744279,
        "batch": null,
        "samples": [
          1448.3835151864198,
          1419.1347279770396,
          1411.1473317624943,
          1418.242433608389,
          1403.437870187052
        ],
        "cv": 0.010735676576082707,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.7324905395507812,
        "pss": 1.7220096588134766,
        "queries": 9744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 102
        },
        "evidence": {
          "$annotation": 105
        }
      }
    },
    "targets": []
  }
];
