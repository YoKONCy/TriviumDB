// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.configurations["D05"] = [
  {
    "id": "D05__bbq-g1",
    "dataset": "D05",
    "method": "Lucene BBQ + FP32",
    "family": "量化图",
    "buildConfig": "M=16; efC=200",
    "build": 3765.341530486,
    "save": null,
    "saveWrapper": null,
    "load": 55.570575682000005,
    "hot": 0.2662992151454091,
    "cold": 5.664825439453125,
    "resident": 16.806610107421875,
    "index": 5.9291306007653475,
    "external": 5.664825439453125,
    "deployment": 11.593956040218472,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.17702579498291016,
      "graph": 0.07082751113921404,
      "factors": 0.018445909023284912
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
    "id": "D05__vsag-fp32-g1",
    "dataset": "D05",
    "method": "VSAG HGraph FP32",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 703.262945279479,
    "save": 8.11664155125618,
    "saveWrapper": 8.11664155125618,
    "load": 11.722930654883385,
    "hot": 5.84906005859375,
    "cold": 0.0,
    "resident": 6.3022613525390625,
    "index": 5.862320861779153,
    "external": 0.0,
    "deployment": 5.862320861779153,
    "exact": false,
    "components": {
      "nav": 5.71875,
      "codes": 0.0,
      "graph": 0.12286376953125,
      "factors": 0.0074462890625
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
    "id": "D05__vsag-sq8-g1",
    "dataset": "D05",
    "method": "VSAG HGraph SQ8 + FP32",
    "family": "量化图",
    "buildConfig": "R=32; L=200",
    "build": 704.5113508552313,
    "save": 13.842903912067413,
    "saveWrapper": 13.842903912067413,
    "load": 14.455540671944618,
    "hot": 1.55999755859375,
    "cold": 5.71875,
    "resident": 7.749214172363281,
    "index": 7.292020073160529,
    "external": 0.0,
    "deployment": 7.292020073160529,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 1.4296875,
      "graph": 0.12286376953125,
      "factors": 0.0074462890625
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
    "id": "D05__faiss-hnsw-g1",
    "dataset": "D05",
    "method": "FAISS HNSW FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 620.2859175801277,
    "save": 6.477926567196846,
    "saveWrapper": 6.477926567196846,
    "load": 11.353554802,
    "hot": 5.797834038734436,
    "cold": 0.0,
    "resident": null,
    "index": 5.79783415235579,
    "external": 0.0,
    "deployment": 5.79783415235579,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.13300859928131104,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 7
    }
  },
  {
    "id": "D05__faiss-ivfflat-g1",
    "dataset": "D05",
    "method": "FAISS IVF-Flat",
    "family": "倒排全精度",
    "buildConfig": "IVF4096,Flat",
    "build": 46.7809332460165,
    "save": 6.43738953769207,
    "saveWrapper": 6.43738953769207,
    "load": 8.799365983,
    "hot": 5.69624936580658,
    "cold": 0.0,
    "resident": null,
    "index": 5.6956696612760425,
    "external": 0.0,
    "deployment": 5.6956696612760425,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.0,
      "factors": 0.03142392635345459
    },
    "install": null,
    "warmLoad": null,
    "training": 31.600771456956863,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 8
    }
  },
  {
    "id": "D05__faiss-ivfpq-g1",
    "dataset": "D05",
    "method": "FAISS IVF-PQ",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ96x8",
    "build": 111.30977350473404,
    "save": 0.09139883518218994,
    "saveWrapper": 0.09139883518218994,
    "load": 0.142197014,
    "hot": 0.12140166759490967,
    "cold": 0.0,
    "resident": null,
    "index": 0.1208220012485981,
    "external": 0.0,
    "deployment": 0.1208220012485981,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08851289749145508,
      "graph": 0.0,
      "factors": 0.03288877010345459
    },
    "install": null,
    "warmLoad": null,
    "training": 61.68514955043793,
    "graphInsert": null,
    "notes": {
      "$annotation": 9
    },
    "evidence": {
      "$annotation": 10
    }
  },
  {
    "id": "D05__faiss-opq-ivfpq-refine-g1",
    "dataset": "D05",
    "method": "FAISS OPQ-IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ96,IVF4096,PQ96x8,Refine(Flat)",
    "build": 418.95104292035103,
    "save": 6.8033833503723145,
    "saveWrapper": 6.8033833503723145,
    "load": 9.705239739,
    "hot": 0.13019073754549026,
    "cold": 5.664825439453125,
    "resident": null,
    "index": 5.794436649419367,
    "external": 0.0,
    "deployment": 5.794436649419367,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08851289749145508,
      "graph": 0.0,
      "factors": 0.04167784005403519
    },
    "install": null,
    "warmLoad": null,
    "training": 355.90709322690964,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 12
    }
  },
  {
    "id": "D05__faiss-rabitq-fp32-g1",
    "dataset": "D05",
    "method": "FAISS RaBitQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF4096,RaBitQfs,Refine(Flat)",
    "build": 67.13578905165195,
    "save": 6.223505526781082,
    "saveWrapper": 6.223505526781082,
    "load": 11.797328369,
    "hot": 0.3050903081893921,
    "cold": 5.664825439453125,
    "resident": null,
    "index": 5.892014179378748,
    "external": 0.0,
    "deployment": 5.892014179378748,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.2740936279296875,
      "graph": 0.0,
      "factors": 0.03099668025970459
    },
    "install": null,
    "warmLoad": null,
    "training": 36.07547231018543,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 13
    }
  },
  {
    "id": "D05__faiss-rabitq-sq8-g1",
    "dataset": "D05",
    "method": "FAISS RaBitQ + SQ8",
    "family": "倒排量化",
    "buildConfig": "IVF4096,RaBitQfs,Refine(SQ8)",
    "build": 77.20262342691422,
    "save": 0.9355850070714951,
    "saveWrapper": 0.9355850070714951,
    "load": 2.500939681,
    "hot": 0.3050903081893921,
    "cold": 1.4162178039550781,
    "resident": null,
    "index": 1.6434065774083138,
    "external": 0.0,
    "deployment": 1.6434065774083138,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.2740936279296875,
      "graph": 0.0,
      "factors": 0.03099668025970459
    },
    "install": null,
    "warmLoad": null,
    "training": 47.66063313186169,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 14
    }
  },
  {
    "id": "D05__opq-fastscan-refine-g1",
    "dataset": "D05",
    "method": "FAISS OPQ-IVF-PQ FastScan + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ192,IVF4096,PQ192x4fs,Refine(Flat)",
    "build": 277.18770003318787,
    "save": 5.819822579622269,
    "saveWrapper": 5.819822579622269,
    "load": 9.608023296,
    "hot": 0.17610014230012894,
    "cold": 5.664825439453125,
    "resident": null,
    "index": 5.7988173784688115,
    "external": 0.0,
    "deployment": 5.7988173784688115,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.13622283935546875,
      "graph": 0.0,
      "factors": 0.03987730294466019
    },
    "install": null,
    "warmLoad": null,
    "training": 245.93894685804844,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 15
    }
  },
  {
    "id": "D05__pq-refine-g1",
    "dataset": "D05",
    "method": "FAISS IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ192x8,Refine(Flat)",
    "build": 128.93270449340343,
    "save": 6.190333843231201,
    "saveWrapper": 6.190333843231201,
    "load": 9.498017147,
    "hot": 0.20991456508636475,
    "cold": 5.664825439453125,
    "resident": null,
    "index": 5.87416041828692,
    "external": 0.0,
    "deployment": 5.87416041828692,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.17702579498291016,
      "graph": 0.0,
      "factors": 0.03288877010345459
    },
    "install": null,
    "warmLoad": null,
    "training": 96.84367530047894,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 16
    }
  },
  {
    "id": "D05__D05-sqg-integer-fixed-g1",
    "dataset": "D05",
    "method": "SymphonyQG",
    "family": "量化图",
    "buildConfig": "R=32; L=200; iter=3",
    "build": 521.6557254642248,
    "save": 29.499775826931,
    "saveWrapper": 29.499775826931,
    "load": 19.75040140748024,
    "hot": 13.69000244140625,
    "cold": 0.0,
    "resident": 13.851314544677734,
    "index": 13.69000244513154,
    "external": 0.0,
    "deployment": 13.69000244513154,
    "exact": true,
    "components": {
      "nav": 5.664825439453125,
      "codes": 7.5531005859375,
      "graph": 0.11801719665527344,
      "factors": 0.35405921936035156
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 17
    },
    "evidence": {
      "$annotation": 18
    }
  },
  {
    "id": "D05__alpha-cng-g1",
    "dataset": "D05",
    "method": "α-CNG",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 2179.914713382721,
    "save": 0.1690055876970291,
    "saveWrapper": 0.1690055876970291,
    "load": 0.20723477005958557,
    "hot": 5.804969701915979,
    "cold": 0.0,
    "resident": 5.977642059326172,
    "index": 0.12170408293604851,
    "external": 5.664825439453125,
    "deployment": 5.7865295223891735,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.14014426246285439,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 19
    },
    "evidence": {
      "$annotation": 20
    }
  },
  {
    "id": "D05__binary-centered-fp32-g1",
    "dataset": "D05",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 100.38399559259415,
    "save": 0.15832281112670898,
    "saveWrapper": 0.15832281112670898,
    "load": 0.3532673865556717,
    "hot": 0.31004011631011963,
    "cold": 5.664825439453125,
    "resident": 6.200096130371094,
    "index": 0.31004022993147373,
    "external": 5.664825439453125,
    "deployment": 5.974865669384599,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.17702579498291016,
      "graph": 0.13300859928131104,
      "factors": 5.7220458984375e-06
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 21
    },
    "evidence": {
      "$annotation": 22
    }
  },
  {
    "id": "D05__diskann-pq-fp32-g1",
    "dataset": "D05",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "buildConfig": "R=32; L=200; PQ96×8",
    "build": 388.180351,
    "save": null,
    "saveWrapper": null,
    "load": 9.325658171,
    "hot": 5.841857247054577,
    "cold": 0.0,
    "resident": 6.048423767089844,
    "index": 5.888975705951452,
    "external": 0.0,
    "deployment": 5.888975705951452,
    "exact": false,
    "components": {
      "nav": 5.664831217378378,
      "codes": 0.08851299434900284,
      "graph": 0.17702602967619896,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 4.5731168696,
    "training": 72.360553,
    "graphInsert": 315.819798,
    "notes": {
      "$annotation": 23
    },
    "evidence": {
      "$annotation": 24
    }
  },
  {
    "id": "D05__diskann-pq-only-g1",
    "dataset": "D05",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "buildConfig": "R=32; L=200; PQ96×8",
    "build": 388.180351,
    "save": null,
    "saveWrapper": null,
    "load": 9.352064844,
    "hot": 0.26553892716765404,
    "cold": 5.664831161499023,
    "resident": 6.071193695068359,
    "index": 5.888975705951452,
    "external": 0.0,
    "deployment": 5.888975705951452,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08851289749145508,
      "graph": 0.17702602967619896,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 4.7130031088,
    "training": 72.360553,
    "graphInsert": 315.819798,
    "notes": {
      "$annotation": 25
    },
    "evidence": {
      "$annotation": 26
    }
  },
  {
    "id": "D05__diskann-rust-g1",
    "dataset": "D05",
    "method": "DiskANN FP32",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 754.669993,
    "save": null,
    "saveWrapper": null,
    "load": 9.173012468,
    "hot": 5.841857247054577,
    "cold": 0.0,
    "resident": 5.925262451171875,
    "index": 5.8018309734761715,
    "external": 0.0,
    "deployment": 5.8018309734761715,
    "exact": false,
    "components": {
      "nav": 5.664831217378378,
      "codes": null,
      "graph": 0.17702602967619896,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 4.1700474506,
    "training": 0.0,
    "graphInsert": 754.669993,
    "notes": {
      "$annotation": 23
    },
    "evidence": {
      "$annotation": 27
    }
  },
  {
    "id": "D05__hnswlib-g1",
    "dataset": "D05",
    "method": "HNSWlib FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 531.1514561325312,
    "save": 6.2093851417303085,
    "saveWrapper": 6.2093851417303085,
    "load": 7.244168668985367,
    "hot": 5.809181414544582,
    "cold": 0.0,
    "resident": 6.102802276611328,
    "index": 5.801805429160595,
    "external": 0.0,
    "deployment": 5.801805429160595,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": null,
      "graph": 0.14435597509145737,
      "factors": null
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
    "id": "D05__hscg-g1",
    "dataset": "D05",
    "method": "HSCG",
    "family": "全精度图",
    "buildConfig": "官方 coco-i2i 预设（实际R=60）",
    "build": 5972.84054300189,
    "save": 1.2771298289299011,
    "saveWrapper": 1.2771298289299011,
    "load": 0.4040679782629013,
    "hot": 5.850315921008587,
    "cold": 0.0,
    "resident": 6.046825408935547,
    "index": 0.1670503057539463,
    "external": 5.664825439453125,
    "deployment": 5.831875745207071,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.18549048155546188,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 19
    },
    "evidence": {
      "$annotation": 30
    }
  },
  {
    "id": "D05__mirage-anns-g1",
    "dataset": "D05",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "buildConfig": "M=16; S=32,R=4,iter=15",
    "build": 391.1649295091629,
    "save": 6.306236878037453,
    "saveWrapper": 6.306236878037453,
    "load": 9.205401927232742,
    "hot": 5.915730036795139,
    "cold": 0.0,
    "resident": 6.105598449707031,
    "index": 5.915730150416493,
    "external": 0.0,
    "deployment": 5.915730150416493,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.2509045973420143,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 31
    },
    "evidence": {
      "$annotation": 32
    }
  },
  {
    "id": "D05__ngt-g1",
    "dataset": "D05",
    "method": "NGT-QG",
    "family": "量化图",
    "buildConfig": "ANNG edge=40; ONNG/QG out=32; Q=2,k=32,p=8",
    "build": 3991.5114968270063,
    "save": null,
    "saveWrapper": 46.759166181087494,
    "load": 29.85774263739586,
    "hot": 11.77404673025012,
    "cold": 5.664825439453125,
    "resident": 18.578472137451172,
    "index": 24.965367603115737,
    "external": 0.0,
    "deployment": 24.965367603115737,
    "exact": false,
    "components": {
      "nav": 0.03344535827636719,
      "codes": 11.597225189208984,
      "graph": 0.1433761827647686,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 33
    },
    "evidence": {
      "$annotation": 34
    }
  },
  {
    "id": "D05__rabitq-32bit-g1",
    "dataset": "D05",
    "method": "RaBitQ 官方 IVF + FP32",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 93.18674808740616,
    "save": 6.232256859540939,
    "saveWrapper": 6.232256859540939,
    "load": 9.107663989067078,
    "hot": 0.2272452712059021,
    "cold": 5.664825439453125,
    "resident": 6.053291320800781,
    "index": 5.8919792007654905,
    "external": 0.0,
    "deployment": 5.8919792007654905,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.1999969482421875,
      "graph": null,
      "factors": 0.0272483229637146
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
    "id": "D05__rabitq-4bit-g1",
    "dataset": "D05",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 95.4381574690342,
    "save": 0.431970939040184,
    "saveWrapper": 0.431970939040184,
    "load": 1.187326356768608,
    "hot": 0.7656987309455872,
    "cold": 0.0,
    "resident": 0.9275360107421875,
    "index": 0.7656072098761797,
    "external": 0.0,
    "deployment": 0.7656072098761797,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.7384504079818726,
      "graph": null,
      "factors": 0.0272483229637146
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
    "id": "D05__rabitq-7bit-g1",
    "dataset": "D05",
    "method": "RaBitQ 官方 IVF 7 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 99.9026207625866,
    "save": 0.6598008722066879,
    "saveWrapper": 0.6598008722066879,
    "load": 2.0829939544200897,
    "hot": 1.2967761158943176,
    "cold": 0.0,
    "resident": 1.4585456848144531,
    "index": 1.2966845948249102,
    "external": 0.0,
    "deployment": 1.2966845948249102,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 1.269527792930603,
      "graph": null,
      "factors": 0.0272483229637146
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 37
    },
    "evidence": {
      "$annotation": 39
    }
  },
  {
    "id": "D05__svs-lvq4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 366.5945024192333,
    "save": 15.550650805234909,
    "saveWrapper": 16.631847485899925,
    "load": 1.215395987033844,
    "hot": 0.8593184351921082,
    "cold": 0.0,
    "resident": 1.0270843505859375,
    "index": 0.8593228040263057,
    "external": 0.0,
    "deployment": 0.8593228040263057,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 0.7081031799316406,
      "graph": 0.12170523405075073,
      "factors": 0.029510021209716797
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 40
    },
    "evidence": {
      "$annotation": 41
    }
  },
  {
    "id": "D05__svs-lvq4x4-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 530.5723485052586,
    "save": 30.22655062377453,
    "saveWrapper": 32.14913606643677,
    "load": 2.837109461426735,
    "hot": 0.8593184351921082,
    "cold": 0.7081031799316406,
    "resident": 1.73516845703125,
    "index": 1.5674274181947112,
    "external": 0.0,
    "deployment": 1.5674274181947112,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 1.4162063598632812,
      "graph": 0.12170523405075073,
      "factors": 0.029510021209716797
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 42
    },
    "evidence": {
      "$annotation": 43
    }
  },
  {
    "id": "D05__svs-lvq4x8-g1",
    "dataset": "D05",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 466.72501833736897,
    "save": 42.01199673116207,
    "saveWrapper": 45.104902520775795,
    "load": 3.2436856478452682,
    "hot": 0.8593184351921082,
    "cold": 1.4162063598632812,
    "resident": 2.441761016845703,
    "index": 2.275530599988997,
    "external": 0.0,
    "deployment": 2.275530599988997,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 2.124309539794922,
      "graph": 0.12170523405075073,
      "factors": 0.029510021209716797
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 42
    },
    "evidence": {
      "$annotation": 44
    }
  },
  {
    "id": "D05__svs-lvq8-g1",
    "dataset": "D05",
    "method": "SVS LVQ8",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 512.9219255447388,
    "save": 29.03402678668499,
    "saveWrapper": 30.961289688944817,
    "load": 2.1733365952968597,
    "hot": 1.5674216151237488,
    "cold": 0.0,
    "resident": 1.7348136901855469,
    "index": 1.5674259830266237,
    "external": 0.0,
    "deployment": 1.5674259830266237,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 1.4162063598632812,
      "graph": 0.12170523405075073,
      "factors": 0.029510021209716797
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
    "id": "D05__binary-sign-fp32-g1",
    "dataset": "D05",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 90.54244387149811,
    "save": null,
    "saveWrapper": 0.1658681184053421,
    "load": 0.31042610108852386,
    "hot": 0.31004011631011963,
    "cold": 5.664825439453125,
    "resident": 6.207305908203125,
    "index": 0.31004022993147373,
    "external": 5.664825439453125,
    "deployment": 5.974865669384599,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.17702579498291016,
      "graph": 0.13300859928131104,
      "factors": 5.7220458984375e-06
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
    "id": "D05__usearch-g1",
    "dataset": "D05",
    "method": "USearch FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 714.0002838522196,
    "save": null,
    "saveWrapper": 6.178946703672409,
    "load": 8.526314690709114,
    "hot": 8.234375,
    "cold": 0.0,
    "resident": 6.084735870361328,
    "index": 5.801766559481621,
    "external": 0.0,
    "deployment": 5.801766559481621,
    "exact": false,
    "components": {
      "nav": 5.664825439453125,
      "codes": 0.0,
      "graph": 0.2421875,
      "factors": 2.327362060546875
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
    "id": "D05__quiver-main",
    "dataset": "D05",
    "method": "QuIVer",
    "family": "QuIVer",
    "buildConfig": "M=32; efC=128; α=1.2; 8T",
    "build": 128.446925676,
    "save": 1.76666108,
    "saveWrapper": 1.76666108,
    "load": 22.306537693,
    "hot": 1.2649968266487122,
    "cold": 5.664825439453125,
    "resident": 6.999958038330078,
    "index": 0.8464046344161034,
    "external": 5.664825439453125,
    "deployment": 6.511230073869228,
    "exact": false,
    "components": {
      "nav": 0,
      "codes": 0.7744878530502319,
      "graph": 0.47575682401657104,
      "factors": 0.01475214958190918
    },
    "install": 20.832488239,
    "nativeLoad": 1.280067828,
    "reloadInstall": 21.026469865,
    "ingest": 8.21885878,
    "inputMap": 2.0514e-05,
    "prepare": 159.264954289,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 51
    },
    "evidence": {
      "$annotation": 52
    }
  }
];
