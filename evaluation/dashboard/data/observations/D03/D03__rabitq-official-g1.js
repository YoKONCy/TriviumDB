// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.observations["D03__rabitq-official-g1"] = [
  {
    "id": "D03__rabitq-official-g1|{\"nprobe\":128}",
    "config": "D03__rabitq-official-g1",
    "dataset": "D03",
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
        "recall": 0.9487903225806451,
        "qps": 498.3170617880149,
        "batch": null,
        "samples": [
          493.32169542320895,
          500.08286830411845,
          511.0336313213404,
          497.7218344584872,
          489.42527943291947
        ],
        "cv": 0.014724737625480842,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.4474372863769531,
        "pss": 0.43452930450439453,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 113
        }
      }
    },
    "targets": []
  },
  {
    "id": "D03__rabitq-official-g1|{\"nprobe\":256}",
    "config": "D03__rabitq-official-g1",
    "dataset": "D03",
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
        "recall": 0.964247311827957,
        "qps": 300.52827680110187,
        "batch": null,
        "samples": [
          292.76695909473625,
          300.8754828210661,
          306.6154434530632,
          300.35078836590907,
          302.03271027073464
        ],
        "cv": 0.014859101008660574,
        "p50": 3.2544895,
        "p95": 3.73296055,
        "p99": 4.244530489999999,
        "p95ci": [
          3.704081,
          3.7681128299999997
        ],
        "p99ci": [
          4.070743,
          4.402388379999996
        ],
        "resident": 0.4477386474609375,
        "pss": 0.4347696304321289,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 113
        }
      },
      "8": {
        "threads": 8,
        "recall": 0.964247311827957,
        "qps": 2316.2451078484546,
        "batch": null,
        "samples": [
          2340.3452256706646,
          2310.7311445562523,
          2304.8934262534995,
          2316.047469271607,
          2309.208273490249
        ],
        "cv": 0.005425553119268649,
        "p50": 3.3958869999999997,
        "p95": 3.8707737,
        "p99": 4.128269079999999,
        "p95ci": [
          3.8358126324999993,
          3.90994035
        ],
        "p99ci": [
          4.06649,
          4.16216195
        ],
        "resident": 0.44780731201171875,
        "pss": 0.4346895217895508,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 113
        }
      },
      "16": {
        "threads": 16,
        "recall": 0.964247311827957,
        "qps": 4157.5093352743315,
        "batch": null,
        "samples": [
          4119.514247995667,
          4101.416102376885,
          4171.994604007571,
          4229.34149011127,
          4165.280231880267
        ],
        "cv": 0.010767596783271266,
        "p50": 3.8390585,
        "p95": 4.443292399999999,
        "p99": 5.561413499999997,
        "p95ci": [
          4.378070484999999,
          4.489458
        ],
        "p99ci": [
          5.234375329999999,
          5.756052139999998
        ],
        "resident": 0.44789886474609375,
        "pss": 0.4347810745239258,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 113
        }
      },
      "32": {
        "threads": 32,
        "recall": 0.964247311827957,
        "qps": 7101.67536044184,
        "batch": null,
        "samples": [
          7063.3634041155065,
          7059.798933217879,
          7058.365013526272,
          7152.173106835759,
          7174.676344513781
        ],
        "cv": 0.007173501336680959,
        "p50": 4.3022255000000005,
        "p95": 4.8740299,
        "p99": 5.631713719999999,
        "p95ci": [
          4.817138545,
          4.918033049999999
        ],
        "p99ci": [
          5.3940058465,
          5.920850000000001
        ],
        "resident": 0.4480857849121094,
        "pss": 0.4349679946899414,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 98
        },
        "evidence": {
          "$annotation": 113
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
    "id": "D03__rabitq-official-g1|{\"nprobe\":64}",
    "config": "D03__rabitq-official-g1",
    "dataset": "D03",
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
        "recall": 0.9233870967741935,
        "qps": 748.0082780095631,
        "batch": null,
        "samples": [
          735.7571525876645,
          753.979524032777,
          742.6266364616054,
          741.9969348088431,
          765.6811421569255
        ],
        "cv": 0.014188288315439498,
        "p50": null,
        "p95": null,
        "p99": null,
        "p95ci": [],
        "p99ci": [],
        "resident": 0.4473991394042969,
        "pss": 0.4344911575317383,
        "queries": 744,
        "route": "标准",
        "eligible": true,
        "notes": {
          "$annotation": 96
        },
        "evidence": {
          "$annotation": 113
        }
      }
    },
    "targets": []
  }
];
