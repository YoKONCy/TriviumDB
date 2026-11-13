// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__usearch-g0"] = [
  {
    "id": "D03__usearch-g0|{\"ef\":32}",
    "config": "D03__usearch-g0",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "parameters": {
      "ef": 32
    },
    "parameterText": "ef=32",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9461021505376344,
        "qps": 1221.3335820639559,
        "batch": null,
        "samples": [
          1242.418050015515,
          1219.7955263833387,
          1214.2703336230659,
          1237.5882838226223,
          1192.5957164752379
        ],
        "cv": 0.014586601845496355,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.27093505859375,
        "pss": 3.260824203491211,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 121
        },
        "evidence": {
          "$annotation": 122
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__usearch-g0|{\"ef\":64}",
    "config": "D03__usearch-g0",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9758064516129032,
        "qps": 694.1687877502474,
        "batch": null,
        "samples": [
          693.3345184551521,
          707.2150096658394,
          706.4171903498045,
          685.483829911313,
          678.3933903691276
        ],
        "cv": 0.016364476642676398,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.27099609375,
        "pss": 3.260885238647461,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 121
        },
        "evidence": {
          "$annotation": 122
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__usearch-g0|{\"ef\":96}",
    "config": "D03__usearch-g0",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9858870967741935,
        "qps": 493.63820207412857,
        "batch": null,
        "samples": [
          485.0130318038666,
          501.9677894517911,
          496.03414742780865,
          490.07842910574414,
          495.09761258143226
        ],
        "cv": 0.011613117293699435,
        "p50": 1.950577,
        "p95": 2.7271549,
        "p99": 2.9403420700000003,
        "p95ci": [
          2.6722770000000002,
          2.7661770000000003
        ],
        "p99ci": [
          2.8898103625,
          2.9837969199999996
        ],
        "resident": 3.27117919921875,
        "pss": 3.261068344116211,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 122
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9858870967741935,
        "qps": 3138.6212034361356,
        "batch": null,
        "samples": [
          3166.3677514284573,
          3123.598034396752,
          3103.670063719449,
          3148.19968440444,
          3151.270483231579
        ],
        "cv": 0.0070799121322431885,
        "p50": 2.506277,
        "p95": 3.4071869,
        "p99": 3.65912455,
        "p95ci": [
          3.34571043625,
          3.4502167749999986
        ],
        "p99ci": [
          3.56051127,
          3.76100606
        ],
        "resident": 3.287567138671875,
        "pss": 3.276923179626465,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 124
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9858870967741935,
        "qps": 4390.135673705721,
        "batch": null,
        "samples": [
          4366.040399491616,
          4333.054866916996,
          4332.251653721195,
          4388.271678432602,
          4531.059769966196
        ],
        "cv": 0.016754838428309517,
        "p50": 3.4574175,
        "p95": 4.695684,
        "p99": 5.058923439999999,
        "p95ci": [
          4.6323052449999995,
          4.748074699999999
        ],
        "p99ci": [
          4.88632084375,
          5.18981146575
        ],
        "resident": 3.2876129150390625,
        "pss": 3.2769689559936523,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 124
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9858870967741935,
        "qps": 4849.753909727261,
        "batch": null,
        "samples": [
          4818.749946159433,
          4798.732344600811,
          4898.788004251226,
          4896.254247829524,
          4836.24500579531
        ],
        "cv": 0.008407924814645355,
        "p50": 6.4191355,
        "p95": 8.697731149999997,
        "p99": 9.50616114,
        "p95ci": [
          8.577457649999998,
          8.816227000000001
        ],
        "p99ci": [
          9.194647145000001,
          9.65075994
        ],
        "resident": 3.287731170654297,
        "pss": 3.2770872116088867,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 124
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
