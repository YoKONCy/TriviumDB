// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__mirage-anns-g1"] = [
  {
    "id": "D14__mirage-anns-g1|{\"ef\":192}",
    "config": "D14__mirage-anns-g1",
    "dataset": "D14",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 192
    },
    "parameterText": "ef=192",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9768817204301076,
        "qps": 592.319339657652,
        "batch": null,
        "samples": [
          595.8075560749551,
          589.7012759919717,
          586.7189102136657,
          594.2333569595339,
          595.1355990481334
        ],
        "cv": 0.0059441550199640664,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.768020629882812,
        "pss": 15.757878303527832,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 88
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__mirage-anns-g1|{\"ef\":256}",
    "config": "D14__mirage-anns-g1",
    "dataset": "D14",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 256
    },
    "parameterText": "ef=256",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9830645161290322,
        "qps": 448.99854467691165,
        "batch": null,
        "samples": [
          446.1210111000323,
          448.88542623105803,
          452.3325660458647,
          457.67945525166743,
          439.9742647559358
        ],
        "cv": 0.013221435160227026,
        "p50": 2.442212,
        "p95": 2.8954336,
        "p99": 3.06599381,
        "p95ci": [
          2.8714526112499996,
          2.9207827
        ],
        "p99ci": [
          3.019545,
          3.184031
        ],
        "resident": 15.768058776855469,
        "pss": 15.757916450500488,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9830645161290322,
        "qps": 1964.0260166913706,
        "batch": null,
        "samples": [
          1959.5841396944345,
          1961.5261371870993,
          1983.4460201769514,
          1974.2671441257107,
          1941.3066422726567
        ],
        "cv": 0.0072870730435241685,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.767974853515625,
        "pss": 15.757832527160645,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9830645161290322,
        "qps": 2149.763761063255,
        "batch": null,
        "samples": [
          2135.6443217359633,
          2136.491507930691,
          2151.775958236772,
          2166.1889238336676,
          2158.7180935791816
        ],
        "cv": 0.005618814856204114,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.768070220947266,
        "pss": 15.757927894592285,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9830645161290322,
        "qps": 2129.6531820635037,
        "batch": null,
        "samples": [
          2120.7911966865345,
          2133.2793899254693,
          2136.7476658076034,
          2128.511842024119,
          2128.935815873792
        ],
        "cv": 0.0025184631106603683,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.768234252929688,
        "pss": 15.758091926574707,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
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
    "id": "D14__mirage-anns-g1|{\"ef\":64}",
    "config": "D14__mirage-anns-g1",
    "dataset": "D14",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 64
    },
    "parameterText": "ef=64",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9387096774193548,
        "qps": 1547.7563003443,
        "batch": null,
        "samples": [
          1567.5543889532396,
          1549.8231543347638,
          1538.787811323518,
          1517.1002095316042,
          1565.5159375783737
        ],
        "cv": 0.01202458616288596,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.767585754394531,
        "pss": 15.757447242736816,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 88
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__mirage-anns-g1|{\"ef\":96}",
    "config": "D14__mirage-anns-g1",
    "dataset": "D14",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "parameters": {
      "ef": 96
    },
    "parameterText": "ef=96",
    "domain": "作者接口",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9567204301075268,
        "qps": 1116.0582298205534,
        "batch": null,
        "samples": [
          1125.003139428625,
          1106.854654466295,
          1104.9868418547587,
          1116.7663243865354,
          1126.6801889665528
        ],
        "cv": 0.008020373507989745,
        "p50": 1.1109365,
        "p95": 1.34431,
        "p99": 1.5015018999999994,
        "p95ci": [
          1.3179699400000002,
          1.36417913625
        ],
        "p99ci": [
          1.442375,
          1.73137336975
        ],
        "resident": 15.767799377441406,
        "pss": 15.757657051086426,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9567204301075268,
        "qps": 4804.054965803471,
        "batch": null,
        "samples": [
          4822.345216385809,
          4831.168319050939,
          4799.868647682458,
          4794.422967021994,
          4772.469678876151
        ],
        "cv": 0.004344060895408491,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.767711639404297,
        "pss": 15.757569313049316,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9567204301075268,
        "qps": 5220.048614874962,
        "batch": null,
        "samples": [
          5239.446787767033,
          5189.312082383258,
          5194.132198516866,
          5229.80038875027,
          5247.551616957385
        ],
        "cv": 0.004568990352232818,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.767776489257812,
        "pss": 15.757634162902832,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9567204301075268,
        "qps": 5093.969106672526,
        "batch": null,
        "samples": [
          5097.026430671123,
          5091.35308548895,
          5093.136897707351,
          5092.5025606328245,
          5095.8265588623835
        ],
        "cv": 0.0004162918780379845,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.767967224121094,
        "pss": 15.757824897766113,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 90
        },
        "evidence": {
          "$annotation": 88
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
