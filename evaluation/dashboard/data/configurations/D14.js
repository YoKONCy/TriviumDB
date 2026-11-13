// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.configurations["D14"] = [
  {
    "id": "D14__bbq-g1",
    "dataset": "D14",
    "method": "Lucene BBQ + FP32",
    "family": "量化图",
    "buildConfig": "M=16; efC=200",
    "build": 7601.2276859230005,
    "save": null,
    "saveWrapper": null,
    "load": 131.707704654,
    "hot": 0.7571676261723042,
    "cold": 14.30511474609375,
    "resident": 31.252716064453125,
    "index": 15.05224625300616,
    "external": 14.30511474609375,
    "deployment": 29.35736099909991,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.4470348358154297,
      "graph": 0.2169976718723774,
      "factors": 0.09313511848449707
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 0
    },
    "evidence": {
      "$annotation": 1
    }
  },
  {
    "id": "D14__vsag-fp32-g1",
    "dataset": "D14",
    "method": "VSAG HGraph FP32",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 1134.2680462747812,
    "save": 34.34320990741253,
    "saveWrapper": 34.34320990741253,
    "load": 32.944964826107025,
    "hot": 14.9974365234375,
    "cold": 0.0,
    "resident": 16.347766876220703,
    "index": 15.061127015389502,
    "external": 0.0,
    "deployment": 15.061127015389502,
    "exact": false,
    "components": {
      "nav": 14.34375,
      "codes": 0.0,
      "graph": 0.6163330078125,
      "factors": 0.037353515625
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 2
    },
    "evidence": {
      "$annotation": 3
    }
  },
  {
    "id": "D14__vsag-sq8-g1",
    "dataset": "D14",
    "method": "VSAG HGraph SQ8 + FP32",
    "family": "量化图",
    "buildConfig": "R=32; L=200",
    "build": 1141.8272824287415,
    "save": 46.66673055291176,
    "saveWrapper": 46.66673055291176,
    "load": 39.393800139427185,
    "hot": 4.2396240234375,
    "cold": 14.34375,
    "resident": 19.887554168701172,
    "index": 18.64707037433982,
    "external": 0.0,
    "deployment": 18.64707037433982,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 3.5859375,
      "graph": 0.6163330078125,
      "factors": 0.037353515625
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 4
    },
    "evidence": {
      "$annotation": 5
    }
  },
  {
    "id": "D14__D14-recheck-faiss-rabitq-fp32-g1",
    "dataset": "D14",
    "method": "FAISS RaBitQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF4096,RaBitQfs,Refine(Flat)",
    "build": 98.44154439866543,
    "save": 35.46577471494675,
    "saveWrapper": 35.46577471494675,
    "load": 23.886396008,
    "hot": 0.7596001625061035,
    "cold": 14.30511474609375,
    "resident": null,
    "index": 14.844860587269068,
    "external": 0.0,
    "deployment": 14.844860587269068,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.7104454040527344,
      "graph": 0.0,
      "factors": 0.04915475845336914
    },
    "install": null,
    "warmLoad": null,
    "training": 18.991094440221786,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 7
    }
  },
  {
    "id": "D14__faiss-hnsw-g1",
    "dataset": "D14",
    "method": "FAISS HNSW FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 1603.8013902008533,
    "save": 34.08325499296188,
    "saveWrapper": 34.08325499296188,
    "load": 28.196677377,
    "hot": 14.976945579051971,
    "cold": 0.0,
    "resident": null,
    "index": 14.976945692673326,
    "external": 0.0,
    "deployment": 14.976945692673326,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 0.6718308329582214,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 8
    },
    "evidence": {
      "$annotation": 9
    }
  },
  {
    "id": "D14__faiss-ivfflat-g1",
    "dataset": "D14",
    "method": "FAISS IVF-Flat",
    "family": "倒排全精度",
    "buildConfig": "IVF4096,Flat",
    "build": 53.2941153049469,
    "save": 33.62607143819332,
    "saveWrapper": 33.62607143819332,
    "load": 24.662921865,
    "hot": 14.35469675064087,
    "cold": 0.0,
    "resident": null,
    "index": 14.354117046110332,
    "external": 0.0,
    "deployment": 14.354117046110332,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 0.0,
      "factors": 0.04958200454711914
    },
    "install": null,
    "warmLoad": null,
    "training": 16.177311658859253,
    "graphInsert": null,
    "notes": {
      "$annotation": 8
    },
    "evidence": {
      "$annotation": 10
    }
  },
  {
    "id": "D14__faiss-ivfpq-g1",
    "dataset": "D14",
    "method": "FAISS IVF-PQ",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ48x8",
    "build": 154.72528797388077,
    "save": 0.1912083625793457,
    "saveWrapper": 0.1912083625793457,
    "load": 0.184238585,
    "hot": 0.273831844329834,
    "cold": 0.0,
    "resident": null,
    "index": 0.2732521779835224,
    "external": 0.0,
    "deployment": 0.2732521779835224,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.22351741790771484,
      "graph": 0.0,
      "factors": 0.05031442642211914
    },
    "install": null,
    "warmLoad": null,
    "training": 34.52372249960899,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 12
    }
  },
  {
    "id": "D14__faiss-opq-ivfpq-refine-g1",
    "dataset": "D14",
    "method": "FAISS OPQ-IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ48,IVF4096,PQ48x8,Refine(Flat)",
    "build": 295.9745414555073,
    "save": 33.7558740824461,
    "saveWrapper": 33.7558740824461,
    "load": 29.870007499,
    "hot": 0.2760291174054146,
    "cold": 14.30511474609375,
    "resident": null,
    "index": 14.580564335919917,
    "external": 0.0,
    "deployment": 14.580564335919917,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.22351741790771484,
      "graph": 0.0,
      "factors": 0.05251169949769974
    },
    "install": null,
    "warmLoad": null,
    "training": 149.90726798772812,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 13
    }
  },
  {
    "id": "D14__faiss-rabitq-sq8-g1",
    "dataset": "D14",
    "method": "FAISS RaBitQ + SQ8",
    "family": "倒排量化",
    "buildConfig": "IVF4096,RaBitQfs,Refine(SQ8)",
    "build": 77.66053289175034,
    "save": 2.54781711101532,
    "saveWrapper": 2.54781711101532,
    "load": 6.667010201,
    "hot": 0.7596001625061035,
    "cold": 3.576284408569336,
    "resident": null,
    "index": 4.116030283272266,
    "external": 0.0,
    "deployment": 4.116030283272266,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.7104454040527344,
      "graph": 0.0,
      "factors": 0.04915475845336914
    },
    "install": null,
    "warmLoad": null,
    "training": 19.623865336179733,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 14
    }
  },
  {
    "id": "D14__opq-fastscan-refine-g1",
    "dataset": "D14",
    "method": "FAISS OPQ-IVF-PQ FastScan + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ96,IVF4096,PQ96x4fs,Refine(Flat)",
    "build": 177.7042525112629,
    "save": 34.157439678907394,
    "saveWrapper": 34.157439678907394,
    "load": 28.024739175,
    "hot": 0.37682200223207474,
    "cold": 14.30511474609375,
    "resident": null,
    "index": 14.582895142026246,
    "external": 0.0,
    "deployment": 14.582895142026246,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.3254241943359375,
      "graph": 0.0,
      "factors": 0.05139780789613724
    },
    "install": null,
    "warmLoad": null,
    "training": 107.56000837683678,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 15
    }
  },
  {
    "id": "D14__pq-refine-g1",
    "dataset": "D14",
    "method": "FAISS IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ96x8,Refine(Flat)",
    "build": 115.02182851731777,
    "save": 33.97705294191837,
    "saveWrapper": 33.97705294191837,
    "load": 27.569815803,
    "hot": 0.49734926223754883,
    "cold": 14.30511474609375,
    "resident": null,
    "index": 14.801884422078729,
    "external": 0.0,
    "deployment": 14.801884422078729,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.4470348358154297,
      "graph": 0.0,
      "factors": 0.05031442642211914
    },
    "install": null,
    "warmLoad": null,
    "training": 45.48601904511452,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 16
    }
  },
  {
    "id": "D14__D14-diskann-pq-fp32-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "buildConfig": "R=64; L=400; PQ48×8",
    "build": 2572.658175,
    "save": null,
    "saveWrapper": null,
    "load": 24.267524348,
    "hot": 16.093257419764996,
    "cold": 0.0,
    "resident": 16.47374725341797,
    "index": 15.782573148608208,
    "external": 0.0,
    "deployment": 15.782573148608208,
    "exact": false,
    "components": {
      "nav": 14.305117662996054,
      "codes": 0.22351747006177902,
      "graph": 1.7881397567689419,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 12.4534245982,
    "training": 215.369873,
    "graphInsert": 2357.288302,
    "notes": {
      "$annotation": 17
    },
    "evidence": {
      "$annotation": 18
    }
  },
  {
    "id": "D14__D14-diskann-pq-only-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "buildConfig": "R=64; L=400; PQ48×8",
    "build": 2572.658175,
    "save": null,
    "saveWrapper": null,
    "load": 24.243127832,
    "hot": 2.0116571746766567,
    "cold": 14.3051176071167,
    "resident": 16.539249420166016,
    "index": 15.782573148608208,
    "external": 0.0,
    "deployment": 15.782573148608208,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.22351741790771484,
      "graph": 1.7881397567689419,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 12.4579894562,
    "training": 215.369873,
    "graphInsert": 2357.288302,
    "notes": {
      "$annotation": 19
    },
    "evidence": {
      "$annotation": 20
    }
  },
  {
    "id": "D14__D14-diskann-rust-strong-r64-l400",
    "dataset": "D14",
    "method": "DiskANN FP32",
    "family": "全精度图",
    "buildConfig": "R=64; L=400",
    "build": 5640.256666,
    "save": null,
    "saveWrapper": null,
    "load": 23.062473868,
    "hot": 16.093257419764996,
    "cold": 0.0,
    "resident": 16.182941436767578,
    "index": 15.383892960846424,
    "external": 0.0,
    "deployment": 15.383892960846424,
    "exact": false,
    "components": {
      "nav": 14.305117662996054,
      "codes": null,
      "graph": 1.7881397567689419,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 12.3151631648,
    "training": 0.0,
    "graphInsert": 5640.256666,
    "notes": {
      "$annotation": 17
    },
    "evidence": {
      "$annotation": 21
    }
  },
  {
    "id": "D14__D14-sqg-integer-fixed-g1",
    "dataset": "D14",
    "method": "SymphonyQG",
    "family": "量化图",
    "buildConfig": "R=32; L=200; iter=3",
    "build": 1130.1536596268415,
    "save": 100.94270370900631,
    "saveWrapper": 100.94270370900631,
    "load": 45.60011276602745,
    "hot": 35.76279067993164,
    "cold": 0.0,
    "resident": 35.81524658203125,
    "index": 35.76279068365693,
    "external": 0.0,
    "deployment": 35.76279068365693,
    "exact": true,
    "components": {
      "nav": 14.30511474609375,
      "codes": 19.073486328125,
      "graph": 0.5960464477539062,
      "factors": 1.7881431579589844
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 22
    },
    "evidence": {
      "$annotation": 23
    }
  },
  {
    "id": "D14__binary-centered-fp32-g1",
    "dataset": "D14",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 431.5409455448389,
    "save": 0.5800622254610062,
    "saveWrapper": 0.5800622254610062,
    "load": 2.032493233680725,
    "hot": 1.1188685297966003,
    "cold": 14.30511474609375,
    "resident": 14.158313751220703,
    "index": 1.1188686434179544,
    "external": 14.30511474609375,
    "deployment": 15.423983389511704,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.4470348358154297,
      "graph": 0.6718308329582214,
      "factors": 2.86102294921875e-06
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 24
    },
    "evidence": {
      "$annotation": 25
    }
  },
  {
    "id": "D14__hnswlib-g1",
    "dataset": "D14",
    "method": "HNSWlib FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 1007.1630979925394,
    "save": 34.87372440099716,
    "saveWrapper": 34.87372440099716,
    "load": 20.60161229968071,
    "hot": 15.034033469855785,
    "cold": 0.0,
    "resident": 15.763553619384766,
    "index": 14.996780656278133,
    "external": 0.0,
    "deployment": 14.996780656278133,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": null,
      "graph": 0.7289187237620354,
      "factors": null
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 26
    },
    "evidence": {
      "$annotation": 27
    }
  },
  {
    "id": "D14__mirage-anns-g1",
    "dataset": "D14",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "buildConfig": "M=16; S=32,R=4,iter=15",
    "build": 1726.4674818068743,
    "save": 36.06129378080368,
    "saveWrapper": 36.06129378080368,
    "load": 28.158349081873894,
    "hot": 15.572375632822514,
    "cold": 0.0,
    "resident": 15.768234252929688,
    "index": 15.572375746443868,
    "external": 0.0,
    "deployment": 15.572375746443868,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 1.2672608867287636,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 28
    },
    "evidence": {
      "$annotation": 29
    }
  },
  {
    "id": "D14__rabitq-32bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF + FP32",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 257.3127762377262,
    "save": 31.862468153238297,
    "saveWrapper": 31.862468153238297,
    "load": 29.939610317349434,
    "hot": 0.5398191213607788,
    "cold": 14.30511474609375,
    "resident": 14.897716522216797,
    "index": 14.844842357560992,
    "external": 0.0,
    "deployment": 14.844842357560992,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.5093514919281006,
      "graph": null,
      "factors": 0.030467629432678223
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 30
    },
    "evidence": {
      "$annotation": 31
    }
  },
  {
    "id": "D14__rabitq-4bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 273.82536911964417,
    "save": 1.1793795973062515,
    "saveWrapper": 1.1793795973062515,
    "load": 3.4232616275548935,
    "hot": 1.918128252029419,
    "cold": 0.0,
    "resident": 1.9684829711914062,
    "index": 1.9180367309600115,
    "external": 0.0,
    "deployment": 1.9180367309600115,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 1.8876606225967407,
      "graph": null,
      "factors": 0.030467629432678223
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 32
    },
    "evidence": {
      "$annotation": 33
    }
  },
  {
    "id": "D14__rabitq-7bit-g1",
    "dataset": "D14",
    "method": "RaBitQ 官方 IVF 7 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 293.15173177421093,
    "save": 2.1514377146959305,
    "saveWrapper": 2.1514377146959305,
    "load": 5.184543281793594,
    "hot": 3.259277820587158,
    "cold": 0.0,
    "resident": 3.309783935546875,
    "index": 3.2591862995177507,
    "external": 0.0,
    "deployment": 3.2591862995177507,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 3.22881019115448,
      "graph": null,
      "factors": 0.030467629432678223
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 32
    },
    "evidence": {
      "$annotation": 34
    }
  },
  {
    "id": "D14__svs-lvq4x4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 1323.3643676638603,
    "save": 75.01962368190289,
    "saveWrapper": 80.06522683799267,
    "load": 5.737451955676079,
    "hot": 2.5518267154693604,
    "cold": 1.7881393432617188,
    "resident": 4.394378662109375,
    "index": 4.339971860870719,
    "external": 0.0,
    "deployment": 4.339971860870719,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 3.5762786865234375,
      "graph": 0.6146728992462158,
      "factors": 0.14901447296142578
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 35
    },
    "evidence": {
      "$annotation": 36
    }
  },
  {
    "id": "D14__svs-lvq4x8-g1",
    "dataset": "D14",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 1296.4128932505846,
    "save": 113.61686620116234,
    "saveWrapper": 120.5475464463234,
    "load": 8.179503098130226,
    "hot": 2.5518267154693604,
    "cold": 3.5762786865234375,
    "resident": 6.18182373046875,
    "index": 6.128111204132438,
    "external": 0.0,
    "deployment": 6.128111204132438,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 5.364418029785156,
      "graph": 0.6146728992462158,
      "factors": 0.14901447296142578
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 37
    },
    "evidence": {
      "$annotation": 38
    }
  },
  {
    "id": "D14__svs-lvq8-g1",
    "dataset": "D14",
    "method": "SVS LVQ8",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 1357.6654780507088,
    "save": 74.33249053359032,
    "saveWrapper": 79.46915128827095,
    "load": 6.165152862668037,
    "hot": 4.339966058731079,
    "cold": 0.0,
    "resident": 4.394035339355469,
    "index": 4.339970424771309,
    "external": 0.0,
    "deployment": 4.339970424771309,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 3.5762786865234375,
      "graph": 0.6146728992462158,
      "factors": 0.14901447296142578
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 39
    },
    "evidence": {
      "$annotation": 40
    }
  },
  {
    "id": "D14__usearch-g1",
    "dataset": "D14",
    "method": "USearch FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 2240.4398954957724,
    "save": 37.273888781666756,
    "saveWrapper": 37.273888781666756,
    "load": 23.46402771770954,
    "hot": 16.984375,
    "cold": 0.0,
    "resident": 15.503265380859375,
    "index": 14.996711388230324,
    "external": 0.0,
    "deployment": 14.996711388230324,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 0.9921875,
      "factors": 1.68707275390625
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 41
    },
    "evidence": {
      "$annotation": 42
    }
  },
  {
    "id": "D14__D14-ngt-dense-epsilon",
    "dataset": "D14",
    "method": "NGT-QG",
    "family": "量化图",
    "buildConfig": "ANNG edge=40; ONNG/QG out=32; Q=2,k=32,p=8",
    "build": 15774.090685248375,
    "save": null,
    "saveWrapper": null,
    "load": 74.7631991058588,
    "hot": 30.135454032570124,
    "cold": 14.30511474609375,
    "resident": 49.6883430480957,
    "index": 64.38705824501812,
    "external": 0.0,
    "deployment": 64.38705824501812,
    "exact": false,
    "components": {
      "nav": 0.0889120101928711,
      "codes": 29.321531295776367,
      "graph": 0.7250107266008854,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 43
    },
    "evidence": {
      "$annotation": 44
    }
  },
  {
    "id": "D14__D14-binary-sign-high-recall",
    "dataset": "D14",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 401.70765732228756,
    "save": null,
    "saveWrapper": 0.5744595676660538,
    "load": 2.017698720097542,
    "hot": 1.1188685297966003,
    "cold": 14.30511474609375,
    "resident": 15.253398895263672,
    "index": 1.1188686434179544,
    "external": 14.30511474609375,
    "deployment": 15.423983389511704,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.4470348358154297,
      "graph": 0.6718308329582214,
      "factors": 2.86102294921875e-06
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 45
    },
    "evidence": {
      "$annotation": 46
    }
  },
  {
    "id": "D14__svs-lvq4-g1",
    "dataset": "D14",
    "method": "SVS LVQ4",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 991.7364659905434,
    "save": 42.166306272149086,
    "saveWrapper": 45.56024493277073,
    "load": 3.8054410070180893,
    "hot": 2.5518267154693604,
    "cold": 0.0,
    "resident": 2.6087303161621094,
    "index": 2.5518310852348804,
    "external": 0.0,
    "deployment": 2.5518310852348804,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 1.7881393432617188,
      "graph": 0.6146728992462158,
      "factors": 0.14901447296142578
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 47
    },
    "evidence": {
      "$annotation": 48
    }
  },
  {
    "id": "D14__D14-alpha-cng-high-recall",
    "dataset": "D14",
    "method": "α-CNG",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 5949.718314662576,
    "save": null,
    "saveWrapper": 1.144644245505333,
    "load": 1.6679541319608688,
    "hot": 15.013734757900238,
    "cold": 0.0,
    "resident": 15.126850128173828,
    "index": 0.6154877617955208,
    "external": 14.30511474609375,
    "deployment": 14.92060250788927,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 0.708620011806488,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 49
    },
    "evidence": {
      "$annotation": 50
    }
  },
  {
    "id": "D14__hscg-g1",
    "dataset": "D14",
    "method": "HSCG",
    "family": "全精度图",
    "buildConfig": "官方 coco-i2i 预设（实际R=60）",
    "build": 15556.077318742871,
    "save": 3.0873695015907288,
    "saveWrapper": 3.0873695015907288,
    "load": 1.3850862979888916,
    "hot": 14.805387422442436,
    "cold": 0.0,
    "resident": 15.00722885131836,
    "index": 0.40714043006300926,
    "external": 14.30511474609375,
    "deployment": 14.71225517615676,
    "exact": false,
    "components": {
      "nav": 14.30511474609375,
      "codes": 0.0,
      "graph": 0.5002726763486862,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 51
    },
    "evidence": {
      "$annotation": 52
    }
  },
  {
    "id": "D14__quiver-main",
    "dataset": "D14",
    "method": "QuIVer",
    "family": "QuIVer",
    "buildConfig": "M=32; efC=128; α=1.2; 8T",
    "build": 465.22496434,
    "save": 7.707706661,
    "saveWrapper": 7.707706661,
    "load": 56.253850033999996,
    "hot": 4.600733518600464,
    "cold": 14.30511474609375,
    "resident": 19.08055877685547,
    "index": 3.380700998008251,
    "external": 14.30511474609375,
    "deployment": 17.685815744102,
    "exact": false,
    "components": {
      "nav": 0,
      "codes": 2.123415470123291,
      "graph": 2.4028122425079346,
      "factors": 0.07450580596923828
    },
    "install": 59.366012077,
    "nativeLoad": 3.353008321,
    "reloadInstall": 52.900841713,
    "ingest": 31.86682532,
    "inputMap": 1.4691e-05,
    "prepare": 564.165523089,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 53
    },
    "evidence": {
      "$annotation": 54
    }
  }
];
