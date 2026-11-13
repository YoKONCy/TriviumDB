// 英文记录文案，中文原文作为稳定查找键。
window.QuiverLocaleData = window.QuiverLocaleData || { en: {} };
Object.assign(window.QuiverLocaleData.en, {
  "冻结jar字节码与元数据SHA交叉核对：每点1-bit码另带16字节校正项；包括实际压缩边、地址数据/元数组、上层ID、centroid及docID映射。索引内.vec与外部FP32两份部署文件保留":
    "Cross-checked frozen JAR bytecode and metadata SHA. Each point stores a 1-bit code and 16 correction bytes. Includes compressed edges, address data and metadata arrays, upper-layer IDs, the centroid and docID mapping. Deployment retains both the index .vec file and external FP32 vectors.",
  "Java对象头、数组头/对齐、DirectoryReader/线程缓存、GC运行时与映射实际驻留单列":
    "Java object and array headers, alignment, DirectoryReader and thread caches, GC runtime and mapped-page residency are accounted for separately.",
  "Java输入校验、插入、量化图合并、commit及writer关闭；组件哈希与外部精排库准备另计":
    "Java input validation, insertion, quantized-graph merging, commit and writer close. Component hashing and external reranking-data preparation are timed separately.",
  "Java内部从FP32精排库读入至服务就绪：含两次显式GC、DirectoryReader/IndexSearcher及docID映射；排除JVM启动。不是单独DirectoryReader.open时长。":
    "Java-internal elapsed time from reading FP32 reranking vectors to service readiness, including two explicit GC calls, DirectoryReader/IndexSearcher and docID mapping. JVM startup is excluded. This boundary extends beyond DirectoryReader.open.",
  "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。":
    "Maximum recorded post-query snapshot. FAISS configurations without point-level snapshots retain the post-load or admission snapshot. These observations are discrete snapshots, with no interpolated values.",
  "冻结SDK详情总长度匹配索引；实际主数组capacity=1015808。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热":
    "The frozen SDK detail total matches the index. Actual primary-array capacity is 1015808. Each base-graph row includes a count. Reserved reranking capacity is included in cold components. The SDK total also includes other components.",
  "SDK已报告总量尚有85.3 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合":
    "The SDK reports another 85.3 MiB outside the primary arrays, including the upper graph, locks and label_remap. Allocator and workspace accounting remains incomplete.",
  "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计":
    "Official build-API wall time in a new process, including the first access to training mmap data. Index serialization and complete input hashing are recorded separately.",
  "SDK已报告总量尚有85.5 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合":
    "The SDK reports another 85.5 MiB outside the primary arrays, including the upper graph, locks and label_remap. Allocator and workspace accounting remains incomplete.",
  "冻结SDK详情总长度匹配索引；实际主数组capacity=999424。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热":
    "The frozen SDK detail total matches the index. Actual primary-array capacity is 999424. Each base-graph row includes a count. Reserved reranking capacity is included in cold components. The SDK total also includes other components.",
  "SDK已报告总量尚有84.2 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合":
    "The SDK reports another 84.2 MiB outside the primary arrays, including the upper graph, locks and label_remap. Allocator and workspace accounting remains incomplete.",
  "冻结SDK详情总长度匹配索引；实际主数组capacity=5013504。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热":
    "The frozen SDK detail total matches the index. Actual primary-array capacity is 5013504. Each base-graph row includes a count. Reserved reranking capacity is included in cold components. The SDK total also includes other components.",
  "SDK已报告总量尚有422.7 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合":
    "The SDK reports another 422.7 MiB outside the primary arrays, including the upper graph, locks and label_remap. Allocator and workspace accounting remains incomplete.",
  "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计":
    "Read size/capacity for individual arrays, including upper-layer edges and offsets, inverted-list directories, FastScan block padding and precomputed tables using their actual types. Values come from loaded arrays.",
  "复用原构建构建适配层与官方库的新构建；包含训练、编码与添加，不含单列序列化/完整哈希":
    "A new build using the original adaptation layer and official library. Includes training, encoding and add. Separately recorded serialization and full hashing are excluded.",
  "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=192字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图":
    "Cross-checked frozen loader source and the 24-byte header. Graph rows are padded to 64-byte boundaries with an actual stride of 192 bytes, including one entry point and trailing padding. In-memory graph storage is computed from this layout.",
  "图锁、删除标记、PQ码本/实际容量、搜索队列/visited与分配器；主数组不是完整进程内存":
    "Graph locks, deletion flags, PQ codebooks and actual capacities, candidate queues, visited state and allocator overhead remain separate. Primary arrays cover part of process memory.",
  "纯DiskANNIndex::load_with的异步API墙钟；不含CLI启动、输入检查、配置元数据读取、证据导出、查询及退出":
    "Asynchronous DiskANNIndex::load_with API wall time. Excludes CLI startup, input checks, configuration metadata, evidence export, queries and shutdown.",
  "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=384字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图":
    "Cross-checked frozen loader source and the 24-byte header. Graph rows are padded to 64-byte boundaries with an actual stride of 384 bytes, including one entry point and trailing padding. In-memory graph storage is computed from this layout.",
  "官方图插入total_time加PQ量化训练时长，二者单列；读入和保存额外含于原生进程墙钟，不将总进程时间冒充独立加载或保存API":
    "Official graph-insertion total_time plus PQ training time, also listed separately. Native-process wall time additionally contains reads and saving, so the full process interval cannot isolate those APIs.",
  "逐行解析冻结图且核对完整SHA、行数和文件末尾；包括邻居ID与至少N个24字节vector描述符，FP32映射库用于导航":
    "Parsed every frozen graph row and verified the complete SHA, row count and end of file. Includes neighbor IDs and at least N 24-byte vector descriptors. The mapped FP32 store participates in navigation.",
  "外层vector额外容量、查询boost bitset/候选队列及分配器/映射驻留另计；不把mmap大小当全部驻留":
    "Additional outer-vector capacity, query boost bitsets and candidate queues, allocator overhead and mapped-page residency are separate. Mapping size and residency are distinct measurements.",
  "仍缺mutex数组、label_lookup桶/节点、实际visited池数量、候选队列及分配器；visited每份2N字节":
    "Mutex arrays, label_lookup buckets and nodes, actual visited-pool count, candidate queues and allocator overhead remain separate. Each visited array uses 2N bytes.",
  "使用作者冻结FAISS1.8库，逐项读取FP32、层级、偏移、上层/底层边及层概率的size/capacity；不是拿其他FAISS版本代算":
    "Using the author's frozen FAISS 1.8 library, read size/capacity for FP32 data, levels, offsets, upper/base edges and level probabilities. All values use that same library version.",
  "类对象、每线程VisitedTable/候选堆和分配器另计；诊断mallinfo2是整个进程glibc分配快照，不能当索引净增量":
    "Class objects, per-thread VisitedTable and candidate heaps, and allocator overhead are separate. Diagnostic mallinfo2 describes process-wide glibc allocations rather than the index's incremental allocation.",
  "树自有内部FP32 pivot选叶；局部量化图导航；全库FP32仅最终精排。叶pivot保留但SearchLeaf不读取。":
    "Internal FP32 tree pivots select leaves, followed by local quantized-graph navigation. Full-database FP32 is used for final reranking. Leaf pivots remain loaded but SearchLeaf does not read them.",
  "三库TreeQuantization=True、非classic路径已核实；树节点自有FP32 pivot单列热，全库原向量仅最终精排；所有qg/grp及tre解析长度严格匹配":
    "Verified TreeQuantization=True and the non-classic path on all three datasets. Internal tree-owned FP32 pivots are hot, while original full-database vectors are used for final reranking. Parsed lengths of all qg/grp and tre files match exactly.",
  "仍需另计原图/叶pivot等加载保留、量化查表与旋转器、对象分配对齐、visited池/候选缓存；非查询必要保留不能算纯热":
    "Retained original-graph and leaf-pivot storage, quantization tables and rotators, object alignment, visited pools and candidate caches remain separate. Retained storage has a distinct role from required navigation components.",
  "保存已包含在构建命令或commit中；不另填0秒；保存包装列实际是组件哈希/清单时间":
    "Persistence is included in the build command or commit. The recorded save-wrapper column measures component hashing and manifest creation, with no fabricated zero-second API value.",
  "9项文件逐组件复算全匹配；含32点块补齐、三因子/额外两因子、ID、中心、旋转与cluster描述符；32位为FP32精排模式":
    "Independent component calculations match all nine files. Includes 32-point block padding, three factors plus two extra factors, IDs, centroids, rotation and cluster descriptors. The 32-bit mode uses FP32 reranking.",
  "图搜索用PrimaryAccessor；随后get_datum重排才读残差。残差计逻辑冷，量化码总量含主码与残差；驻留已加hugetlb":
    "Graph search uses PrimaryAccessor. Residuals are read later by get_datum for reranking. Residuals are logically cold, while total quantized storage includes primary and residual codes. Resident memory includes hugetlb.",
  "官方同版本memory_stats实测：未用预留1.322 GiB、对齐/旧arena空隙47.6 MiB，均已包含在分配容量中，不能再相加；未触碰页不等于驻留":
    "Official same-version memory_stats reports 1.322 GiB unused reserve and 47.6 MiB alignment/old-arena gaps, already included in allocated capacity. Untouched pages have a separate residency status.",
  "节点指针/锁、向量lookup与key哈希、实际查询队列/visited、线程上下文及自有mmap另计；mallinfo2不覆盖自有mmap":
    "Node pointers and locks, vector lookup and key hashes, actual candidate/visited queues, thread contexts and internal mmap remain separate. mallinfo2 excludes internal mmap.",
  "官方同版本memory_stats实测：未用预留1.191 GiB、对齐/旧arena空隙47.4 MiB，均已包含在分配容量中，不能再相加；未触碰页不等于驻留":
    "Official same-version memory_stats reports 1.191 GiB unused reserve and 47.4 MiB alignment/old-arena gaps, already included in allocated capacity. Untouched pages have a separate residency status.",
  "官方同版本memory_stats实测：未用预留1.765 GiB、对齐/旧arena空隙237.2 MiB，均已包含在分配容量中，不能再相加；未触碰页不等于驻留":
    "Official same-version memory_stats reports 1.765 GiB unused reserve and 237.2 MiB alignment/old-arena gaps, already included in allocated capacity. Untouched pages have a separate residency status.",
  "节点指针/锁、key哈希、实际查询队列/visited、线程上下文及分配器仍另计":
    "Node pointers and locks, key hashes, actual candidate/visited queues, thread contexts and allocator overhead remain separate.",
  "已核完整组件SHA及文件头；主码含对齐/缩放、图含容量、centroid计热；剔除文件头":
    "Verified complete component SHA and headers. Primary codes include alignment/scaling, the graph includes capacity, and centroids are hot. File headers are excluded.",
  "完整SHA、逐行度数、行数和文件末尾核对通过；含邻居ID及至少每行24字节vector描述符":
    "Verified complete SHA, per-row degree, row count and end of file. Includes neighbor IDs and at least one 24-byte vector descriptor per row.",
  "外层vector额外容量、boost bitset、候选队列、分配器及映射驻留另计":
    "Additional outer-vector capacity, boost bitsets, candidate queues, allocator overhead and mapped-page residency remain separate.",
  "逐行解析完整图且核对SHA、行数和文件末尾；邻居ID与至少每行24字节vector描述符计热，完整FP32用于导航":
    "Parsed the full graph row by row and verified SHA, row count and end of file. Neighbor IDs and at least one 24-byte vector descriptor per row are hot, while full FP32 vectors participate in navigation.",
  "外层vector额外容量、实际查询boost bitset/候选队列、分配器及映射驻留另计；映射回收不减少逻辑导航组件":
    "Additional outer-vector capacity, actual query boost bitsets and candidate queues, allocator overhead and mapped-page residency remain separate. Reclaiming mapped pages preserves the logical navigation components.",
  "保存时钟包围仅委托原生保存的函数，包含Python包装/绑定；不是更窄的C++内部时钟，完整哈希在计时外":
    "The timer surrounds a function delegating to native saving, including Python wrapper/binding overhead. Complete hashing is outside the interval. The boundary extends beyond the internal C++ timer.",
  "包装创建图及FP32映射的墙钟；图文件和导航FP32均先驱逐并核0驻留，映射按需触页，非全库预载":
    "Wrapper wall time for graph creation and FP32 mapping. The graph file and navigation FP32 were evicted and verified at zero residency first. Mapped pages are touched on demand.",
  "已计保留BQ、SM对齐行及幅值、共享图、两套ID/slot映射；索引文件不包含运行期SM派生状态。逻辑冷不等于未驻留。":
    "Includes retained BQ, aligned SM rows and magnitudes, the shared graph, and two ID/slot maps. The index file excludes runtime-derived SM state. Logically cold vectors may remain resident.",
  "原对象管理数组、哈希表及队列容量未全部逐对象归属，分配器与驻留另列；组件小计不冒充完整纯热内存":
    "Management arrays, hash tables and queue capacities are incompletely attributed per object. Allocator overhead and residency are listed separately. The component subtotal covers the confirmed arrays.",
  "原生每轮重复官方批API的墙钟，含任务调度、结果分配与释放；逐查询时钟关闭；不含加载、文件IO导出和真值计算":
    "Wall time for each round of the official batch API, including scheduling and result allocation/deallocation. Per-query timers are disabled. Loading, file export and ground-truth computation are excluded.",
  "官方逐查询服务整数微秒，单批完整查询五轮，含搜索与编号缓冲写入，不含排队及Python":
    "Official per-query service timer in integer microseconds. Five rounds of complete queries, including search and ID-buffer writes. Queuing and Python are excluded.",
  "官方原生批量查询计时，首轮预热排除；原生逐查询时钟仍开启；进程启动、加载、导出不计入QPS":
    "Official native batch-query timer with the first warm-up round excluded. Native per-query timers remain enabled. Process startup, loading and export are excluded from QPS.",
  "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭":
    "Wall time of Python calling C++ batch search, including thread creation, returned arrays and cross-language overhead. Per-query timers are disabled.",
  "C++内单查询服务时长，含查询Dataset准备、官方搜索及标签复制；不含应用排队和Python":
    "C++ per-query service time, including query Dataset preparation, official search and label copying. Application queuing and Python are excluded.",
  "主qps为官方批量Index::search原生墙钟；service另列包装吞吐，包含分配/线程创建/标签验证及开启的逐查询时钟":
    "Primary QPS uses official batch Index::search native wall time. The separate service-wrapper throughput includes allocation, thread creation, label verification and enabled per-query timers.",
  "逐查询服务API纳秒时钟；对应service路径，与官方批量QPS不是同一执行路径":
    "Nanosecond per-query service-API timer for the service path. Official batch QPS uses a separate execution path.",
  "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python":
    "C++ per-query service time, including official single-query search and ID/distance writes. Application queuing and Python are excluded.",
  "Java批量查询核心墙钟，含任务派发、结果数组和标签复制；不含跨进程IPC、查询文件读入与结果文件写出，原始请求墙钟另记":
    "Java batch-query core wall time, including task dispatch, result arrays and label copying. Cross-process IPC, query-file reads and result-file writes are excluded. Original request wall time is recorded separately.",
  "Java单查询服务时长，含Lucene图查询与FP32精排，不含任务排队和IPC":
    "Java per-query service time, including Lucene graph search and FP32 reranking. Task queuing and IPC are excluded.",
  "官方NGT-QG搜索的OpenMP批量包装墙钟，含数组分配、跨语言、批量调度、查询准备与标签复制；逐查询时钟关闭":
    "OpenMP batch-wrapper wall time for official NGT-QG search, including allocation, cross-language calls, scheduling, query preparation and label copying. Per-query timers are disabled.",
  "既有NGT正式原生包装：单查询准备、官方QG搜索到标签复制；不含排队或Python，只有冻结锚点开启时钟":
    "Existing formal NGT native wrapper, from single-query preparation through official QG search to label copying. Queuing and Python are excluded. Only frozen anchor points enable this timer.",
  "官方SVS Python批量API包装墙钟，含结果分配、缓存参数与编号核验；逐查询时钟关闭，官方常驻线程池":
    "Official SVS Python batch-API wrapper wall time, including result allocation, cached parameters and ID verification. Per-query timers are disabled. Uses the official persistent thread pool.",
  "C++内单查询服务时长，含官方cosine搜索及编号/距离复制；不含应用排队和Python":
    "C++ per-query service time, including official cosine search and ID/distance copying. Application queuing and Python are excluded.",
  "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。":
    "Same graph, binary, ef and complete queries. Only the requested reranking budget changes to min(64,ef). Direct SM-reference and four-thread consistency checks passed.",
  "原QPS 5144.822160016；原Recall@10 0.925940860215；原证据身份 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5144.822160016, Recall@10 0.925940860215, evidence identity 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 36518.807143891；原Recall@10 0.925940860215；原证据身份 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 36518.807143891, Recall@10 0.925940860215, evidence identity 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 58973.200677821；原Recall@10 0.925940860215；原证据身份 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 58973.200677821, Recall@10 0.925940860215, evidence identity 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 81801.249739643；原Recall@10 0.925940860215；原证据身份 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 81801.249739643, Recall@10 0.925940860215, evidence identity 40b16a7e4be977f35be29639514ef29aee2e939631bd9ca616d3da82a5404fc6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 4229.450117552；原Recall@10 0.834595648604；原证据身份 de50d997ab119b75f2dd034dfd5e8eb94f666350e60801c929fb61418ca1d2d8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 4229.450117552, Recall@10 0.834595648604, evidence identity de50d997ab119b75f2dd034dfd5e8eb94f666350e60801c929fb61418ca1d2d8. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 7851.608464215；原Recall@10 0.780510752688；原证据身份 85296063936997f64e32b6a52914d2e664f9609b99a21a887d089d15893fdaa7。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 7851.608464215, Recall@10 0.780510752688, evidence identity 85296063936997f64e32b6a52914d2e664f9609b99a21a887d089d15893fdaa7. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 8640.300595054；原Recall@10 0.627284946237；原证据身份 363217da0dbc494a3028843210ef14fe401a6cc51d99e55270380b9c1a0a4eb2。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 8640.300595054, Recall@10 0.627284946237, evidence identity 363217da0dbc494a3028843210ef14fe401a6cc51d99e55270380b9c1a0a4eb2. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 4245.070928694；原Recall@10 0.929032258065；原证据身份 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 4245.070928694, Recall@10 0.929032258065, evidence identity 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 29292.814555776；原Recall@10 0.929032258065；原证据身份 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 29292.814555776, Recall@10 0.929032258065, evidence identity 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 45584.523859461；原Recall@10 0.929032258065；原证据身份 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 45584.523859461, Recall@10 0.929032258065, evidence identity 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 55337.640427050；原Recall@10 0.929032258065；原证据身份 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 55337.640427050, Recall@10 0.929032258065, evidence identity 7a76379d1d86bffcb6beb73fdae8d2d3a7b8f699692f2e5c688dae351a3a9def. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 2259.922765975；原Recall@10 0.933713054187；原证据身份 ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 2259.922765975, Recall@10 0.933713054187, evidence identity ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 14142.156724410；原Recall@10 0.933713054187；原证据身份 ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 14142.156724410, Recall@10 0.933713054187, evidence identity ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 17766.431879566；原Recall@10 0.933713054187；原证据身份 ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 17766.431879566, Recall@10 0.933713054187, evidence identity ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 18213.977054449；原Recall@10 0.933713054187；原证据身份 ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 18213.977054449, Recall@10 0.933713054187, evidence identity ddca1f4eb4bc0a5bb2cf39908b4264df34ee411233024fe8d9ee2e5df75e9448. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 7075.515723029；原Recall@10 0.560324302135；原证据身份 950f70d9c3fd4a747213519c7d3053a46c434a29ea4aa6e7d1f2d20d4079e284。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 7075.515723029, Recall@10 0.560324302135, evidence identity 950f70d9c3fd4a747213519c7d3053a46c434a29ea4aa6e7d1f2d20d4079e284. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 10109.037997335；原Recall@10 0.200143678161；原证据身份 cb9660733e1604a0b11a926ac6d8627b4357f5f037db75ee53d705880bb2f29c。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 10109.037997335, Recall@10 0.200143678161, evidence identity cb9660733e1604a0b11a926ac6d8627b4357f5f037db75ee53d705880bb2f29c. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 6721.647957989；原Recall@10 0.913440860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 6721.647957989, Recall@10 0.913440860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 46453.037529611；原Recall@10 0.913440860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 46453.037529611, Recall@10 0.913440860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 76055.332042230；原Recall@10 0.913440860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 76055.332042230, Recall@10 0.913440860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 105113.018473746；原Recall@10 0.913440860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 105113.018473746, Recall@10 0.913440860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 4957.213801547；原Recall@10 0.950940860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 4957.213801547, Recall@10 0.950940860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 34466.622637823；原Recall@10 0.950940860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 34466.622637823, Recall@10 0.950940860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 55635.440021090；原Recall@10 0.950940860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 55635.440021090, Recall@10 0.950940860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 70814.558382246；原Recall@10 0.950940860215；原证据身份 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 70814.558382246, Recall@10 0.950940860215, evidence identity 3c61acc40c6868ca233d99a5f37bbfd9f40bf215f7507fc282e3b288ac0803a6. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 21988.285259836；原Recall@10 0.125537634409；原证据身份 b941c6a693d7487da2a8e42148cec6e1ca7821abab1ebdbd4ee43a0d9eb66483。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 21988.285259836, Recall@10 0.125537634409, evidence identity b941c6a693d7487da2a8e42148cec6e1ca7821abab1ebdbd4ee43a0d9eb66483. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 11320.390216248；原Recall@10 0.000537634409；原证据身份 1dbb73d1fd3f7fc87faf4d690cba0ccf1aa701bf3a884c1d2331cc3569b7f2e4。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 11320.390216248, Recall@10 0.000537634409, evidence identity 1dbb73d1fd3f7fc87faf4d690cba0ccf1aa701bf3a884c1d2331cc3569b7f2e4. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 5430.626971345；原Recall@10 0.924462365591；原证据身份 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5430.626971345, Recall@10 0.924462365591, evidence identity 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 37394.221408178；原Recall@10 0.924462365591；原证据身份 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 37394.221408178, Recall@10 0.924462365591, evidence identity 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 59833.973328286；原Recall@10 0.924462365591；原证据身份 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 59833.973328286, Recall@10 0.924462365591, evidence identity 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 78475.338565547；原Recall@10 0.924462365591；原证据身份 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 78475.338565547, Recall@10 0.924462365591, evidence identity 3523ac50a6694dd9991383a6d82e82feae54bd67e5b047d2349a68c96209fdb8. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 5441.633339213；原Recall@10 0.948387096774；原证据身份 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5441.633339213, Recall@10 0.948387096774, evidence identity 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 37258.690285924；原Recall@10 0.948387096774；原证据身份 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 37258.690285924, Recall@10 0.948387096774, evidence identity 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 59883.399946569；原Recall@10 0.948387096774；原证据身份 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 59883.399946569, Recall@10 0.948387096774, evidence identity 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 79365.709413273；原Recall@10 0.948387096774；原证据身份 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 79365.709413273, Recall@10 0.948387096774, evidence identity 9ca0c488eee72c8e7af220a857ce72fb48f6a21ed30401fe21bc4b9c6e540454. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 6142.927672092；原Recall@10 0.907392473118；原证据身份 f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 6142.927672092, Recall@10 0.907392473118, evidence identity f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 44142.212240613；原Recall@10 0.907392473118；原证据身份 f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 44142.212240613, Recall@10 0.907392473118, evidence identity f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 74069.470213812；原Recall@10 0.907392473118；原证据身份 f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 74069.470213812, Recall@10 0.907392473118, evidence identity f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 113104.314272495；原Recall@10 0.907392473118；原证据身份 f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 113104.314272495, Recall@10 0.907392473118, evidence identity f408e24713596e745f01ac1cbba9a2205224d86aa2ab1764f0c27bf4c0b12027. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 5328.186798632；原Recall@10 0.003225806452；原证据身份 3eb9a198236f5298a05036fce5331a36fde2955b85286a60454f96b50c92f192。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5328.186798632, Recall@10 0.003225806452, evidence identity 3eb9a198236f5298a05036fce5331a36fde2955b85286a60454f96b50c92f192. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 5489.143399952；原Recall@10 0.303897849462；原证据身份 bc603a4be4586fdeaa1cc55677bc3b2571aade8f7bafc6146f9ead8505c9c414。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5489.143399952, Recall@10 0.303897849462, evidence identity bc603a4be4586fdeaa1cc55677bc3b2571aade8f7bafc6146f9ead8505c9c414. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 6502.414110342；原Recall@10 0.860215053763；原证据身份 6058ae8e380838e4d982f31d4ce910827fdf5fa0880156eb7499fed5b62d790f。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 6502.414110342, Recall@10 0.860215053763, evidence identity 6058ae8e380838e4d982f31d4ce910827fdf5fa0880156eb7499fed5b62d790f. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "原QPS 5660.942552791；原Recall@10 0.566564039409；原证据身份 130d9d28749c730af44739db981cd5d2de6b51bf4157ae61c7fe600aec27bf7e。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。":
    "Original QPS 5660.942552791, Recall@10 0.566564039409, evidence identity 130d9d28749c730af44739db981cd5d2de6b51bf4157ae61c7fe600aec27bf7e. These observations remain in the historical records. The current SM-L2 curve uses the corrected measurements.",
  "论文预期的适用边界对照，不是承诺达到目标的适用任务；原生L2与旧稿归一化余弦结果不可混比。":
    "Expected applicability control in the paper. Native L2 and the old normalized-cosine results correspond to different metrics.",
  "旧入口ef小于64时拒绝64精排预算后回退BQ；保留原始标签与计时，不称纯SM-L2。":
    "The old entry point rejected a reranking budget of 64 when ef was below 64, then fell back to BQ. Original labels and timing are retained with their actual BQ route.",
});
