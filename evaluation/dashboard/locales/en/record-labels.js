// 英文记录文案，中文原文作为稳定查找键。
window.QuiverLocaleData = window.QuiverLocaleData || { en: {} };
Object.assign(window.QuiverLocaleData.en, {
  核心对比库: "Core comparisons",
  扩展库: "Extended datasets",
  适用边界对照: "Applicability controls",
  合成与规模库: "Synthetic and scale datasets",
  量化图: "Quantized graph",
  量化数据区与压缩图下界: "Lower bound for quantized data and the compressed graph",
  "BBQ量化导航；外部FP32精排": "BBQ navigation with external FP32 reranking",
  "保存已包含在构建命令或commit中；不另填0秒":
    "Persistence is included in the build command or commit. No separate zero-second save value is inserted.",
  全精度图: "Full-precision graph",
  实际头部与底图容量小计: "Subtotal of actual header and base-graph capacities",
  FP32参与图搜索: "FP32 participates in graph search",
  "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外":
    "The original timer surrounds the save function, including serialization, file writes and binding overhead. Complete file hashing is outside the interval.",
  "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载":
    "The original wrapper's load boundary is retained, with library-specific definitions. It may include more than the native C++ API or may use demand paging.",
  "SQ8导航；自有FP32精排": "SQ8 navigation with internal FP32 reranking",
  重载后数组容量小计: "Reloaded array-capacity subtotal",
  FP32参与搜索: "FP32 participates in search",
  "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值":
    "Class objects, hidden allocator overhead and query workspaces remain outside the subtotal. Reloaded capacities describe persistent arrays rather than the build-time peak.",
  倒排全精度: "Full-precision IVF",
  倒排量化: "Quantized IVF",
  "量化搜索；无全库精排库": "Quantized search without a full-database reranking store",
  "量化导航；精排库独立计冷":
    "Quantized navigation with a separately accounted cold reranking store",
  "DiskANN PQ构建 / FP32查询": "DiskANN PQ build / FP32 search",
  对齐图与导航主数组小计: "Aligned graph and primary navigation-array subtotal",
  "沿用原索引构建记录；本阶段只核验和复用索引，没有重新构建":
    "Reuses the original index-build record. This stage verifies and reuses the existing index.",
  "保存包含在原生进程墙钟内，现有记录不能分离出保存时长":
    "Saving is included in native-process wall time. The available record cannot isolate a save duration.",
  "DiskANN PQ导航 + FP32精排": "DiskANN PQ navigation + FP32 reranking",
  "PQ导航；自有FP32只用于最终精排":
    "PQ navigation with internal FP32 used only for final reranking",
  持久组件精确合计: "Exact persistent-component total",
  "FP32参与导航；无全库冷精排向量": "FP32 navigation without a full-database cold reranking store",
  "含逐边局部二进制码、三个浮点因子和邻居ID；不是仅2-bit码与图":
    "Includes per-edge local binary codes, three floating-point factors and neighbor IDs in addition to the graph representation.",
  "查询工作区、容器容量及分配器开销未完整拆分":
    "Query workspaces, container capacities and allocator overhead are incompletely itemized.",
  图行与容器描述符下界: "Lower bound for graph rows and container descriptors",
  FP32参与导航: "FP32 participates in navigation",
  "FAISS BinaryHNSW 中心化 + FP32": "FAISS BinaryHNSW centered + FP32",
  "M=16; 底层容量=32; efC=200": "M=16; base-layer capacity=32; efC=200",
  "1-bit导航；外部FP32精排": "1-bit navigation with external FP32 reranking",
  "FAISS BinaryHNSW 符号位 + FP32": "FAISS BinaryHNSW sign bits + FP32",
  文件头与加载数组小计: "File-header and loaded-array subtotal",
  "含底层向量/边/标签、上层边、上层指针及层级数组；文件长度与加载源码交叉核算":
    "Includes base-layer vectors, edges and labels, upper-layer edges and pointers, and level arrays. File lengths were cross-checked against the loader source.",
  "官方 coco-i2i 预设（实际R=60）": "Official coco-i2i preset (actual R=60)",
  作者库重载数组容量小计: "Author-library reloaded-array-capacity subtotal",
  量化图与树导航数组下界: "Lower bound for quantized-graph and tree-navigation arrays",
  "ANNG、图重构及量化图创建/构建命令墙钟；命令内部已持久化，文件哈希另计":
    "Wall time for ANNG, graph reconstruction and quantized-graph creation/build commands. These commands persist data internally. File hashing is separate.",
  "RaBitQ 官方 IVF + FP32": "RaBitQ official IVF + FP32",
  "nlist=4096; 标准量化": "nlist=4096; standard quantization",
  加载源码主数组请求小计: "Primary-array requested-size subtotal from loader source",
  "量化导航；自有FP32精排": "Quantized navigation with internal FP32 reranking",
  "仍缺类对象、容器隐藏容量、分配器及逐查询旋转/LUT/中心候选/重排工作区":
    "Class objects, hidden container capacity, allocator overhead and per-query rotation, LUT, centroid-candidate and reranking workspaces remain separate.",
  "RaBitQ 官方 IVF 7 bit": "RaBitQ official IVF 7 bit",
  "多位量化查询；无全库FP32精排库":
    "Multi-bit quantized search without a full-database FP32 reranking store",
  "RaBitQ 官方 IVF 4 bit": "RaBitQ official IVF 4 bit",
  "nlist=4096; 快速量化": "nlist=4096; fast quantization",
  "SVS LVQ4×4（共8 bit）": "SVS LVQ4×4 (8 bits total)",
  "主码导航；残差码仅最终重排；无全库FP32":
    "Primary-code navigation with residual codes used for final reranking, without full-database FP32",
  "复用原官方LVQ量化与Vamana构建API实测墙钟；未重新建图":
    "Reuses the original measured official LVQ quantization and Vamana build-API wall time, with the same graph.",
  "旧SVS保存函数内部含组件清单哈希，现有记录无法剥离SDK保存子步骤":
    "The old SVS save function also hashes the component manifest. Existing records cannot isolate the SDK save substep.",
  "SVS LVQ4×8（共12 bit）": "SVS LVQ4×8 (12 bits total)",
  "官方LVQ量化和Vamana构建API墙钟；冻结源哈希、格式转换及保存独立记录":
    "Official LVQ quantization and Vamana build-API wall time. Frozen-source hashes, format conversion and saving are recorded separately.",
  "保存包装另含组件哈希，原生API时间独立列示":
    "The save wrapper also hashes components. Native-API time is listed separately.",
  "LVQ主码导航；无全库FP32": "LVQ primary-code navigation without full-database FP32",
  "包括对齐、缩放等实际布局；驻留量已加hugetlb":
    "Includes actual alignment, scaling and layout. Resident memory includes hugetlb.",
  自有tape已分配容量小计: "Allocated capacity of internal tape storage",
  "1-bit导航；外部FP32仅精排": "1-bit navigation with external FP32 used only for reranking",
  "同版本只读工具实读码、上下层图、层级、偏移和中心容量；不含隐藏运行时开销":
    "A same-version read-only tool reads capacities of codes, upper/base graphs, levels, offsets and centroids. Hidden runtime overhead is excluded.",
  "类对象、分配器和查询工作区仍另计":
    "Class objects, allocator overhead and query workspaces remain separate.",
  "保存包装时钟；不把缺少的内层API时钟补造为0":
    "Save-wrapper timer. A missing inner-API timer is left unavailable.",
  "冻结库包装加载边界；外部向量映射按需触页":
    "Frozen library-wrapper load boundary. External vector mappings are touched on demand.",
  "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值":
    "Ordinary RSS plus private and shared hugetlb. The maximum recorded snapshot is a discrete observation.",
  "同版本memory_stats实读分配容量，包含预留与对齐空隙，不重复相加":
    "Same-version memory_stats reads allocated capacity, including reserved space and alignment gaps, counted once.",
  "保存包装墙钟含完整组件哈希；官方保存API时长单列":
    "Save-wrapper wall time includes complete component hashing. Official save-API duration is listed separately.",
  "保存的查询后RSS加私有/共享hugetlb快照；最大快照不是连续峰值":
    "Recorded post-query RSS plus private/shared hugetlb snapshots. The maximum is taken across discrete observations.",
  "持久数组小计，非完整热内存": "Persistent-array subtotal",
  "八核原生BQ构图API；排除导入、保存与SM安装。构建单次，查询五轮。":
    "Eight-CPU native BQ graph-build API. Excludes import, saving and SM installation. One build and five query rounds.",
  "图加载API加重载SM安装；不是统一完全冷启动。":
    "Graph-load API plus SM installation after reload. The boundary includes these two operations.",
  "五轮查询后快照最大值；不是连续驻留峰值。":
    "Maximum post-query snapshot across five rounds. Residency was sampled discretely.",
  "原始FP32仅用于末尾精排；逻辑冷不等于未驻留。":
    "Original FP32 vectors are used for final reranking. Logically cold vectors may remain resident.",
  "原生L2保留原幅值及零向量，BQ/SM/共享图和映射均计入持久数组小计。":
    "Native L2 preserves original magnitudes and zero vectors. BQ, SM, the shared graph and maps are included in the persistent-array subtotal.",
  原生请求: "Native requests",
  标准: "Standard",
  独立调参集达标: "Target met on independent tuning queries",
  "官方逐查询服务时长，原生整数微秒；不含应用排队及Python；边界仍须跨库审计":
    "Official per-query service duration in native integer microseconds. Application queuing and Python are excluded. Library-specific boundaries are recorded separately.",
  逐请求: "Per request",
  独立调参集未达目标: "Target unmet on independent tuning queries",
  作者接口: "Author interface",
  SDK批量: "SDK batch",
  "SVS官方SDK原生逐查询计时未支持，所有P95/P99明确保留缺项":
    "The official SVS SDK exposes no native per-query timing in this wrapper. P95/P99 are left unavailable.",
  "原冻结最高预算诊断；未设置达标锚点":
    "Frozen maximum-budget diagnostic, without a target-qualified anchor",
  "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。":
    "Uses the original frozen positions. Historical BQ routing at low ef is retained, and parameters are independent of the new evaluation outcomes.",
  "原生请求完整搜索与精排；五轮均值。":
    "Native requests with complete search and reranking, averaged across five rounds.",
  "单查询服务时钟，五轮完整评估查询。":
    "Per-query service timer, with five complete evaluation-query rounds.",
  "调参未达目标，保留最高预算": "Tuning target unmet, maximum budget retained",
  原生L2: "Native L2",
  事项: "Item",
  含义与比较限制: "Definition and comparison notes",
  使用方式: "How to browse",
  结果范围: "Result coverage",
  参数选择: "Parameter selection",
  "实际990,000×1536，表中写990K；调参256条，评估9,744条。":
    "Actual size 990,000 × 1536, shown as 990K. 256 tuning and 9,744 evaluation queries.",
  查询独立性限制: "Query independence",
  "Cohere查询曾用于既有探索；当前调参/评估拆分不能称为从未接触的全新盲测。":
    "Cohere queries had appeared in earlier exploration. The current tuning/evaluation split therefore shares that development history.",
  服务器: "Server",
  距离: "Distance metric",
  QPS均值: "Mean QPS",
  FAISS缺请求QPS: "Unavailable FAISS request QPS",
  FAISS线程环境缺陷: "FAISS thread-environment issue",
  FAISS测量替换: "FAISS measurement replacement",
  FAISS线程初始化: "FAISS thread initialization",
  其他绑定路径抽查: "Checks of other bindings",
  FAISS稳定性: "FAISS stability",
  分配器对照: "Allocator comparison",
  "SVS QPS与延迟": "SVS throughput and latency",
  MIRAGE多线程: "MIRAGE multithreading",
  未采集的尾延迟: "Uncollected tail latency",
  尾延迟置信区间: "Tail-latency confidence intervals",
  冷热定义: "Hot and cold definitions",
  热组件下界: "Hot-component lower bound",
  数组容量核对: "Array-capacity audit",
  补齐的大组件: "Additional large components",
  NGT冷热角色: "NGT component roles",
  USearch预留容量: "USearch reserved capacity",
  完整运行时的限度: "Runtime accounting scope",
  驻留快照: "Resident snapshots",
  部署文件: "Deployment files",
  构建时长: "Build duration",
  构建比较限制: "Build-timer boundaries",
  保存时长: "Save duration",
  独立保存计时核对: "Separate save timers",
  加载时长: "Load duration",
  外部映射限制: "External mapping scope",
  SQG热组件: "SQG hot components",
  DiskANN模式: "DiskANN modes",
  NGT组件核对: "NGT component audit",
  SVS表示: "SVS representations",
  BinaryHNSW组件核对: "BinaryHNSW component audit",
  BBQ重复向量存储: "BBQ duplicate vector storage",
  软件: "Software",
  配置结论: "Configuration scope",
  "采用已验证配置与独立调参选择；不宣称各库全局最优或已完成统一内存预算最优搜索。":
    "Uses verified configurations and independent tuning selections. The archive records this configuration search and its measured memory budgets.",
  仍无可比结果: "Unavailable comparisons",
  数据集覆盖: "Dataset coverage",
  线程覆盖: "Thread coverage",
  SQG多线程: "SQG multithreading",
  补测覆盖: "Supplementary coverage",
  已补录的计时证据: "Recovered timing evidence",
  加载缓存核对: "Load-cache audit",
  结果有效性: "Result validity",
  证据来源: "Evidence sources",
  "基线/模式": "Baseline / mode",
  "1/8/16/32T\n各参数覆盖见曲线": "1/8/16/32T\nPer-parameter coverage is recorded in the curves",
  QuIVer覆盖: "QuIVer coverage",
  QuIVer参数选择: "QuIVer parameter selection",
  QuIVer构建与加载: "QuIVer build and load",
  QuIVer组件与驻留: "QuIVer components and residency",
  QuIVer内存限制: "QuIVer memory accounting",
  QuIVer真值修复: "QuIVer ground-truth repair",
  "43个低ef位置的修正": "Correction of 43 low-ef positions",
  论文适用范围与边界对照: "Paper scope and applicability controls",
  外部权威JSON: "External authoritative JSON",
  QuIVer增补JSON: "QuIVer supplementary JSON",
  权威工作簿: "Authoritative workbook",
  低宽度修正覆盖: "Low-width correction overlay",
  热组件口径: "Hot-component accounting",
  导航与精排: "Navigation and reranking",
  内存组件: "Memory components",
  未归因开销: "Unattributed overhead",
  构建计时: "Build timer",
  保存计时: "Save timer",
  加载计时: "Load timer",
  冷加载确认: "Cold-load confirmation",
  驻留口径: "Resident-memory definition",
  冷组件: "Cold components",
  吞吐边界: "Throughput boundary",
  延迟边界: "Latency boundary",
  延迟可用性: "Latency availability",
  线程环境待核: "Thread environment pending verification",
  预算修正: "Budget correction",
  原BQ对照: "Original BQ comparison",
  适用范围: "Applicability",
  低宽度路由: "Low-width route",
});
