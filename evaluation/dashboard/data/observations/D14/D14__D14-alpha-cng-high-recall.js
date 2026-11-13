// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__D14-alpha-cng-high-recall"] = [
  {
    "id": "D14__D14-alpha-cng-high-recall|{\"ef\":1024}",
    "config": "D14__D14-alpha-cng-high-recall",
    "dataset": "D14",
    "method": "α-CNG",
    "family": "全精度图",
    "parameters": {
      "ef": 1024
    },
    "parameterText": "ef=1024",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9658602150537634,
        "qps": 193.58331734982488,
        "batch": null,
        "samples": [
          194.53434039628962,
          193.20549954177295,
          195.0109226516117,
          193.182895380608,
          191.9829287788421
        ],
        "cv": 0.005566940787116526,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.072490692138672,
        "pss": 15.06184196472168,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 108
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-alpha-cng-high-recall|{\"ef\":1536}",
    "config": "D14__D14-alpha-cng-high-recall",
    "dataset": "D14",
    "method": "α-CNG",
    "family": "全精度图",
    "parameters": {
      "ef": 1536
    },
    "parameterText": "ef=1536",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9744623655913979,
        "qps": 131.83892035215084,
        "batch": null,
        "samples": [
          131.68331446902644,
          130.78738581740592,
          130.28607121955258,
          131.35446444337353,
          135.0833658113957
        ],
        "cv": 0.012828499328846056,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 15.096916198730469,
        "pss": 15.086267471313477,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 108
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-alpha-cng-high-recall|{\"ef\":3072}",
    "config": "D14__D14-alpha-cng-high-recall",
    "dataset": "D14",
    "method": "α-CNG",
    "family": "全精度图",
    "parameters": {
      "ef": 3072
    },
    "parameterText": "ef=3072",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9830645161290322,
        "qps": 68.56609785590109,
        "batch": null,
        "samples": [
          68.61700536922729,
          68.38828537108431,
          68.80395352582326,
          68.10486041887698,
          68.91638459449364
        ],
        "cv": 0.004258189614024286,
        "p50": 14.688701000000002,
        "p95": 19.01977475,
        "p99": 21.37911464,
        "p95ci": [
          18.733866952499994,
          19.2810098325
        ],
        "p99ci": [
          20.774211849999997,
          22.439608
        ],
        "resident": 15.126605987548828,
        "pss": 15.115873336791992,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 108
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9830645161290322,
        "qps": 385.2014860524931,
        "batch": null,
        "samples": [
          388.52982208119687,
          389.9481877525313,
          384.21499654282906,
          376.6834398862953,
          386.6309839996132
        ],
        "cv": 0.012131207376408127,
        "p50": 20.7553665,
        "p95": 26.85840895,
        "p99": 29.14394198999999,
        "p95ci": [
          26.383366000000002,
          27.2696076
        ],
        "p99ci": [
          28.496686559999997,
          29.877238000000002
        ],
        "resident": 15.12656021118164,
        "pss": 15.115776062011719,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 108
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9830645161290322,
        "qps": 425.6420655805255,
        "batch": null,
        "samples": [
          426.07073240823235,
          428.2071146550733,
          427.2754729670154,
          425.2377217226141,
          421.41928614969237
        ],
        "cv": 0.005502455452994598,
        "p50": 37.021142,
        "p95": 48.11165455,
        "p99": 52.00276887,
        "p95ci": [
          47.219586639999996,
          48.81527144124998
        ],
        "p99ci": [
          50.057373,
          53.44753026
        ],
        "resident": 15.126655578613281,
        "pss": 15.11575698852539,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 108
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9830645161290322,
        "qps": 416.98989114663857,
        "batch": null,
        "samples": [
          417.3268829645506,
          416.52598617905085,
          417.1965352176071,
          417.3048736582319,
          416.5951777137524
        ],
        "cv": 0.0008488775941273269,
        "p50": 75.59384750000001,
        "p95": 98.41611514999995,
        "p99": 107.43993080999998,
        "p95ci": [
          96.85122504749997,
          99.90071703749999
        ],
        "p99ci": [
          104.03704145974999,
          110.40089283
        ],
        "resident": 15.126850128173828,
        "pss": 15.115951538085938,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 108
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
