// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__diskann-pq-fp32-g0"] = [
  {
    "id": "D03__diskann-pq-fp32-g0|{\"ef\":32}",
    "config": "D03__diskann-pq-fp32-g0",
    "dataset": "D03",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9541666666666667,
        "qps": 1655.37371045498,
        "batch": null,
        "samples": [
          1634.5870339591188,
          1638.8226476946013,
          1645.9060484834858,
          1665.9848062174478,
          1691.5680159202468
        ],
        "cv": 0.01272411106195405,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.3008460998535156,
        "pss": 3.2990617752075195,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 103
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__diskann-pq-fp32-g0|{\"ef\":64}",
    "config": "D03__diskann-pq-fp32-g0",
    "dataset": "D03",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9763440860215054,
        "qps": 971.0488797009568,
        "batch": null,
        "samples": [
          991.1340384503211,
          952.1968662106325,
          968.3528632882019,
          983.0638566667709,
          960.496773888858
        ],
        "cv": 0.014721569713459993,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.3008270263671875,
        "pss": 3.298924446105957,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 92
        },
        "evidence": {
          "$annotation": 103
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__diskann-pq-fp32-g0|{\"ef\":96}",
    "config": "D03__diskann-pq-fp32-g0",
    "dataset": "D03",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9861559139784947,
        "qps": 683.485156903035,
        "batch": null,
        "samples": [
          687.7476862282076,
          675.9657872360376,
          678.1182833267003,
          688.4121693258825,
          687.1818583983471
        ],
        "cv": 0.007782053920061646,
        "p50": 1.423,
        "p95": 2.002,
        "p99": 2.31324,
        "p95ci": [
          1.9721499999999992,
          2.036
        ],
        "p99ci": [
          2.23042175,
          2.39
        ],
        "resident": 3.30078125,
        "pss": 3.2989158630371094,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9861559139784947,
        "qps": 3456.0032029911413,
        "batch": null,
        "samples": [
          3541.906743161302,
          3504.14340751168,
          3399.861669658174,
          3411.202400644454,
          3422.901793980096
        ],
        "cv": 0.01634341373689523,
        "p50": 2.245,
        "p95": 3.113149999999999,
        "p99": 3.463,
        "p95ci": [
          3.058122499999999,
          3.164
        ],
        "p99ci": [
          3.38362,
          3.6018199999999987
        ],
        "resident": 3.3025169372558594,
        "pss": 3.3006296157836914,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9861559139784947,
        "qps": 3579.8919819870375,
        "batch": null,
        "samples": [
          3600.872847861292,
          3552.436977098128,
          3557.9038195435105,
          3561.725265138421,
          3626.521000293835
        ],
        "cv": 0.008078375157091134,
        "p50": 4.009,
        "p95": 5.5291,
        "p99": 6.70339,
        "p95ci": [
          5.39915,
          5.666
        ],
        "p99ci": [
          6.081959999999999,
          7.168
        ],
        "resident": 3.3041305541992188,
        "pss": 3.302356719970703,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 103
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9861559139784947,
        "qps": 3772.0347796123087,
        "batch": null,
        "samples": [
          3764.230234418319,
          3795.5219974692677,
          3780.409536924503,
          3746.212426291854,
          3773.7997029576004
        ],
        "cv": 0.004360385912502405,
        "p50": 7.9985,
        "p95": 11.225249999999999,
        "p99": 12.854119999999996,
        "p95ci": [
          11.0552,
          11.395
        ],
        "p99ci": [
          12.17629,
          13.646539999999998
        ],
        "resident": 3.3077659606933594,
        "pss": 3.305936813354492,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
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
  }
];
