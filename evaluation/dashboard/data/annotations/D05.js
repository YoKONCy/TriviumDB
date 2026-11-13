// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.annotations["D05"] = [
  {
    "热组件口径": "量化数据区与压缩图下界",
    "导航与精排": "BBQ量化导航；外部FP32精排",
    "内存组件": "冻结jar字节码与元数据SHA交叉核对：每点1-bit码另带16字节校正项；包括实际压缩边、地址数据/元数组、上层ID、centroid及docID映射。索引内.vec与外部FP32两份部署文件保留",
    "未归因开销": "Java对象头、数组头/对齐、DirectoryReader/线程缓存、GC运行时与映射实际驻留单列",
    "构建计时": "Java输入校验、插入、量化图合并、commit及writer关闭；组件哈希与外部精排库准备另计",
    "保存计时": "保存已包含在构建命令或commit中；不另填0秒",
    "加载计时": "Java内部从FP32精排库读入至服务就绪：含两次显式GC、DirectoryReader/IndexSearcher及docID映射；排除JVM启动。不是单独DirectoryReader.open时长。",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "783a22a08fa30bdaefecd03833da29d7dc9f983f087bb193f288b71fc37177a2",
    "protocol": "18fd561d746c13a695c586f1bc78db98c9b36a3de831126a61a9823ffdf70d11"
  },
  {
    "热组件口径": "实际头部与底图容量小计",
    "导航与精排": "FP32参与图搜索",
    "内存组件": "冻结SDK详情总长度匹配索引；实际主数组capacity=999424。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热",
    "未归因开销": "SDK已报告总量尚有84.2 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "454589cc88bc0d74c11add458d25563d8447e0abc70cc30d21173888e2f18471",
    "protocol": "4453e5814fb42c0d24bf7ae385412554a0cc3f95eca5a115e5e9531f660c4e2c"
  },
  {
    "热组件口径": "实际头部与底图容量小计",
    "导航与精排": "SQ8导航；自有FP32精排",
    "内存组件": "冻结SDK详情总长度匹配索引；实际主数组capacity=999424。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热",
    "未归因开销": "SDK已报告总量尚有84.2 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "e6b48dda310e4038d0b9c0baa9bba7c568099011ad9890f096df9b9b18105715",
    "protocol": "a89a8e4ae103b5b3badae3cb47f51b136ecfd002d453e5fc2198f93e75e0f6bc"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "FP32参与搜索",
    "内存组件": "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计",
    "未归因开销": "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值",
    "构建计时": "复用原构建构建适配层与官方库的新构建；包含训练、编码与添加，不含单列序列化/完整哈希",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "9ca4b26574963cae7cd48af7cb112fe68cd8ce149f44ae543cf8b7f30a0a1c0e",
    "protocol": "7946a1fa3d089640121d7f735781ac34ee0f31fc7094b71eb86fe47227d0ef7a"
  },
  {
    "result": "61c731ff82ba57d6360e149c1fa98a4c901b488733bf3104e845ced3d85bb184",
    "protocol": "8c18977492424bfe35c486c9b7ef7131d04a8b2182fe8e2ad3c4b3f7104c7543"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "量化搜索；无全库精排库",
    "内存组件": "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计",
    "未归因开销": "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值",
    "构建计时": "复用原构建构建适配层与官方库的新构建；包含训练、编码与添加，不含单列序列化/完整哈希",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": false,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "77a4670d36d6c334b85ca7333c289ea61e25e425b2d898bed235ee9e2e703aae",
    "protocol": "95e2e2260be9ee75a675671ba463906231e8d93b8018fa7b01c604d6c4de520f"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "量化导航；精排库独立计冷",
    "内存组件": "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计",
    "未归因开销": "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值",
    "构建计时": "复用原构建构建适配层与官方库的新构建；包含训练、编码与添加，不含单列序列化/完整哈希",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "2cba407e8ebb5317d0f688dce79e61acf8d87a94154bad8a1d6b707513bc9541",
    "protocol": "b3d693454f7cfb5016eb7dfe6bd7c6b25f47d8fb48eb83cb56bc0c608fedd932"
  },
  {
    "result": "9649b79219546cf622e612bb30105f02364ed395ece090d24a96cf61386e4193",
    "protocol": "cf3bdcfe316b79d71043e18403af81423f909e088a89debe44e21e1e544ad33d"
  },
  {
    "result": "11d6f9ecb029a2e989817828649011f7a87bffe1028761bb4d06c7bae1127bf5",
    "protocol": "a048324e7fe24aa9a133fcbf5e6f01f4f584c6a2b08016ede46c4e5d270855c4"
  },
  {
    "result": "8a7ae0ccb99e03211ea75b3af4eca48d22812c42c210caa48e473641d7adc961",
    "protocol": "655e69995e45cf52a2ed05d3cc4cd02b7f77d45f8e81c15a06e1ee21115870f7"
  },
  {
    "result": "bdcb4462b57c35c43d4661a7a53dffb9369a309280df1358fadbffeb65e36342",
    "protocol": "425f0838e643797dd35456008d1052076a4f24475153cc54e142a88a7456c04f"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "FP32参与导航；无全库冷精排向量",
    "内存组件": "含逐边局部二进制码、三个浮点因子和邻居ID；不是仅2-bit码与图",
    "未归因开销": "查询工作区、容器容量及分配器开销未完整拆分",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "08939754eb4cc9965a23ebba2f72545f59a3b73de47af8ee4616c921d9585279",
    "protocol": "98ee6c18fc709c8e8de954eeb033338dd116fb39c5e83b6a670fa10bbe85e873"
  },
  {
    "热组件口径": "图行与容器描述符下界",
    "导航与精排": "FP32参与导航",
    "内存组件": "逐行解析冻结图且核对完整SHA、行数和文件末尾；包括邻居ID与至少N个24字节vector描述符，FP32映射库用于导航",
    "未归因开销": "外层vector额外容量、查询boost bitset/候选队列及分配器/映射驻留另计；不把mmap大小当全部驻留",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "5b5b8095ed254989c3b5720d8f80a856dc684f98121977637ab473a4e2374462",
    "protocol": "4dc6264f0b93a99f8e9817bb7c5740698e7df79bb32066fea9958a49c9d7c58d"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "1-bit导航；外部FP32精排",
    "内存组件": "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计",
    "未归因开销": "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": false,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "3ab526a0016481c99e29fad40dfe2b9af586f778e5283d6da4237b6a9431715f",
    "protocol": "3a983d28a409018ae63d03acf14232f172d73d508e4025f9603f7587d9380844"
  },
  {
    "热组件口径": "对齐图与导航主数组小计",
    "导航与精排": "FP32参与图搜索",
    "内存组件": "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=192字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图",
    "未归因开销": "图锁、删除标记、PQ码本/实际容量、搜索队列/visited与分配器；主数组不是完整进程内存",
    "构建计时": "官方图插入total_time加PQ量化训练时长，二者单列；读入和保存额外含于原生进程墙钟，不将总进程时间冒充独立加载或保存API",
    "保存计时": "保存包含在原生进程墙钟内，现有记录不能分离出保存时长",
    "加载计时": "纯DiskANNIndex::load_with的异步API墙钟；不含CLI启动、输入检查、配置元数据读取、证据导出、查询及退出",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "ada5e3f61bdf0ae15c97c0a4e97b5eef513c99cbe75bbf7fda7b2a606f8aa34d",
    "protocol": "1e247562c4d0f78d8be26a9432924f1956cd9baec9b752a688237f108651a475"
  },
  {
    "热组件口径": "对齐图与导航主数组小计",
    "导航与精排": "PQ导航；自有FP32只用于最终精排",
    "内存组件": "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=192字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图",
    "未归因开销": "图锁、删除标记、PQ码本/实际容量、搜索队列/visited与分配器；主数组不是完整进程内存",
    "构建计时": "官方图插入total_time加PQ量化训练时长，二者单列；读入和保存额外含于原生进程墙钟，不将总进程时间冒充独立加载或保存API",
    "保存计时": "保存包含在原生进程墙钟内，现有记录不能分离出保存时长",
    "加载计时": "纯DiskANNIndex::load_with的异步API墙钟；不含CLI启动、输入检查、配置元数据读取、证据导出、查询及退出",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "496f9dd1543b42155c02e0b8a4d620f6be6cf8e7d179ccb660769a074e85f010",
    "protocol": "676ea811ded35cf8ae373ab999dd4fcbc9bb41d279a292850ab9133ec668bc6e"
  },
  {
    "result": "f906819d8c91107988da05af93eafc289072e1631dd65e8d01b4b04ecc352585",
    "protocol": "808f498181fe8865fdb87b48ead7f1d1e70608c9351cb7082cd61b15ca5454d5"
  },
  {
    "热组件口径": "文件头与加载数组小计",
    "导航与精排": "FP32参与导航",
    "内存组件": "含底层向量/边/标签、上层边、上层指针及层级数组；文件长度与加载源码交叉核算",
    "未归因开销": "仍缺mutex数组、label_lookup桶/节点、实际visited池数量、候选队列及分配器；visited每份2N字节",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "260205e5ad2f56a27bba793ef633dadb2a44b6c58f3986ade6b1ee419e6e7a1f",
    "protocol": "de3e2b148eb57487b1ad79aadf346c6a833318c1c54a83d0debde7e305bf348e"
  },
  {
    "result": "150226fb191026cdf9f05229b9797792d35714abe15211d9df932c6ac50ef96e",
    "protocol": "8db521927a24e4a6dd7f76752d2557afc0016dc4d77ad6092218e42d874c1360"
  },
  {
    "热组件口径": "作者库重载数组容量小计",
    "导航与精排": "FP32参与导航",
    "内存组件": "使用作者冻结FAISS1.8库，逐项读取FP32、层级、偏移、上层/底层边及层概率的size/capacity；不是拿其他FAISS版本代算",
    "未归因开销": "类对象、每线程VisitedTable/候选堆和分配器另计；诊断mallinfo2是整个进程glibc分配快照，不能当索引净增量",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "7fc39df5e7735b48f3b9778d1e3dfa60436c430ce146b79229f5ca358396d5d6",
    "protocol": "8dbcf7816283f2fb9d7ce0e1946fce454e2c0ecc98577fab776f16689d1bf168"
  },
  {
    "热组件口径": "量化图与树导航数组下界",
    "导航与精排": "树自有内部FP32 pivot选叶；局部量化图导航；全库FP32仅最终精排。叶pivot保留但SearchLeaf不读取。",
    "内存组件": "三库TreeQuantization=True、非classic路径已核实；树节点自有FP32 pivot单列热，全库原向量仅最终精排；所有qg/grp及tre解析长度严格匹配",
    "未归因开销": "仍需另计原图/叶pivot等加载保留、量化查表与旋转器、对象分配对齐、visited池/候选缓存；非查询必要保留不能算纯热",
    "构建计时": "ANNG、图重构及量化图创建/构建命令墙钟；命令内部已持久化，文件哈希另计",
    "保存计时": "保存已包含在构建命令或commit中；不另填0秒；保存包装列实际是组件哈希/清单时间",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "80637900de81220a1f8da6368d56d27b023e87ce4547c0b45cbb5e161dfad180",
    "protocol": "a4bb8ac306df7b73d0920acbcc1ff3f793f5350ff3a4bbea42e2870c7a35aa4c"
  },
  {
    "热组件口径": "加载源码主数组请求小计",
    "导航与精排": "量化导航；自有FP32精排",
    "内存组件": "9项文件逐组件复算全匹配；含32点块补齐、三因子/额外两因子、ID、中心、旋转与cluster描述符；32位为FP32精排模式",
    "未归因开销": "仍缺类对象、容器隐藏容量、分配器及逐查询旋转/LUT/中心候选/重排工作区",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "1b433549d36decece25d3cbc2733d1ae3527475b74cfa3188a56164fd89617e2",
    "protocol": "aa9346a81fec2dcdc4dc8831a209f006d47400c01d0a125e05594dfd4798514d"
  },
  {
    "热组件口径": "加载源码主数组请求小计",
    "导航与精排": "多位量化查询；无全库FP32精排库",
    "内存组件": "9项文件逐组件复算全匹配；含32点块补齐、三因子/额外两因子、ID、中心、旋转与cluster描述符；32位为FP32精排模式",
    "未归因开销": "仍缺类对象、容器隐藏容量、分配器及逐查询旋转/LUT/中心候选/重排工作区",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "687e7d8a07425089c7411c8b64d5b0c0edd80b61342ed274da2b680db650a5dd",
    "protocol": "42adfd648f3dc634a9f78d762a65949ecf1d8be88da875aba1ea127c1235fb9f"
  },
  {
    "result": "d496d4c06165a26d6fba626204a52cc7be24a76d108c3e027baba65dfb64baa0",
    "protocol": "566fbde10d707fc66fd9245a4920f36de196074d9a373bbbaf91bc3266aaced0"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "LVQ主码导航；无全库FP32",
    "内存组件": "包括对齐、缩放等实际布局；驻留量已加hugetlb",
    "未归因开销": "查询工作区、容器容量及分配器开销未完整拆分",
    "构建计时": "复用原官方LVQ量化与Vamana构建API实测墙钟；未重新建图",
    "保存计时": "保存包装另含组件哈希，原生API时间独立列示",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "e460a33fb18ead697b5ffb39ae2c483ae56538b727bb9e17c66beb0a9f2a95fb",
    "protocol": "0fdf8c0a42cb5bbab7158deb8986bbb3cddabf07e6473cf13434cdfcb7d50db6"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "主码导航；残差码仅最终重排；无全库FP32",
    "内存组件": "图搜索用PrimaryAccessor；随后get_datum重排才读残差。残差计逻辑冷，量化码总量含主码与残差；驻留已加hugetlb",
    "未归因开销": "查询工作区、容器容量及分配器开销未完整拆分",
    "构建计时": "官方LVQ量化和Vamana构建API墙钟；冻结源哈希、格式转换及保存独立记录",
    "保存计时": "保存包装另含组件哈希，原生API时间独立列示",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": false,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "4528b5d9e4b1b740629e03a1276a928b944709d2804eed7a11cd139b691114ec",
    "protocol": "f4f9b5652d5ea9fc7ffa97d659a268168cd75476fafaa00c184db72891fffc4c"
  },
  {
    "result": "645f686e865c897781e206cad02fc52f5765de6d4e111a765b6ec9e6bec22e0d",
    "protocol": "40b5c7853395566d00a0437e8d3aabd8eef53788d6d4376eddf2daa5b2dbd7f5"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "LVQ主码导航；无全库FP32",
    "内存组件": "包括对齐、缩放等实际布局；驻留量已加hugetlb",
    "未归因开销": "查询工作区、容器容量及分配器开销未完整拆分",
    "构建计时": "官方LVQ量化和Vamana构建API墙钟；冻结源哈希、格式转换及保存独立记录",
    "保存计时": "保存包装另含组件哈希，原生API时间独立列示",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "d800265bb71304119bbf499cec09595f325d790c75f791ca178ad53c88257233",
    "protocol": "1f6ad509d16adf524736c7fe557537dbd42457b80e7958b2573b27bb203f55a8"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "1-bit导航；外部FP32仅精排",
    "内存组件": "同版本只读工具实读码、上下层图、层级、偏移和中心容量；不含隐藏运行时开销",
    "未归因开销": "类对象、分配器和查询工作区仍另计",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "保存包装时钟；不把缺少的内层API时钟补造为0",
    "加载计时": "冻结库包装加载边界；外部向量映射按需触页",
    "冷加载确认": false,
    "驻留口径": "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值"
  },
  {
    "result": "82887e04c48e404de480042d8df0a6d3a3bfccb2c1d9d150691f1113724734f3",
    "protocol": "11369fbb02cdbddb1fd98aed41c1df51b5c7afa7e0c088e4e36e31acb8492ccc"
  },
  {
    "热组件口径": "自有tape已分配容量小计",
    "导航与精排": "FP32参与导航",
    "内存组件": "同版本memory_stats实读分配容量，包含预留与对齐空隙，不重复相加",
    "未归因开销": "节点指针/锁、key哈希、实际查询队列/visited、线程上下文及分配器仍另计",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "保存包装时钟；不把缺少的内层API时钟补造为0",
    "加载计时": "冻结库包装加载边界；外部向量映射按需触页",
    "冷加载确认": true,
    "驻留口径": "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值"
  },
  {
    "result": "c777ff38cc03db31ff6d2031f67eb09e79f1e978e5ed422d6773b247be4c29e2",
    "protocol": "2aa8b2198f9a7be6126e89187c7fb9bb0c628ebd811145dce6cff1e6d779d74b"
  },
  {
    "热组件口径": "持久数组小计，非完整热内存",
    "内存组件": "已计保留BQ、SM对齐行及幅值、共享图、两套ID/slot映射；索引文件不包含运行期SM派生状态。逻辑冷不等于未驻留。",
    "未归因开销": "原对象管理数组、哈希表及队列容量未全部逐对象归属，分配器与驻留另列；组件小计不冒充完整纯热内存",
    "构建计时": "八核原生BQ构图API；排除导入、保存与SM安装。构建单次，查询五轮。",
    "加载计时": "图加载API加重载SM安装；不是统一完全冷启动。",
    "驻留口径": "五轮查询后快照最大值；不是连续驻留峰值。",
    "冷组件": "原始FP32仅用于末尾精排；逻辑冷不等于未驻留。"
  },
  {
    "result": "de50d997ab119b75f2dd034dfd5e8eb94f666350e60801c929fb61418ca1d2d8",
    "protocol": "934c751300cb11be902bdcfa558fee9f2ba4be9713dda8a24a61812b8702692b",
    "index": "5112b9bbb5ae7dbde5e903c73d8f23a62c24100fc22fb55c30d12db881d3a561",
    "archive": "420bbc09a9f826f1f5c557a467c20cf249d828546f122422c69f6087c68c95ac"
  },
  {
    "吞吐边界": "Java批量查询核心墙钟，含任务派发、结果数组和标签复制；不含跨进程IPC、查询文件读入与结果文件写出，原始请求墙钟另记",
    "延迟边界": "Java单查询服务时长，含Lucene图查询与FP32精排，不含任务排队和IPC",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "783a22a08fa30bdaefecd03833da29d7dc9f983f087bb193f288b71fc37177a2",
    "protocol": "18fd561d746c13a695c586f1bc78db98c9b36a3de831126a61a9823ffdf70d11",
    "archive": "ceb79e2d5d4a00f515c5c14393c7416a54a3e34af6d198c90ff92079cbf1f05b"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含查询Dataset准备、官方搜索及标签复制；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "454589cc88bc0d74c11add458d25563d8447e0abc70cc30d21173888e2f18471",
    "protocol": "4453e5814fb42c0d24bf7ae385412554a0cc3f95eca5a115e5e9531f660c4e2c",
    "archive": "6ee22314edff740ef9f384c706c9426a34b47cd25764b2146d5b5955f7f72753"
  },
  {
    "result": "e6b48dda310e4038d0b9c0baa9bba7c568099011ad9890f096df9b9b18105715",
    "protocol": "a89a8e4ae103b5b3badae3cb47f51b136ecfd002d453e5fc2198f93e75e0f6bc",
    "archive": "b83d14e2ec0d9c9d4e2bb095df0c9a5f09ded7467e37e76f9eb2513882083570"
  },
  {
    "吞吐边界": "主qps为官方批量Index::search原生墙钟；service另列包装吞吐，包含分配/线程创建/标签验证及开启的逐查询时钟",
    "延迟边界": "逐查询服务API纳秒时钟；对应service路径，与官方批量QPS不是同一执行路径",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "9ca4b26574963cae7cd48af7cb112fe68cd8ce149f44ae543cf8b7f30a0a1c0e",
    "protocol": "7946a1fa3d089640121d7f735781ac34ee0f31fc7094b71eb86fe47227d0ef7a",
    "archive": "b1d0c01e642d06f3e9d661f2768a8a7f5cd312b82a82d49f7c07d1ecd3ac5d23"
  },
  {
    "result": "2106950a07a11a21232de96256ae5fbe3fc5f99d7fcedee71cb5f44157b4825f",
    "protocol": "8534498eb475bbb3b2df13e7527782ff79696105db13ab0a35dab59329136795",
    "archive": "e808da4d18203143471ad5e1afd302406e15a434e473a0a3a9759f85e9ed985f"
  },
  {
    "result": "61c731ff82ba57d6360e149c1fa98a4c901b488733bf3104e845ced3d85bb184",
    "protocol": "8c18977492424bfe35c486c9b7ef7131d04a8b2182fe8e2ad3c4b3f7104c7543",
    "archive": "fdc40f2b5c0504e149dbd2b6c73aa0e6e376081493281889a56737e60627494b"
  },
  {
    "result": "f38e8722ab95e89d04675c237646bfb4df54c3619ffe188b15b9c11407940161",
    "protocol": "5835856d30cc13003a672704f6291ce41be90c2b38679dc53bfe309db1258abd",
    "archive": "fa43de10972ebf703838c4601852a19c2643ace0f61e514557fc102221d46888"
  },
  {
    "result": "77a4670d36d6c334b85ca7333c289ea61e25e425b2d898bed235ee9e2e703aae",
    "protocol": "95e2e2260be9ee75a675671ba463906231e8d93b8018fa7b01c604d6c4de520f",
    "archive": "82a0f047c254e5d97954a85aa746931d2ebddc5af15c1829914198db2af5434f"
  },
  {
    "result": "6accf09d2b80f4ac6b310fbeed0a5bd44a628ff5071093ec27e916b3b96328aa",
    "protocol": "8a55cbd1781b8a588e0d5aa9a176ca10943bb4d3679357229641394066d6de1d",
    "archive": "2742b4349b0cfa629a5dc64b51edaa45803c92bbd5edd47830d00387bf2ebc34"
  },
  {
    "result": "2cba407e8ebb5317d0f688dce79e61acf8d87a94154bad8a1d6b707513bc9541",
    "protocol": "b3d693454f7cfb5016eb7dfe6bd7c6b25f47d8fb48eb83cb56bc0c608fedd932",
    "archive": "a502e8e9ea4108afd30b37c1fc1d5b8818102cfe5da36f46bf8aaa6f965e3a75"
  },
  {
    "result": "6bd63e8bc11ea22a528a235953bf7777f6bb64760cdfc710084944139f57c306",
    "protocol": "917658060d853ae3c04cb1e00df3263d6b8d1c3b0ee8ff145a2350c14ab20ba5",
    "archive": "d16ca211853598539854e1d21e33598f33266dceb7cc412878de31ae653ac2a7"
  },
  {
    "result": "9649b79219546cf622e612bb30105f02364ed395ece090d24a96cf61386e4193",
    "protocol": "cf3bdcfe316b79d71043e18403af81423f909e088a89debe44e21e1e544ad33d",
    "archive": "dbab7e5f4a9b3362f1b31807cf3cf4a1a556aef18b36db3cdd470e62a1833838"
  },
  {
    "result": "df6d8d613e5e33a1477881c258989c7123c3cb3962f76ff48db0f88a813914d3",
    "protocol": "bf07744cfebdef551986c064b41c114d8f765d878847bf078b1aabdc1e9bac6b",
    "archive": "fd7ab8aec121e198b03f6be3de8e2a00c63e33dcf632893a33bd8e5783c6db7b"
  },
  {
    "result": "11d6f9ecb029a2e989817828649011f7a87bffe1028761bb4d06c7bae1127bf5",
    "protocol": "a048324e7fe24aa9a133fcbf5e6f01f4f584c6a2b08016ede46c4e5d270855c4",
    "archive": "a258a50dc3ed959d1aae44c06a2d6ea9c1560b54b835598cd76431abf4c296e2"
  },
  {
    "result": "695d132c63a7cf90398896d90d0e7479a2b561f1978a38791afce38a058b93ce",
    "protocol": "4503c4c321a179027440f918fd4c1293158b1feba737b40df0ad694637ab11a6",
    "archive": "006f239c49a2ac7a08b9522498314bca8445d3028af2a9fd12cbfdf14dfccbed"
  },
  {
    "result": "8a7ae0ccb99e03211ea75b3af4eca48d22812c42c210caa48e473641d7adc961",
    "protocol": "655e69995e45cf52a2ed05d3cc4cd02b7f77d45f8e81c15a06e1ee21115870f7",
    "archive": "435617892657440e6aa4aa5985afb413b95e4fd2730cffa36f74ee1da2b6e5cd"
  },
  {
    "result": "4fa6a4e8d787233d9cffbc4a8c56311e0bc97949761d5b23eba9cdf668e0a74f",
    "protocol": "48edf6da0beb70776c765e249a608a7d754332728fa1d3e8c67ef0ad18d6ba95",
    "archive": "4aedb0e7fa788fafa22e07df458c29afe1ee96dbe003c32bfbabd36f3a88e54e"
  },
  {
    "result": "bdcb4462b57c35c43d4661a7a53dffb9369a309280df1358fadbffeb65e36342",
    "protocol": "425f0838e643797dd35456008d1052076a4f24475153cc54e142a88a7456c04f",
    "archive": "0b4742af48db6aa772fd70a7cb95d2f61bc16c071681c5919bf2e69727ad34d2"
  },
  {
    "result": "403e308e67a379e744c9120422797c1900893a9b703530dca7d6beb4252cca45",
    "protocol": "3eb7ff6107e94109f19745f697d0d04f9cf11122bfc7f92cd63d7ad6724c3f8c",
    "archive": "243db964f993aba8d9987dc29cc5a3d0c81973ae1e6bd6321fd4fa7aafbc7b50"
  },
  {
    "result": "08939754eb4cc9965a23ebba2f72545f59a3b73de47af8ee4616c921d9585279",
    "protocol": "98ee6c18fc709c8e8de954eeb033338dd116fb39c5e83b6a670fa10bbe85e873",
    "archive": "5b016db97dfa07fdbb9799f430ce3c53f0a405e7173d082726ae2ecfac5760cb"
  },
  {
    "result": "52ee36842766512ce462166e241a69e68caf31f9ad962e4d6e08ce7333b99348",
    "protocol": "71fbc0241c2fe826a96e30cfeeb6e2d921c7098d5a730f454aba65d0d8cf029a",
    "archive": "24ae2b45553144a5d63b5a587f3b3ae7fac5855395de4cdd3c65e396cb6600e2"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "5b5b8095ed254989c3b5720d8f80a856dc684f98121977637ab473a4e2374462",
    "protocol": "4dc6264f0b93a99f8e9817bb7c5740698e7df79bb32066fea9958a49c9d7c58d",
    "archive": "004330e2c0b9269727c6ec8a029d975a40ec3e15f4b6cbfed0fefa26e9240683"
  },
  {
    "result": "45623bcadbcd37a72de996124acb5a9f219f83d33ebdfc77891ba277c54a171e",
    "protocol": "952e1954d261b527388f75675442e46096423e4dd08618b28780d3d402786ba6",
    "archive": "1254b772eeaef573b11aeb90d1523215ea77ba1825cd5a3c2075d71c00e6d6d5"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含查询Dataset准备、官方搜索及标签复制；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "3ab526a0016481c99e29fad40dfe2b9af586f778e5283d6da4237b6a9431715f",
    "protocol": "3a983d28a409018ae63d03acf14232f172d73d508e4025f9603f7587d9380844",
    "archive": "5fb3dc3bdad7bd42412b58da5eaad995d066cf06dee72a854a75ec0a3bc165fb"
  },
  {
    "吞吐边界": "原生每轮重复官方批API的墙钟，含任务调度、结果分配与释放；逐查询时钟关闭；不含加载、文件IO导出和真值计算",
    "延迟边界": "官方逐查询服务整数微秒，单批完整查询五轮，含搜索与编号缓冲写入，不含排队及Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "ada5e3f61bdf0ae15c97c0a4e97b5eef513c99cbe75bbf7fda7b2a606f8aa34d",
    "protocol": "1e247562c4d0f78d8be26a9432924f1956cd9baec9b752a688237f108651a475",
    "archive": "13d2320d3edc3d8880ba8b302e311ddc2203dba2476733553ee7873999ca0fe9"
  },
  {
    "吞吐边界": "原生每轮重复官方批API的墙钟，含任务调度、结果分配与释放；逐查询时钟关闭；不含加载、文件IO导出和真值计算",
    "延迟边界": "官方逐查询服务整数微秒，单批完整查询五轮，含搜索与编号缓冲写入，不含排队及Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "496f9dd1543b42155c02e0b8a4d620f6be6cf8e7d179ccb660769a074e85f010",
    "protocol": "676ea811ded35cf8ae373ab999dd4fcbc9bb41d279a292850ab9133ec668bc6e",
    "archive": "6ea7fe63227fabeeaaf8a0b6d4a9f4e3c74e10c46804c45855da15fc18d83bb1"
  },
  {
    "result": "f906819d8c91107988da05af93eafc289072e1631dd65e8d01b4b04ecc352585",
    "protocol": "808f498181fe8865fdb87b48ead7f1d1e70608c9351cb7082cd61b15ca5454d5",
    "archive": "807ad2dd2e5159a22a526f40faadbf93659651c7e4bc78d5134950fc8daef66c"
  },
  {
    "result": "260205e5ad2f56a27bba793ef633dadb2a44b6c58f3986ade6b1ee419e6e7a1f",
    "protocol": "de3e2b148eb57487b1ad79aadf346c6a833318c1c54a83d0debde7e305bf348e",
    "archive": "65469b96ac7bcd6813db547ad98d5260620416a9eaae922ad41a1e3c688c2e35"
  },
  {
    "result": "150226fb191026cdf9f05229b9797792d35714abe15211d9df932c6ac50ef96e",
    "protocol": "8db521927a24e4a6dd7f76752d2557afc0016dc4d77ad6092218e42d874c1360",
    "archive": "c5d248c90f02f288f4de8ee42f69d2f4f928b8dad304e4db8546f30e66750e90"
  },
  {
    "result": "c1638eac07328669c714077a2c050f6e5928f1a0cb248c709db13ec567cc2a99",
    "protocol": "9322909e535a02612a112dd8ed957c1896d28a2332a081adafe31f64db2f57da",
    "archive": "6480a9d6155f11f77a90fa224634f21d31b87c95962e270b998d114beebad82d"
  },
  {
    "result": "7fc39df5e7735b48f3b9778d1e3dfa60436c430ce146b79229f5ca358396d5d6",
    "protocol": "8dbcf7816283f2fb9d7ce0e1946fce454e2c0ecc98577fab776f16689d1bf168",
    "archive": "a923f302386177ff2fffc8adc92951fca520d2f2909e09d5d04902473008520c"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "unsupported_author_global_statistics",
    "线程环境待核": false
  },
  {
    "吞吐边界": "官方NGT-QG搜索的OpenMP批量包装墙钟，含数组分配、跨语言、批量调度、查询准备与标签复制；逐查询时钟关闭",
    "延迟边界": "既有NGT正式原生包装：单查询准备、官方QG搜索到标签复制；不含排队或Python，只有冻结锚点开启时钟",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "80637900de81220a1f8da6368d56d27b023e87ce4547c0b45cbb5e161dfad180",
    "protocol": "a4bb8ac306df7b73d0920acbcc1ff3f793f5350ff3a4bbea42e2870c7a35aa4c",
    "archive": "b80a66de7785423d5f3c7e6cef3529508a08e2eee1575c0a927a6e513e126375"
  },
  {
    "吞吐边界": "官方NGT-QG搜索的OpenMP批量包装墙钟，含数组分配、跨语言、批量调度、查询准备与标签复制；逐查询时钟关闭",
    "延迟边界": "既有NGT正式原生包装：单查询准备、官方QG搜索到标签复制；不含排队或Python，只有冻结锚点开启时钟",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "1b433549d36decece25d3cbc2733d1ae3527475b74cfa3188a56164fd89617e2",
    "protocol": "aa9346a81fec2dcdc4dc8831a209f006d47400c01d0a125e05594dfd4798514d",
    "archive": "592951dc90d7a62436f552262197bc6321c7a52a82c3a97fe7f12e71155c156f"
  },
  {
    "result": "687e7d8a07425089c7411c8b64d5b0c0edd80b61342ed274da2b680db650a5dd",
    "protocol": "42adfd648f3dc634a9f78d762a65949ecf1d8be88da875aba1ea127c1235fb9f",
    "archive": "35d0ec5556164546cb07498e3626ab5e0d3948743b5fa1f2e2d9e22274650c4d"
  },
  {
    "result": "d496d4c06165a26d6fba626204a52cc7be24a76d108c3e027baba65dfb64baa0",
    "protocol": "566fbde10d707fc66fd9245a4920f36de196074d9a373bbbaf91bc3266aaced0",
    "archive": "4ba54fb965b66f50c5c08cba2df4b95d3ce74c55c7146bd3b6e1772070a9e223"
  },
  {
    "吞吐边界": "官方SVS Python批量API包装墙钟，含结果分配、缓存参数与编号核验；逐查询时钟关闭，官方常驻线程池",
    "延迟边界": "SVS官方SDK原生逐查询计时未支持，所有P95/P99明确保留缺项",
    "延迟可用性": "unsupported_official_sdk_native_latency",
    "线程环境待核": false
  },
  {
    "result": "e460a33fb18ead697b5ffb39ae2c483ae56538b727bb9e17c66beb0a9f2a95fb",
    "protocol": "0fdf8c0a42cb5bbab7158deb8986bbb3cddabf07e6473cf13434cdfcb7d50db6",
    "archive": "147f97993d7a35bd8b36cde00e96d3a02d609152c376b7acc1619b9332edfa37"
  },
  {
    "result": "389beb2fff8de0a0bca25caa961ea6a22126302a0cec7973cce09bf4e0a6a2e5",
    "protocol": "af0ef218b99d1496854b8c4c7f51ef77cbe5802127934b9998f9d6a0a6ec0c9d",
    "archive": "14ca12eedde54b5eaf873e0f1c00b556371df28e808b2e0853482c9dec84db28"
  },
  {
    "吞吐边界": "官方SVS Python批量API包装墙钟，含结果分配、缓存参数与编号核验；逐查询时钟关闭，官方常驻线程池",
    "延迟边界": "SVS官方SDK原生逐查询计时未支持，所有P95/P99明确保留缺项",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "4528b5d9e4b1b740629e03a1276a928b944709d2804eed7a11cd139b691114ec",
    "protocol": "f4f9b5652d5ea9fc7ffa97d659a268168cd75476fafaa00c184db72891fffc4c",
    "archive": "6f73e715c4ff12f2d8ecfa6f5e53caf7c111a5e2bbece3775beb0b1d7b215427"
  },
  {
    "result": "645f686e865c897781e206cad02fc52f5765de6d4e111a765b6ec9e6bec22e0d",
    "protocol": "40b5c7853395566d00a0437e8d3aabd8eef53788d6d4376eddf2daa5b2dbd7f5",
    "archive": "dd13120ec3979d84a39b0105a4ebc69b74e5bc6f04fc7361ad219c0241546bf1"
  },
  {
    "result": "d800265bb71304119bbf499cec09595f325d790c75f791ca178ad53c88257233",
    "protocol": "1f6ad509d16adf524736c7fe557537dbd42457b80e7958b2573b27bb203f55a8",
    "archive": "c93ddfdb4925e25cee3a757f0874eb686be2ae08c4e2af360354083a32f34ad3"
  },
  {
    "result": "82887e04c48e404de480042d8df0a6d3a3bfccb2c1d9d150691f1113724734f3",
    "protocol": "11369fbb02cdbddb1fd98aed41c1df51b5c7afa7e0c088e4e36e31acb8492ccc",
    "archive": "2e45e52e2f8b6986dcc23c2765ddc0284b1be80423e22b93e6b42660eeb79bcc"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方cosine搜索及编号/距离复制；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "c777ff38cc03db31ff6d2031f67eb09e79f1e978e5ed422d6773b247be4c29e2",
    "protocol": "2aa8b2198f9a7be6126e89187c7fb9bb0c628ebd811145dce6cff1e6d779d74b",
    "archive": "5516156e449ce95ed9f87ec87a3e96f9be2c9ca5da44609c4fb9481183efa9fd"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方cosine搜索及编号/距离复制；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "预算修正": "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。",
    "参数选择": "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。",
    "原BQ对照": "原QPS 4229.450117552；原Recall@10 0.834595648604；原证据身份 de50d997ab119b75f2dd034dfd5e8eb94f666350e60801c929fb61418ca1d2d8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。"
  },
  {
    "result": "502fcf8be8c5637c2e47e077ec99674a6ee1c3443d093689c257b2f9d015b072",
    "protocol": "654c8223e62a39afa6b0b7195640574d769dbf67991f93e3653366881ddb7f35",
    "archive": "d54ebd09dcf93ebf441324011ac36e56e6144ec1090e1fb06f4b03b534b9c3d1",
    "index": "5112b9bbb5ae7dbde5e903c73d8f23a62c24100fc22fb55c30d12db881d3a561"
  },
  {
    "吞吐边界": "原生请求完整搜索与精排；五轮均值。",
    "延迟边界": "单查询服务时钟，五轮完整评估查询。",
    "适用范围": null,
    "低宽度路由": null
  }
];
