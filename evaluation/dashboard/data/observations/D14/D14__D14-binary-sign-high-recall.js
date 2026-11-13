// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__D14-binary-sign-high-recall"] = [
  {
    "id": "D14__D14-binary-sign-high-recall|{\"candidates\":1024,\"ef\":2048}",
    "config": "D14__D14-binary-sign-high-recall",
    "dataset": "D14",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 2048,
      "candidates": 1024
    },
    "parameterText": "ef=2048; candidates=1024",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9813172043010753,
        "qps": 152.22362142445638,
        "batch": null,
        "samples": [
          150.39337972906375,
          149.9044870801978,
          151.34574665846387,
          155.59131511308019,
          153.88317854147627
        ],
        "cv": 0.014270418606465066,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 14.432994842529297,
        "pss": 14.430597305297852,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 82
        },
        "evidence": {
          "$annotation": 105
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-binary-sign-high-recall|{\"candidates\":2048,\"ef\":4096}",
    "config": "D14__D14-binary-sign-high-recall",
    "dataset": "D14",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "parameters": {
      "ef": 4096,
      "candidates": 2048
    },
    "parameterText": "ef=4096; candidates=2048",
    "domain": "逐请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9893817204301075,
        "qps": 70.09426015734299,
        "batch": null,
        "samples": [
          69.95235178860213,
          69.61269490747942,
          69.3577474778321,
          70.99524010771434,
          70.55326650508697
        ],
        "cv": 0.00860010029340075,
        "p50": 14.3822675,
        "p95": 18.132339749999996,
        "p99": 22.87265353999997,
        "p95ci": [
          17.6579812275,
          18.47139614999999
        ],
        "p99ci": [
          21.312908329999996,
          23.754009559999993
        ],
        "resident": 15.144054412841797,
        "pss": 15.141656875610352,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9893817204301075,
        "qps": 479.4626534502045,
        "batch": null,
        "samples": [
          488.64494747027123,
          474.141559274777,
          488.45870964665477,
          468.24614861078686,
          477.82190224853275
        ],
        "cv": 0.01673895586430277,
        "p50": 16.205464,
        "p95": 18.869230399999996,
        "p99": 22.640483229999997,
        "p95ci": [
          18.593787,
          19.173262648749994
        ],
        "p99ci": [
          22.1043637845,
          23.250963
        ],
        "resident": 15.166786193847656,
        "pss": 15.164388656616211,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9893817204301075,
        "qps": 773.5584560823322,
        "batch": null,
        "samples": [
          772.2676493816963,
          773.8223430090394,
          776.4603343286549,
          766.0288052369912,
          779.2131484552792
        ],
        "cv": 0.00574727066726662,
        "p50": 19.745904000000003,
        "p95": 22.344697699999998,
        "p99": 24.353710079999995,
        "p95ci": [
          22.18099665,
          22.509496099999993
        ],
        "p99ci": [
          24.039875418749993,
          24.86325053
        ],
        "resident": 15.182689666748047,
        "pss": 15.180292129516602,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
        },
        "evidence": {
          "$annotation": 105
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9893817204301075,
        "qps": 1223.4847187156467,
        "batch": null,
        "samples": [
          1208.5711994877493,
          1208.7016135607792,
          1232.1172374722273,
          1249.1091633532053,
          1218.924379704273
        ],
        "cv": 0.012624531784264259,
        "p50": 27.0759525,
        "p95": 30.715820049999998,
        "p99": 32.77187031,
        "p95ci": [
          30.44629545,
          30.964989152500003
        ],
        "p99ci": [
          32.550743,
          33.418887999999995
        ],
        "resident": 15.253398895263672,
        "pss": 15.251001358032227,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 57
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
      },
      {
        "target": 0.98,
        "note": "独立调参集达标"
      }
    ]
  }
];
