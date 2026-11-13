// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.configurations["D03"] = [
  {
    "id": "D03__vsag-fp32-g0",
    "dataset": "D03",
    "method": "VSAG HGraph FP32",
    "family": "全精度图",
    "buildConfig": "R=64; L=400",
    "build": 790.556801572442,
    "save": 2.113890990614891,
    "saveWrapper": 2.113890990614891,
    "load": 7.019726812839508,
    "hot": 3.1597900390625,
    "cold": 0.0,
    "resident": 3.4813690185546875,
    "index": 3.172144646756351,
    "external": 0.0,
    "deployment": 3.172144646756351,
    "exact": false,
    "components": {
      "nav": 2.90625,
      "codes": 0.0,
      "graph": 0.2459716796875,
      "factors": 0.007568359375
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
    "id": "D03__vsag-sq8-g0",
    "dataset": "D03",
    "method": "VSAG HGraph SQ8 + FP32",
    "family": "量化图",
    "buildConfig": "R=64; L=400",
    "build": 798.597605124116,
    "save": 2.746237874031067,
    "saveWrapper": 2.746237874031067,
    "load": 8.004164636135101,
    "hot": 0.9801025390625,
    "cold": 2.90625,
    "resident": 4.210033416748047,
    "index": 3.8987131286412477,
    "external": 0.0,
    "deployment": 3.8987131286412477,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.7265625,
      "graph": 0.2459716796875,
      "factors": 0.007568359375
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
    "id": "D03__vsag-sq8-g1",
    "dataset": "D03",
    "method": "VSAG HGraph SQ8 + FP32",
    "family": "量化图",
    "buildConfig": "R=32; L=200",
    "build": 352.3960295319557,
    "save": 3.7776062935590744,
    "saveWrapper": 3.7776062935590744,
    "load": 7.334093943238258,
    "hot": 0.8590087890625,
    "cold": 2.90625,
    "resident": 4.102569580078125,
    "index": 3.7780191618949175,
    "external": 0.0,
    "deployment": 3.7780191618949175,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.7265625,
      "graph": 0.1248779296875,
      "factors": 0.007568359375
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
    "id": "D03__D03-recheck-faiss-hnsw-g0",
    "dataset": "D03",
    "method": "FAISS HNSW FP32",
    "family": "全精度图",
    "buildConfig": "M=32; efC=400",
    "build": 759.5923447906971,
    "save": 1.984272986650467,
    "saveWrapper": 1.984272986650467,
    "load": 5.825050229,
    "hot": 3.114463187754154,
    "cold": 0.0,
    "resident": null,
    "index": 3.1144633013755083,
    "external": 0.0,
    "deployment": 3.1144633013755083,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.2534402385354042,
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
    "id": "D03__faiss-ivfflat-g1",
    "dataset": "D03",
    "method": "FAISS IVF-Flat",
    "family": "倒排全精度",
    "buildConfig": "IVF4096,Flat",
    "build": 25.01160065829754,
    "save": 1.4019405096769333,
    "saveWrapper": 1.4019405096769333,
    "load": 3.609706872,
    "hot": 2.880802631378174,
    "cold": 0.0,
    "resident": null,
    "index": 2.8802229268476367,
    "external": 0.0,
    "deployment": 2.8802229268476367,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.0,
      "factors": 0.019779682159423828
    },
    "install": null,
    "warmLoad": null,
    "training": 16.873425796628,
    "graphInsert": null,
    "notes": {
      "$annotation": 6
    },
    "evidence": {
      "$annotation": 8
    }
  },
  {
    "id": "D03__faiss-ivfpq-g1",
    "dataset": "D03",
    "method": "FAISS IVF-PQ",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ96x8",
    "build": 58.350970566272736,
    "save": 0.07453411817550659,
    "saveWrapper": 0.07453411817550659,
    "load": 0.080136833,
    "hot": 0.10991907119750977,
    "cold": 0.0,
    "resident": null,
    "index": 0.1093394048511982,
    "external": 0.0,
    "deployment": 0.1093394048511982,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.0,
      "factors": 0.020512104034423828
    },
    "install": null,
    "warmLoad": null,
    "training": 48.32006646692753,
    "graphInsert": null,
    "notes": {
      "$annotation": 9
    },
    "evidence": {
      "$annotation": 10
    }
  },
  {
    "id": "D03__faiss-opq-ivfpq-refine-g1",
    "dataset": "D03",
    "method": "FAISS OPQ-IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ96,IVF4096,PQ96x8,Refine(Flat)",
    "build": 147.59973457455635,
    "save": 1.487435057759285,
    "saveWrapper": 1.487435057759285,
    "load": 4.548531328,
    "hot": 0.11211634427309036,
    "cold": 2.86102294921875,
    "resident": null,
    "index": 2.9725597659125924,
    "external": 0.0,
    "deployment": 2.9725597659125924,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.0,
      "factors": 0.022709377110004425
    },
    "install": null,
    "warmLoad": null,
    "training": 131.32755509018898,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 12
    }
  },
  {
    "id": "D03__faiss-rabitq-fp32-g0",
    "dataset": "D03",
    "method": "FAISS RaBitQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF1024,RaBitQfs,Refine(Flat)",
    "build": 9.610794380307198,
    "save": 1.5261080265045166,
    "saveWrapper": 1.5261080265045166,
    "load": 5.383552531,
    "hot": 0.15339326858520508,
    "cold": 2.86102294921875,
    "resident": null,
    "index": 2.969866309314966,
    "external": 0.0,
    "deployment": 2.969866309314966,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.14296722412109375,
      "graph": 0.0,
      "factors": 0.010426044464111328
    },
    "install": null,
    "warmLoad": null,
    "training": 1.323310375213623,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 13
    }
  },
  {
    "id": "D03__faiss-rabitq-sq8-g0",
    "dataset": "D03",
    "method": "FAISS RaBitQ + SQ8",
    "family": "倒排量化",
    "buildConfig": "IVF1024,RaBitQfs,Refine(SQ8)",
    "build": 7.644545540213585,
    "save": 0.49290619790554047,
    "saveWrapper": 0.49290619790554047,
    "load": 0.609676706,
    "hot": 0.15339326858520508,
    "cold": 0.7152614593505859,
    "resident": null,
    "index": 0.8241048529744148,
    "external": 0.0,
    "deployment": 0.8241048529744148,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.14296722412109375,
      "graph": 0.0,
      "factors": 0.010426044464111328
    },
    "install": null,
    "warmLoad": null,
    "training": 1.4267240315675735,
    "graphInsert": null,
    "notes": {
      "$annotation": 14
    },
    "evidence": {
      "$annotation": 15
    }
  },
  {
    "id": "D03__opq96-fastscan-refine",
    "dataset": "D03",
    "method": "FAISS OPQ-IVF-PQ FastScan + FP32",
    "family": "倒排量化",
    "buildConfig": "OPQ96,IVF4096,PQ96x4fs,Refine(Flat)",
    "build": 115.6201054751873,
    "save": 1.9603103399276733,
    "saveWrapper": 1.9603103399276733,
    "load": 4.942427584,
    "hot": 0.09052897244691849,
    "cold": 2.86102294921875,
    "resident": null,
    "index": 2.9301613392308354,
    "external": 0.0,
    "deployment": 2.9301613392308354,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.06893348693847656,
      "graph": 0.0,
      "factors": 0.021595485508441925
    },
    "install": null,
    "warmLoad": null,
    "training": 100.158646017313,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 16
    }
  },
  {
    "id": "D03__pq96-refine",
    "dataset": "D03",
    "method": "FAISS IVF-PQ + FP32",
    "family": "倒排量化",
    "buildConfig": "IVF4096,PQ96x8,Refine(Flat)",
    "build": 59.82083658874035,
    "save": 2.163373813033104,
    "saveWrapper": 2.163373813033104,
    "load": 5.460794001,
    "hot": 0.10991907119750977,
    "cold": 2.86102294921875,
    "resident": null,
    "index": 2.9703624341636896,
    "external": 0.0,
    "deployment": 2.9703624341636896,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.0,
      "factors": 0.020512104034423828
    },
    "install": null,
    "warmLoad": null,
    "training": 45.12152214348316,
    "graphInsert": null,
    "notes": {
      "$annotation": 11
    },
    "evidence": {
      "$annotation": 17
    }
  },
  {
    "id": "D03__diskann-pq-fp32-g1",
    "dataset": "D03",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "buildConfig": "R=32; L=200; PQ48×8",
    "build": 217.87805400000002,
    "save": null,
    "saveWrapper": null,
    "load": 4.807565222,
    "hot": 3.0398400351405144,
    "cold": 0.0,
    "resident": 3.1248207092285156,
    "index": 3.0310182571411133,
    "external": 0.0,
    "deployment": 3.0310182571411133,
    "exact": false,
    "components": {
      "nav": 2.8610258661210537,
      "codes": 0.04470353573560715,
      "graph": 0.17881416901946068,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 2.947635375,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 18
    },
    "evidence": {
      "$annotation": 19
    }
  },
  {
    "id": "D03__D03-recheck-bbq-g0",
    "dataset": "D03",
    "method": "Lucene BBQ + FP32",
    "family": "量化图",
    "buildConfig": "M=32; efC=400",
    "build": 3886.1011820350004,
    "save": null,
    "saveWrapper": null,
    "load": 27.759744954000002,
    "hot": 0.17091817688196898,
    "cold": 2.86102294921875,
    "resident": 12.120468139648438,
    "index": 3.0300318589434028,
    "external": 2.86102294921875,
    "deployment": 5.891054808162153,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.06288189720362425,
      "factors": 0.01862931251525879
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 20
    },
    "evidence": {
      "$annotation": 21
    }
  },
  {
    "id": "D03__D03-recheck-diskann-pq-only-g0",
    "dataset": "D03",
    "method": "DiskANN PQ导航 + FP32精排",
    "family": "量化图",
    "buildConfig": "R=64; L=400; PQ48×8",
    "build": 418.367656,
    "save": null,
    "saveWrapper": null,
    "load": 5.668081859,
    "hot": 0.40233176574110985,
    "cold": 2.861025810241699,
    "resident": 3.3432273864746094,
    "index": 3.0840001553297043,
    "external": 0.0,
    "deployment": 3.0840001553297043,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.04470348358154297,
      "graph": 0.3576282821595669,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 2.7880641001999997,
    "training": 36.837534,
    "graphInsert": 381.530122,
    "notes": {
      "$annotation": 22
    },
    "evidence": {
      "$annotation": 23
    }
  },
  {
    "id": "D03__D03-recheck-diskann-rust-g1",
    "dataset": "D03",
    "method": "DiskANN FP32",
    "family": "全精度图",
    "buildConfig": "R=32; L=200",
    "build": 370.960887,
    "save": null,
    "saveWrapper": null,
    "load": 4.815551921,
    "hot": 3.0398400351405144,
    "cold": 0.0,
    "resident": 3.0658798217773438,
    "index": 2.9874883480370045,
    "external": 0.0,
    "deployment": 2.9874883480370045,
    "exact": false,
    "components": {
      "nav": 2.8610258661210537,
      "codes": null,
      "graph": 0.17881416901946068,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 2.5998600328,
    "training": 0.0,
    "graphInsert": 370.960887,
    "notes": {
      "$annotation": 24
    },
    "evidence": {
      "$annotation": 25
    }
  },
  {
    "id": "D03__D03-sqg-integer-fixed-g0",
    "dataset": "D03",
    "method": "SymphonyQG",
    "family": "量化图",
    "buildConfig": "R=64; L=400; iter=3",
    "build": 585.3092323690653,
    "save": 23.802766159176826,
    "saveWrapper": 23.802766159176826,
    "load": 14.32336288690567,
    "hot": 11.444095611572266,
    "cold": 0.0,
    "resident": 11.49002456665039,
    "index": 11.444095615297556,
    "external": 0.0,
    "deployment": 11.444095615297556,
    "exact": true,
    "components": {
      "nav": 2.86102294921875,
      "codes": 7.62939453125,
      "graph": 0.2384185791015625,
      "factors": 0.7152595520019531
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
    "id": "D03__alpha-cng-g0",
    "dataset": "D03",
    "method": "α-CNG",
    "family": "全精度图",
    "buildConfig": "R=64; L=400",
    "build": 1882.4950721263885,
    "save": 0.25677765905857086,
    "saveWrapper": 0.25677765905857086,
    "load": 0.32169120013713837,
    "hot": 3.1218403168022633,
    "cold": 0.0,
    "resident": 3.1833114624023438,
    "index": 0.2421909235417843,
    "external": 2.86102294921875,
    "deployment": 3.1032138727605343,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.26081736758351326,
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
    "id": "D03__binary-centered-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 中心化 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 68.00323496758938,
    "save": 0.12116739153862,
    "saveWrapper": 0.12116739153862,
    "load": 0.25979918241500854,
    "hot": 0.22376078367233276,
    "cold": 2.86102294921875,
    "resident": 3.1928672790527344,
    "index": 0.22376089729368687,
    "external": 2.86102294921875,
    "deployment": 3.084783846512437,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.1343509554862976,
      "factors": 2.86102294921875e-06
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
    "id": "D03__binary-sign-fp32-g1",
    "dataset": "D03",
    "method": "FAISS BinaryHNSW 符号位 + FP32",
    "family": "量化图",
    "buildConfig": "M=16; 底层容量=32; efC=200",
    "build": 61.2420222312212,
    "save": 0.13387008011341095,
    "saveWrapper": 0.13387008011341095,
    "load": 0.2557232975959778,
    "hot": 0.22376078367233276,
    "cold": 2.86102294921875,
    "resident": 3.187236785888672,
    "index": 0.22376089729368687,
    "external": 2.86102294921875,
    "deployment": 3.084783846512437,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.08940696716308594,
      "graph": 0.1343509554862976,
      "factors": 2.86102294921875e-06
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 30
    },
    "evidence": {
      "$annotation": 32
    }
  },
  {
    "id": "D03__diskann-pq-fp32-g0",
    "dataset": "D03",
    "method": "DiskANN PQ构建 / FP32查询",
    "family": "量化图",
    "buildConfig": "R=64; L=400; PQ48×8",
    "build": 418.367656,
    "save": null,
    "saveWrapper": null,
    "load": 4.755080899,
    "hot": 3.2186541482806206,
    "cold": 0.0,
    "resident": 3.3077659606933594,
    "index": 3.0840001553297043,
    "external": 0.0,
    "deployment": 3.0840001553297043,
    "exact": false,
    "components": {
      "nav": 2.8610258661210537,
      "codes": 0.04470353573560715,
      "graph": 0.3576282821595669,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": 2.5000906384,
    "training": 36.837534,
    "graphInsert": 381.530122,
    "notes": {
      "$annotation": 33
    },
    "evidence": {
      "$annotation": 34
    }
  },
  {
    "id": "D03__hnswlib-g1",
    "dataset": "D03",
    "method": "HNSWlib FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 261.2401856184006,
    "save": 1.5543250739574432,
    "saveWrapper": 1.5543250739574432,
    "load": 4.356230914592743,
    "hot": 3.006835635751486,
    "cold": 0.0,
    "resident": 3.1906394958496094,
    "index": 2.999385144561529,
    "external": 0.0,
    "deployment": 2.999385144561529,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": null,
      "graph": 0.14581268653273582,
      "factors": null
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
    "id": "D03__hscg-g0",
    "dataset": "D03",
    "method": "HSCG",
    "family": "全精度图",
    "buildConfig": "官方 coco-i2i 预设（实际R=60）",
    "build": 3028.007871747017,
    "save": 0.5936817228794098,
    "saveWrapper": 0.5936817228794098,
    "load": 0.1982550472021103,
    "hot": 2.96028408408165,
    "cold": 0.0,
    "resident": 3.0449981689453125,
    "index": 0.0806346945464611,
    "external": 2.86102294921875,
    "deployment": 2.941657643765211,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.09926113486289978,
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
      "$annotation": 37
    }
  },
  {
    "id": "D03__mirage-anns-g0",
    "dataset": "D03",
    "method": "MIRAGE-ANNS",
    "family": "量化图",
    "buildConfig": "M=32; S=32,R=4,iter=15",
    "build": 228.82868947088718,
    "save": 1.6254491955041885,
    "saveWrapper": 1.6254491955041885,
    "load": 4.683848187327385,
    "hot": 3.114463187754154,
    "cold": 0.0,
    "resident": 3.1907691955566406,
    "index": 3.1144633013755083,
    "external": 0.0,
    "deployment": 3.1144633013755083,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.2534402385354042,
      "factors": 0.0
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 38
    },
    "evidence": {
      "$annotation": 39
    }
  },
  {
    "id": "D03__ngt-g1",
    "dataset": "D03",
    "method": "NGT-QG",
    "family": "量化图",
    "buildConfig": "ANNG edge=40; ONNG/QG out=32; Q=2,k=32,p=8",
    "build": 1703.5899608582258,
    "save": null,
    "saveWrapper": 16.63721987605095,
    "load": 12.817029312252998,
    "hot": 6.020900037139654,
    "cold": 2.86102294921875,
    "resident": 9.910110473632812,
    "index": 12.870454717427492,
    "external": 0.0,
    "deployment": 12.870454717427492,
    "exact": false,
    "components": {
      "nav": 0.017106056213378906,
      "codes": 5.85890007019043,
      "graph": 0.14489391073584557,
      "factors": 0.0
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
    "id": "D03__rabitq-32bit-standard",
    "dataset": "D03",
    "method": "RaBitQ 官方 IVF + FP32",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 58.19495905935764,
    "save": 1.455109879374504,
    "saveWrapper": 1.455109879374504,
    "load": 5.390217214822769,
    "hot": 0.12265419960021973,
    "cold": 2.86102294921875,
    "resident": 3.03326416015625,
    "index": 2.983585638925433,
    "external": 0.0,
    "deployment": 2.983585638925433,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.10708773136138916,
      "graph": null,
      "factors": 0.015566468238830566
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
    "id": "D03__rabitq-7bit-standard",
    "dataset": "D03",
    "method": "RaBitQ 官方 IVF 7 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 标准量化",
    "build": 61.06573320925236,
    "save": 0.34093280136585236,
    "saveWrapper": 0.34093280136585236,
    "load": 0.5470823347568512,
    "hot": 0.6666914224624634,
    "cold": 0.0,
    "resident": 0.7167434692382812,
    "index": 0.6665999013930559,
    "external": 0.0,
    "deployment": 0.6665999013930559,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.6511249542236328,
      "graph": null,
      "factors": 0.015566468238830566
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 44
    },
    "evidence": {
      "$annotation": 45
    }
  },
  {
    "id": "D03__rabitq-official-g1",
    "dataset": "D03",
    "method": "RaBitQ 官方 IVF 4 bit",
    "family": "倒排量化",
    "buildConfig": "nlist=4096; 快速量化",
    "build": 54.46599830687046,
    "save": 0.20404189825057983,
    "saveWrapper": 0.20404189825057983,
    "load": 0.6381255239248276,
    "hot": 0.3984029293060303,
    "cold": 0.0,
    "resident": 0.4480857849121094,
    "index": 0.3983114082366228,
    "external": 0.0,
    "deployment": 0.3983114082366228,
    "exact": false,
    "components": {
      "nav": 0.0,
      "codes": 0.3828364610671997,
      "graph": null,
      "factors": 0.015566468238830566
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 46
    },
    "evidence": {
      "$annotation": 47
    }
  },
  {
    "id": "D03__svs-lvq4x4-g0",
    "dataset": "D03",
    "method": "SVS LVQ4×4（共8 bit）",
    "family": "量化图",
    "buildConfig": "R=64; L=400; pad=32",
    "build": 831.5347849577665,
    "save": null,
    "saveWrapper": 17.778289645910263,
    "load": 1.3473649770021439,
    "hot": 0.6295769214630127,
    "cold": 0.35762786865234375,
    "resident": 1.0416107177734375,
    "index": 0.987210595048964,
    "external": 0.0,
    "deployment": 0.987210595048964,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 0.7152557373046875,
      "graph": 0.24214386940002441,
      "factors": 0.02980518341064453
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 48
    },
    "evidence": {
      "$annotation": 49
    }
  },
  {
    "id": "D03__svs-lvq4x8-g0",
    "dataset": "D03",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "buildConfig": "R=64; L=400; pad=32",
    "build": 774.8305243849754,
    "save": null,
    "saveWrapper": 25.154870808124542,
    "load": 1.851208508014679,
    "hot": 0.6295769214630127,
    "cold": 0.7152557373046875,
    "resident": 1.3984756469726562,
    "index": 1.3448384637013078,
    "external": 0.0,
    "deployment": 1.3448384637013078,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 1.0728836059570312,
      "graph": 0.24214386940002441,
      "factors": 0.02980518341064453
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 48
    },
    "evidence": {
      "$annotation": 50
    }
  },
  {
    "id": "D03__svs-lvq4x8-pad64-g1",
    "dataset": "D03",
    "method": "SVS LVQ4×8（共12 bit）",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=64",
    "build": 248.01346589624882,
    "save": 23.51759572327137,
    "saveWrapper": 25.0641777664423,
    "load": 1.6655641049146652,
    "hot": 0.5401699542999268,
    "cold": 0.7152557373046875,
    "resident": 1.3095054626464844,
    "index": 1.2554314956068993,
    "external": 0.0,
    "deployment": 1.2554314956068993,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 1.0728836059570312,
      "graph": 0.12293457984924316,
      "factors": 0.059607505798339844
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
    "id": "D03__svs-lvq8-g0",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "buildConfig": "R=64; L=400; pad=32",
    "build": 865.9468814879656,
    "save": 16.60438697040081,
    "saveWrapper": 17.74835166335106,
    "load": 1.2069066911935806,
    "hot": 0.9872047901153564,
    "cold": 0.0,
    "resident": 1.03961181640625,
    "index": 0.987209158949554,
    "external": 0.0,
    "deployment": 0.987209158949554,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 0.7152557373046875,
      "graph": 0.24214386940002441,
      "factors": 0.02980518341064453
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 53
    },
    "evidence": {
      "$annotation": 54
    }
  },
  {
    "id": "D03__svs-lvq8-pad32-g1",
    "dataset": "D03",
    "method": "SVS LVQ8",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 260.7986218780279,
    "save": 15.43281902372837,
    "saveWrapper": 16.455997228622437,
    "load": 1.376284196972847,
    "hot": 0.8679955005645752,
    "cold": 0.0,
    "resident": 0.9202880859375,
    "index": 0.8679998684674501,
    "external": 0.0,
    "deployment": 0.8679998684674501,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 0.7152557373046875,
      "graph": 0.12293457984924316,
      "factors": 0.02980518341064453
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 53
    },
    "evidence": {
      "$annotation": 55
    }
  },
  {
    "id": "D03__usearch-g0",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "buildConfig": "M=32; efC=400",
    "build": 1133.846072152257,
    "save": 2.846327766776085,
    "saveWrapper": 2.846327766776085,
    "load": 4.863882333040237,
    "hot": 4.484375,
    "cold": 0.0,
    "resident": 3.287731170654297,
    "index": 3.1182314604520798,
    "external": 0.0,
    "deployment": 3.1182314604520798,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.4921875,
      "factors": 1.13116455078125
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 56
    },
    "evidence": {
      "$annotation": 57
    }
  },
  {
    "id": "D03__usearch-g1",
    "dataset": "D03",
    "method": "USearch FP32",
    "family": "全精度图",
    "buildConfig": "M=16; efC=200",
    "build": 397.3840866833925,
    "save": 2.1065554171800613,
    "saveWrapper": 2.1065554171800613,
    "load": 4.643501490354538,
    "hot": 4.234375,
    "cold": 0.0,
    "resident": 3.170177459716797,
    "index": 2.9993492513895035,
    "external": 0.0,
    "deployment": 2.9993492513895035,
    "exact": false,
    "components": {
      "nav": 2.86102294921875,
      "codes": 0.0,
      "graph": 0.2421875,
      "factors": 1.13116455078125
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 58
    },
    "evidence": {
      "$annotation": 59
    }
  },
  {
    "id": "D03__svs-lvq4-g1",
    "dataset": "D03",
    "method": "SVS LVQ4",
    "family": "量化图",
    "buildConfig": "R=32; L=200; pad=32",
    "build": 184.33067852258682,
    "save": 8.272125408053398,
    "saveWrapper": 9.049864694476128,
    "load": 1.0246178209781647,
    "hot": 0.5103676319122314,
    "cold": 0.0,
    "resident": 0.5662117004394531,
    "index": 0.510372000746429,
    "external": 0.0,
    "deployment": 0.510372000746429,
    "exact": true,
    "components": {
      "nav": 0.0,
      "codes": 0.35762786865234375,
      "graph": 0.12293457984924316,
      "factors": 0.02980518341064453
    },
    "install": null,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 60
    },
    "evidence": {
      "$annotation": 61
    }
  },
  {
    "id": "D03__quiver-main",
    "dataset": "D03",
    "method": "QuIVer",
    "family": "QuIVer",
    "buildConfig": "M=32; efC=128; α=1.2; 8T",
    "build": 89.399057695,
    "save": 1.557383497,
    "saveWrapper": 1.557383497,
    "load": 12.411925393,
    "hot": 0.9201467037200928,
    "cold": 2.86102294921875,
    "resident": 3.832447052001953,
    "index": 0.6761402413249016,
    "external": 2.86102294921875,
    "deployment": 3.5371631905436516,
    "exact": false,
    "components": {
      "nav": 0,
      "codes": 0.4246830940246582,
      "graph": 0.4805624485015869,
      "factors": 0.014901161193847656
    },
    "install": 11.161405496,
    "nativeLoad": 0.941435216,
    "reloadInstall": 11.470490177,
    "ingest": 5.161089477,
    "inputMap": 1.5232e-05,
    "prepare": 107.278951397,
    "warmLoad": null,
    "training": null,
    "graphInsert": null,
    "notes": {
      "$annotation": 62
    },
    "evidence": {
      "$annotation": 63
    }
  }
];
