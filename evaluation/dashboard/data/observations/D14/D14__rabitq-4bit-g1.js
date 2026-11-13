// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D14__rabitq-4bit-g1"] = [
  {
    "id": "D14__rabitq-4bit-g1|{\"nprobe\":128}",
    "config": "D14__rabitq-4bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "parameters": {
      "nprobe": 128
    },
    "parameterText": "nprobe=128",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9400537634408602,
        "qps": 142.88869651878227,
        "batch": null,
        "samples": [
          139.59701100817705,
          145.23479865580785,
          145.2995593941512,
          142.48279448806554,
          141.82931904770965
        ],
        "cv": 0.015152481079805031,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.9678726196289062,
        "pss": 1.9570398330688477,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 92
        }
      }
    },
    "targets": []
  },
  {
    "id": "D14__rabitq-4bit-g1|{\"nprobe\":256}",
    "config": "D14__rabitq-4bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "parameters": {
      "nprobe": 256
    },
    "parameterText": "nprobe=256",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9544354838709678,
        "qps": 74.7299811271484,
        "batch": null,
        "samples": [
          75.00688898889742,
          75.9185800035512,
          75.41414081881928,
          73.32554428354099,
          73.98475154093313
        ],
        "cv": 0.012674105334116097,
        "p50": 13.153204,
        "p95": 15.127264349999997,
        "p99": 17.521548779999986,
        "p95ci": [
          14.9950310225,
          15.253587249999997
        ],
        "p99ci": [
          16.636167967499986,
          17.820041
        ],
        "resident": 1.9681663513183594,
        "pss": 1.9573297500610352,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 92
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.9544354838709678,
        "qps": 496.1618740848346,
        "batch": null,
        "samples": [
          495.13893112535334,
          496.57125653190394,
          501.47349791407856,
          496.08983756772164,
          491.5358472851155
        ],
        "cv": 0.006426330504591584,
        "p50": 15.8849905,
        "p95": 18.0178242,
        "p99": 18.968760379999996,
        "p95ci": [
          17.857250702499996,
          18.15205485
        ],
        "p99ci": [
          18.67629844,
          19.280956838999998
        ],
        "resident": 1.9682159423828125,
        "pss": 1.957223892211914,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 92
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.9544354838709678,
        "qps": 814.4264242379747,
        "batch": null,
        "samples": [
          812.9318633701753,
          802.7197783001858,
          808.3032762496168,
          809.9666877014043,
          838.2105155684915
        ],
        "cv": 0.015161381189941793,
        "p50": 19.028735,
        "p95": 21.593591800000002,
        "p99": 22.936020650000003,
        "p95ci": [
          21.39046915,
          21.737632
        ],
        "p99ci": [
          22.56036822,
          23.214988
        ],
        "resident": 1.9682884216308594,
        "pss": 1.957296371459961,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 92
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.9544354838709678,
        "qps": 1188.408925589205,
        "batch": null,
        "samples": [
          1185.3044504615082,
          1182.753190535337,
          1195.008995375493,
          1193.972923091881,
          1185.0050684818052
        ],
        "cv": 0.004253007245824305,
        "p50": 26.7421295,
        "p95": 31.2755804,
        "p99": 33.68800089999999,
        "p95ci": [
          30.889047249999997,
          31.470975
        ],
        "p99ci": [
          33.1320904,
          34.517423059999985
        ],
        "resident": 1.9684829711914062,
        "pss": 1.9574909210205078,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 89
        },
        "evidence": {
          "$annotation": 92
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
    "id": "D14__rabitq-4bit-g1|{\"nprobe\":64}",
    "config": "D14__rabitq-4bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "parameters": {
      "nprobe": 64
    },
    "parameterText": "nprobe=64",
    "domain": "原生请求",
    "points": {
      "1": {
        "threads": 1,
        "recall": 0.9139784946236559,
        "qps": 261.2111124622802,
        "batch": null,
        "samples": [
          258.7538958863902,
          263.36906633385456,
          257.0397850719057,
          263.1095826336401,
          263.7832323856105
        ],
        "cv": 0.010597556128427353,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 1.9678306579589844,
        "pss": 1.9569978713989258,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 87
        },
        "evidence": {
          "$annotation": 92
        }
      }
    },
    "targets": []
  }
];
