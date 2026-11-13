// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__usearch-g1"] = [
  {
    "id": "D03__usearch-g1|{\"ef\":128}",
    "config": "D03__usearch-g1",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "parameters": {
      "ef": 128
    },
    "parameterText": "ef=128",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9704301075268817,
        "qps": 629.0434606768451,
        "batch": null,
        "samples": [
          614.8333306102634,
          613.2675287425473,
          644.9591149047234,
          643.1591711772595,
          628.9981579494321
        ],
        "cv": 0.02136817681443827,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1701278686523438,
        "pss": 3.1597900390625,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 121
        },
        "evidence": {
          "$annotation": 125
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__usearch-g1|{\"ef\":192}",
    "config": "D03__usearch-g1",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "parameters": {
      "ef": 192
    },
    "parameterText": "ef=192",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9778225806451613,
        "qps": 440.6622985404409,
        "batch": null,
        "samples": [
          445.5588375993479,
          445.5926926872296,
          437.3206231428444,
          436.2766082003368,
          438.56273107244573
        ],
        "cv": 0.00925111965880511,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.170177459716797,
        "pss": 3.159839630126953,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 121
        },
        "evidence": {
          "$annotation": 125
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__usearch-g1|{\"ef\":64}",
    "config": "D03__usearch-g1",
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
        "recall": 0.947983870967742,
        "qps": 1150.8905472738877,
        "batch": null,
        "samples": [
          1156.6647368892616,
          1149.9372930726863,
          1155.30945755605,
          1132.7722476478925,
          1159.7690012035487
        ],
        "cv": 0.008342058568425416,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 3.1695556640625,
        "pss": 3.1593894958496094,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 121
        },
        "evidence": {
          "$annotation": 125
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__usearch-g1|{\"ef\":96}",
    "config": "D03__usearch-g1",
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
        "recall": 0.9637096774193549,
        "qps": 816.3618477048706,
        "batch": null,
        "samples": [
          797.1243430366989,
          832.6382327946943,
          831.8001832129681,
          816.3084615312615,
          803.9380179487298
        ],
        "cv": 0.017561646986738844,
        "p50": 1.1901970000000002,
        "p95": 1.5643863999999998,
        "p99": 1.6860894199999998,
        "p95ci": [
          1.5363316387499997,
          1.5956831999999999
        ],
        "p99ci": [
          1.6526907999999998,
          1.8098994199999998
        ],
        "resident": 3.1698875427246094,
        "pss": 3.1597213745117188,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 125
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9637096774193549,
        "qps": 4904.6952079804705,
        "batch": null,
        "samples": [
          4726.743315863996,
          4912.066500815328,
          4952.5875167798895,
          4988.163161993415,
          4943.915544449722
        ],
        "cv": 0.018801811739392944,
        "p50": 1.6529605,
        "p95": 2.1807197,
        "p99": 2.981768029999999,
        "p95ci": [
          2.1415996937499995,
          2.225349
        ],
        "p99ci": [
          2.6143634324999994,
          3.130042
        ],
        "resident": 3.1700515747070312,
        "pss": 3.1598854064941406,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 125
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9637096774193549,
        "qps": 7455.74376420954,
        "batch": null,
        "samples": [
          7433.490186061936,
          7359.355256442553,
          7557.4761130489505,
          7534.752577703465,
          7393.6446877907965
        ],
        "cv": 0.010429721819019763,
        "p50": 2.0899859999999997,
        "p95": 2.66033455,
        "p99": 2.8915188299999994,
        "p95ci": [
          2.6188992024999997,
          2.6990905887499994
        ],
        "p99ci": [
          2.79624225275,
          3.166866169999998
        ],
        "resident": 3.154102325439453,
        "pss": 3.1432743072509766,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 126
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9637096774193549,
        "qps": 8148.943849306655,
        "batch": null,
        "samples": [
          8253.70175590171,
          8258.311232902513,
          8136.8163603059975,
          8064.097967341248,
          8031.791930081805
        ],
        "cv": 0.01151248831782418,
        "p50": 3.9141324999999996,
        "p95": 5.008674599999999,
        "p99": 5.794005769999997,
        "p95ci": [
          4.920291518749999,
          5.099157893749999
        ],
        "p99ci": [
          5.409579,
          6.084756459999994
        ],
        "resident": 3.1545028686523438,
        "pss": 3.143674850463867,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 123
        },
        "evidence": {
          "$annotation": 126
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
