// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__D14-diskann-pq-fp32-strong-r64-l400"] = [
  {
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400|{\"ef\":192}",
    "config": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 192
    },
    "parameterText": "ef=192",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9696236559139785,
        "qps": 258.22292104406165,
        "batch": null,
        "samples": [
          266.0350848814903,
          256.1974402267263,
          254.8064961914787,
          252.03556993852393,
          262.04001398208885
        ],
        "cv": 0.01972058433067081,
        "p50": 3.775,
        "p95": 4.877149999999999,
        "p99": 5.415959999999999,
        "p95ci": [
          4.823,
          4.938
        ],
        "p99ci": [
          5.243,
          5.49843
        ],
        "resident": 16.462432861328125,
        "pss": 16.460625648498535,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9696236559139785,
        "qps": 1306.903273151881,
        "batch": null,
        "samples": [
          1302.7637505321802,
          1282.5655668075003,
          1319.9338799938978,
          1323.0643909297874,
          1306.1887774960396
        ],
        "cv": 0.011040434950132,
        "p50": 5.978,
        "p95": 7.658049999999999,
        "p99": 8.213289999999999,
        "p95ci": [
          7.598,
          7.727
        ],
        "p99ci": [
          8.094,
          8.351959999999998
        ],
        "resident": 16.465591430664062,
        "pss": 16.46367835998535,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9696236559139785,
        "qps": 1309.1810622704925,
        "batch": null,
        "samples": [
          1294.684040208139,
          1301.152466221776,
          1320.3365810057362,
          1322.478431421573,
          1307.253792495238
        ],
        "cv": 0.008223945427640158,
        "p50": 10.9075,
        "p95": 14.355049999999999,
        "p99": 20.23036999999998,
        "p95ci": [
          14.147999999999994,
          14.637253749999992
        ],
        "p99ci": [
          18.048057999999987,
          23.45593999999999
        ],
        "resident": 16.468040466308594,
        "pss": 16.466208457946777,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9696236559139785,
        "qps": 1368.592099004794,
        "batch": null,
        "samples": [
          1366.7935726170708,
          1360.1762025197236,
          1368.952625167466,
          1376.4437761356853,
          1370.594318584024
        ],
        "cv": 0.003864308139313357,
        "p50": 22.1865,
        "p95": 28.656699999999997,
        "p99": 31.884529999999994,
        "p95ci": [
          28.203585,
          29.024
        ],
        "p99ci": [
          30.571489999999997,
          34.331859999999985
        ],
        "resident": 16.473388671875,
        "pss": 16.471607208251953,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
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
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400|{\"ef\":256}",
    "config": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 256
    },
    "parameterText": "ef=256",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9762096774193548,
        "qps": 202.06650430519304,
        "batch": null,
        "samples": [
          205.7574524899889,
          203.49929471507104,
          198.0577602570618,
          199.69904596848747,
          203.318968095356
        ],
        "cv": 0.013807165165259159,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.46247100830078,
        "pss": 16.460641860961914,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 78
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400|{\"ef\":384}",
    "config": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "parameters": {
      "ef": 384
    },
    "parameterText": "ef=384",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9817204301075269,
        "qps": 144.0208238125393,
        "batch": null,
        "samples": [
          147.4497090007457,
          141.098508468168,
          142.89651770724305,
          143.91929672510977,
          144.74008716142995
        ],
        "cv": 0.01459407844333887,
        "p50": 7.0135,
        "p95": 9.40705,
        "p99": 12.215479999999994,
        "p95ci": [
          9.281,
          9.51615
        ],
        "p99ci": [
          11.283,
          13.011250749999997
        ],
        "resident": 16.462509155273438,
        "pss": 16.46065044403076,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9817204301075269,
        "qps": 724.1391611402634,
        "batch": null,
        "samples": [
          732.903224846534,
          728.1370827889609,
          718.6319055298412,
          719.6037623982863,
          721.4198301376944
        ],
        "cv": 0.007591122848543395,
        "p50": 10.938,
        "p95": 14.502249999999998,
        "p99": 16.68729,
        "p95ci": [
          14.272,
          14.615005
        ],
        "p99ci": [
          16.097079999999995,
          17.395
        ],
        "resident": 16.465457916259766,
        "pss": 16.463574409484863,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9817204301075269,
        "qps": 720.1291480538334,
        "batch": null,
        "samples": [
          706.1035739850365,
          728.4988790466332,
          724.9091145489937,
          711.5205220256835,
          729.6136506628203
        ],
        "cv": 0.013227571344404911,
        "p50": 19.8995,
        "p95": 25.98605,
        "p99": 27.48048,
        "p95ci": [
          25.65655125,
          26.219661249999998
        ],
        "p99ci": [
          27.118629999999996,
          27.781
        ],
        "resident": 16.467655181884766,
        "pss": 16.465805053710938,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9817204301075269,
        "qps": 743.8587002732177,
        "batch": null,
        "samples": [
          747.4480821473786,
          740.0096405392488,
          741.9311475497644,
          742.2447363815281,
          747.6598947481682
        ],
        "cv": 0.004185560562528769,
        "p50": 40.684,
        "p95": 53.752549999999985,
        "p99": 58.56412999999998,
        "p95ci": [
          52.551,
          54.3383
        ],
        "p99ci": [
          56.247632999999986,
          65.6706599999998
        ],
        "resident": 16.47374725341797,
        "pss": 16.47188377380371,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 77
        },
        "evidence": {
          "$annotation": 78
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
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400|{\"ef\":64}",
    "config": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
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
        "recall": 0.9141129032258064,
        "qps": 651.2493741538253,
        "batch": null,
        "samples": [
          647.5423033720798,
          668.3022248585364,
          669.9678467842318,
          635.7258630500544,
          634.7086327042242
        ],
        "cv": 0.023483756863741125,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.462448120117188,
        "pss": 16.460575103759766,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 78
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400|{\"ef\":96}",
    "config": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
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
        "recall": 0.9426075268817204,
        "qps": 468.2834496317686,
        "batch": null,
        "samples": [
          475.8770953306847,
          466.5777456235195,
          463.99526877744717,
          464.31548840496544,
          470.6516500222261
        ],
        "cv": 0.00956506599480793,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 16.462310791015625,
        "pss": 16.46056842803955,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 79
        },
        "evidence": {
          "$annotation": 78
        }
      }
    },
    "targets": []
  }
];
