// 英文记录文案，中文原文作为稳定查找键。
window.QuiverLocaleData = window.QuiverLocaleData || { en: {} };
Object.assign(window.QuiverLocaleData.en, {
  "先筛数据集，再看95%/98%锚点；全部参数与线程明细见对应工作表。目标只表示调参目标，排名必须查看实测召回。":
    "Filter by dataset, then inspect the 95%/98% anchors. Corresponding records contain every parameter and thread. Targets describe tuning goals. Comparisons use actual measured recall.",
  "外部基线30种模式、95配置、813个线程位置；增补QuIVer十九库、19配置、227位置。合计114配置、1,040位置。外部基线覆盖三个核心库，QuIVer覆盖十九库。":
    "The source record contains 30 external modes, 95 configurations and 813 thread observations, plus QuIVer on 19 datasets with 19 configurations and 227 observations. Total: 114 configurations and 1,040 observations. External methods cover three core datasets.",
  "展示锚点仅根据已冻结的独立调参查询及原选定锚点确定；没有按评估集QPS/召回重新挑参数。曲线保留全部有效点。":
    "Displayed anchors use frozen independent tuning queries and the original selections. Evaluation QPS and recall do not reselect parameters. Curves retain all valid measured points.",
  "1,000,000×768；调参256条，评估744条；归一化余弦、Recall@10。":
    "1,000,000 × 768. 256 tuning and 744 evaluation queries. Normalized cosine, Recall@10.",
  "5,000,000×768；公开35M源前5M，查询近重复已替换并冻结行号；与1M复用查询，真值按5M重算。":
    "5,000,000 × 768, the first 5M rows of the public 35M source. Near-duplicate query matches were replaced and row IDs frozen. Shares queries with 1M, with ground truth recomputed against 5M.",
  "Intel Xeon Platinum 8352V；CPU亲和范围0–31；同机串行测量。每点五轮；不同线程点不同时运行。":
    "Intel Xeon Platinum 8352V, CPU affinity 0–31. Measurements run serially on the same server, five rounds per point. Different thread settings are measured separately.",
  "本次工作簿中的三个外部核心库和十九个QuIVer库均为余弦任务，输入按协议归一化。QuIVer低宽度BQ回退单独标注；SIFT、GIST原生L2未纳入本快照。":
    "This source workbook covers cosine tasks for three external core datasets and 19 QuIVer datasets, with inputs normalized by protocol. Low-width BQ fallback is labeled separately. Native-L2 SIFT/GIST was added in a later snapshot.",
  "五轮吞吐算术均值；轮次不是独立新查询集。1T/8T/16T/32T为请求执行线程数。":
    "Arithmetic mean of five throughput rounds using the same query set. 1T/8T/16T/32T denotes the request-execution thread count.",
  "总表优先展示补测的逐请求原生吞吐，包含搜索与精排。官方批量search吞吐在完整参数曲线独立列示；两者不能混排。":
    "The main table prioritizes supplementary native per-request throughput, including search and reranking. Official batch-search throughput has a separate field in the complete parameter curves.",
  "34个非锚点只有官方批量吞吐，未找到独立逐请求计数/墙钟；请求QPS留空，不能拿延迟倒数或批量吞吐补入。":
    "34 non-anchor points have only official batch throughput. Independent per-request counts and wall times were unavailable, so request QPS remains empty.",
  "原队列用0-7启动并初始化OpenMP，之后才放宽CPU范围。单点32T同图对照：初始化32CPU约1011 QPS、31.3核；初始化8CPU约313 QPS、7.9核。标签相同，活动线程掩码也证实受限0-7。":
    "The original queue started and initialized OpenMP on CPUs 0–7 before expanding affinity. A same-index 32T check measured about 1011 QPS/31.3 cores with 32-CPU initialization, versus 313 QPS/7.9 cores with 8-CPU initialization. Labels matched. Active-thread masks confirmed restriction to 0–7.",
  "21个配置的78个16/32T位置已用同索引补测替换。保留冻结参数、完整评估查询及五轮，原始标签全部一致；构建、加载及1/8T结果沿用原记录。旧测量另行保留追溯。":
    "78 points at 16/32T across 21 configurations were replaced by same-index reruns. Frozen parameters, complete evaluation queries and five rounds were retained, with all raw labels matching. Build, load and 1/8T results retain their original records. Historical measurements remain traceable.",
  "在完整0–31 CPU范围初始化OpenMP/MKL，关闭库内重绑核，再按16/32线程预算执行。新测量的原始计数、墙钟、标签、延迟和协议均独立复核；新区间来自新延迟数组。":
    "Initialize OpenMP/MKL with CPUs 0–31 available, disable library-internal rebinding, then execute with a 16/32-thread budget. Raw counts, wall times, labels, latency and protocols were independently checked. New intervals use the new latency arrays.",
  "1M代表配置中，BinaryHNSW实际工作线程逐核绑定0–31；MIRAGE采用作者FAISS/GNU OpenMP，观察线程掩码0–31。未复现上述8CPU限制；这是代表性抽查，不代替所有配置环境审计。":
    "In representative 1M configurations, BinaryHNSW workers were pinned individually to CPUs 0–31. MIRAGE used the author's FAISS/GNU OpenMP with observed masks 0–31. These configurations did not reproduce the 8-CPU restriction. The checks cover those representative configurations.",
  "78个补测位置均通过冻结稳定性门检；请求QPS最高CV为3.40%。旧DBpedia IVF-PQ+FP32的9.78%波动不再用于本表，历史慢轮的精确调用栈仍无法追溯。":
    "All 78 supplementary positions passed the frozen stability checks, with maximum request-QPS CV 3.40%. The old DBpedia IVF-PQ+FP32 observation with 9.78% CV is retained only historically. Its slow-round call stack could not be recovered.",
  "默认约990 QPS与关闭自动trim约988 QPS，各五次全量查询且标签相同；关闭trim没有提高均值，不能认定trim是根因。诊断结果不替换正式数字。":
    "Default allocation gave about 990 QPS and disabling automatic trim about 988 QPS. Each used five complete-query rounds with identical labels. Disabling trim did not improve the mean. These diagnostic values remain separate from formal measurements.",
  "85个位置的冻结SDK包装未导出原生逐查询数组；P95/P99留空，不能从批量耗时倒推。":
    "The frozen SDK wrapper for 85 positions exported no native per-query arrays. P95/P99 remain unavailable because batch durations cannot recover them.",
  "18个位置保留作者接口吞吐；现存多线程汇总统计不足以还原逐请求P95/P99，未发现可恢复原始数组。":
    "18 positions retain throughput from the author's interface. Existing multithread summaries cannot recover per-request P95/P99, and no recoverable raw arrays were found.",
  "92个扫描位置未采集对应原始延迟数组，不能仅靠统计补出。618个已测位置均保留原始数组及查询簇区间。":
    "92 scan positions lack the corresponding raw latency arrays. The 618 measured positions retain raw arrays and query-cluster confidence intervals.",
  "原生请求耗时，通常不含外层排队；含该查询的搜索与精排。它与包含调度的总吞吐不一定互为倒数。":
    "Native request duration, usually excluding outer queuing and including search and reranking. Aggregate throughput additionally includes scheduling, so its reciprocal has a different boundary.",
  "按唯一查询做聚类重采样，保留每条查询跨轮相关性；2,000次、95%区间。不把同一查询五轮当五个独立样本。":
    "Cluster bootstrap over unique queries, retaining each query's cross-round dependence. 2,000 resamples and 95% intervals. The five repeats of one query remain in the same cluster.",
  "热：导航/候选筛选必需组件。冷：只用于最终重排的全库向量或残差码。逻辑冷不代表当前未驻留，也不保证可直接从SSD以相同QPS查询。":
    "Hot components are required for navigation or candidate selection. Cold components are full-database vectors or residual codes used only for final reranking. Logically cold storage may remain resident in the measured process. SSD throughput is measured separately.",
  "带≥的数值只覆盖已确认组件，不能据此排名完整热内存。精确持久组件合计也不含搜索工作区及分配器开销。":
    "Values marked ≥ cover confirmed components. Exact persistent-component totals also exclude search-workspace and allocator overhead. Each row specifies its accounting scope.",
  "FAISS/BinaryHNSW30项、MIRAGE3项实读数组容量；USearch4项实读自有tape；VSAG7项核对实际头部容量。其余图、树及量化组件按冻结头/元数据/源码拆分；每库仍保留未覆盖开销。":
    "Read array capacities for 30 FAISS/BinaryHNSW items and three MIRAGE items, internal tape storage for four USearch items, and actual header capacities for seven VSAG items. Other graph, tree and quantized components use frozen headers, metadata and source. Uncovered overhead remains listed per library.",
  "原30项粗下界与3项NGT冷热未知均已细化。BBQ每点除1-bit码还有16字节校正项；DiskANN图行按64字节对齐；不能把这些存储成本漏掉。":
    "The original 30 coarse lower bounds and three NGT entries with unknown hot/cold roles were refined. BBQ stores 16 correction bytes per point in addition to 1-bit codes. DiskANN graph rows use 64-byte alignment. These costs are included.",
  "三库均用量化树：内部节点自有FP32 pivot为热，局部量化边码/ID导航；全库原FP32只精排，单列冷。叶pivot、原图等保留组件仍要与纯导航热和进程RSS区分。":
    "All three datasets use quantized trees. Internal tree-owned FP32 pivots are hot, and local quantized edge codes/IDs support navigation. Original full-database FP32 serves final reranking and is logically cold. Retained leaf pivots and original graphs have separately identified roles.",
  "memory_stats的已分配tape包含未用预留与对齐/旧arena空隙，不能重复相加；自有mmap也不能用glibc mallinfo2代表。线程reserve诊断未执行查询，不能据此断言查询工作区很小。":
    "Allocated tape in memory_stats already includes unused reserve and alignment/old-arena gaps, counted once. Internal mmap is outside glibc mallinfo2. The thread-reserve diagnostic ran no queries, so it supplies no query-workspace measurement.",
  "逐组件已知数组不等于完整进程：哈希桶/节点、锁、实际visited池、候选队列、分配器及Java GC/代码开销仍按库明确列出。没有为缺少这些字段重建索引。":
    "Known component arrays cover part of process storage. Hash buckets/nodes, locks, actual visited pools, candidate queues, allocator and Java GC/code overhead remain listed per library. Existing indexes were retained during this accounting audit.",
  "进程族RSS加私有/共享hugetlb保守合计；包含代码、运行时、精排库及已触碰映射页，不等于热组件。最大快照不是连续测得的峰值。":
    "Conservative process-family RSS plus private/shared hugetlb. Includes code, runtime, reranking vectors and touched mapping pages. The maximum is taken across discrete snapshots.",
  "SVS的大页内存已计入。只看普通RSS会出现不可信的两位数MiB，不能用于论文内存比较。":
    "SVS huge-page memory is included. Ordinary RSS alone omits these allocations and can show implausibly small two-digit MiB values.",
  "只计查询部署必需文件：索引组件＋外部必需向量；同一模式内不重复加自有FP32。不同模式各自计算，不能相加当共享部署。":
    "Counts files required to serve queries: index components and required external vectors. Internal FP32 storage is counted once per mode. Each mode has its own deployment total.",
  "使用对应图/索引的原生构建记录；FAISS包含训练和add，RaBitQ包含聚类与量化，α-CNG包含初始NNDescent；SVS含量化和建图。":
    "Uses the graph/index's native build record. FAISS includes training and add, RaBitQ includes clustering and quantization, α-CNG includes initial NNDescent, and SVS includes quantization and graph construction.",
  "未统一剥离所有库的数据转换、保存和初始化。共享图模式复用相同构建记录，不代表额外又建了一次。不能把所有构建列解释成完全相同边界。":
    "Data conversion, saving and initialization have library-specific boundaries. Modes sharing a graph reuse its build record once. Per-configuration notes describe the recorded interval.",
  "95配置中14项保存包装为空：BBQ3项和NGT1项在构建内持久化；DiskANN10项未单独计时。保存包装与内层API时钟按实际记录分列，不填占位0。":
    "Among 95 source configurations, 14 save-wrapper fields are empty. Three BBQ and one NGT configuration persist during construction, while ten DiskANN configurations have no separate timer. Wrapper and inner-API clocks are listed where available.",
  "保存包装与SDK内层API独立时钟分别记录。没有内层时钟的配置保留包装值和具体边界，不把包装时间冒充更窄的API时间。":
    "The save wrapper and SDK inner API have separate clocks. Configurations lacking an inner clock retain the wrapper duration with its actual boundary.",
  "DiskANN：独立冷加载原生API；BBQ：Java内部至服务就绪，含FP32读入与映射、显式GC；其余：原库包装加载边界。不是统一完全预载时长。":
    "DiskANN uses an independent native cold-load API. BBQ measures Java-internal time to service readiness, including FP32 reads/mappings and explicit GC. Other libraries retain their original wrapper boundaries. Preloading coverage is library-specific.",
  "BinaryHNSW未证明外部FP32全冷；HSCG/α-CNG导航mmap按需缺页，加载完成不等于全部导航页驻留。":
    "BinaryHNSW has no proof that all external FP32 pages were cold. HSCG/α-CNG navigation mmap uses demand paging, so completed loading can precede navigation-page residency.",
  "包含导航FP32、逐边局部二进制码、三个float因子、ID与旋转向量。局部码是逐边存储，不能按每个点只有一个1-bit码估算。":
    "Includes navigation FP32, per-edge local binary codes, three float factors, IDs and rotation vectors. Local codes are stored per edge, so their size scales with edges as well as points.",
  "FullPrecision查询模式的自有FP32属于热；Hybrid源码明确PQ导航、FP32只精排，冷向量仍会常驻当前进程。":
    "Internal FP32 is hot in FullPrecision search. Hybrid source uses PQ navigation and FP32 only for reranking, with those logically cold vectors resident in the current process.",
  "量化树内部独立FP32 pivot参与导航；全库原FP32只精排，已计冷。78项组件下界中的未覆盖运行时开销仍逐库列示。":
    "Independent internal-tree FP32 pivots participate in navigation. Original full-database FP32 is used for reranking and counted cold. Remaining runtime overhead is listed per library for the 78 component lower bounds.",
  "LVQ4×4共8位、LVQ4×8共12位。Vamana源码的图搜索用PrimaryAccessor，随后get_datum读取主码＋残差重排；残差计逻辑冷，主码、布局、centroid与图计热。":
    "LVQ4×4 uses 8 bits total and LVQ4×8 uses 12. Vamana graph search uses PrimaryAccessor, then get_datum reads primary and residual codes for reranking. Residuals are logically cold. Primary codes, layout, centroids and graph are hot.",
  "已用同版本加载实读数组容量，包含上下层图、层级、偏移、二进制码及中心；外部FP32精排计冷。下界剩余部分为类对象、分配器及查询工作区。":
    "Same-version reloading reads capacities of upper/base graphs, levels, offsets, binary codes and centroids. External FP32 reranking vectors are cold. Class objects, allocator overhead and query workspaces remain outside the component subtotal.",
  "Lucene目录的.vec已经包含一份FP32，包装还读入外部FP32精排库；部署合计包括两份。它们不能一起算成热导航码，也不能漏算外部必需文件。":
    "The Lucene .vec file already stores FP32 vectors, while the wrapper also loads an external FP32 reranking store. Deployment includes both copies with their actual roles. External required files are included.",
  "FAISS 1.15.0/MKL/AVX512；HNSWlib 0.8.0；USearch 2.25.1；官方RaBitQ 0.5.2；Lucene 10.5.1。MIRAGE使用作者FAISS 1.8分支。其余以冻结源码与二进制哈希为准。":
    "FAISS 1.15.0/MKL/AVX512, HNSWlib 0.8.0, USearch 2.25.1, official RaBitQ 0.5.2 and Lucene 10.5.1. MIRAGE uses the author's FAISS 1.8 branch. Other versions are bound by frozen-source and binary hashes.",
  "HSCG采用作者coco-i2i剪枝R=60；5M官方连通性补边后最大度66。α-CNG是完整对应实现，固定α诊断不能冒称独立α-CG基线。":
    "HSCG uses the author's coco-i2i pruning preset R=60. On 5M, official connectivity augmentation yields maximum degree 66. α-CNG is the corresponding complete implementation. Fixed-α diagnostics remain a separate configuration analysis.",
  "SegPQ、BSH是组件方案，未作为独立ANN系统给出可比整机结果；DiskANN SSD、RaBitQ 5-bit未形成完整比较。不能用别的模式代替。":
    "SegPQ and BSH are component techniques with no comparable standalone ANN-system results here. DiskANN SSD and RaBitQ 5-bit also lack complete comparisons. Their entries remain unavailable.",
  "外部基线90个数据集×模式组合，QuIVer十九个数据集组合。其余十六库没有对应外部全交叉结果，不能将空白当性能为零。":
    "External baselines cover 90 dataset–mode combinations, and the source QuIVer results cover 19 datasets. The other 16 datasets have no full external-method cross-product. Empty entries indicate absent measurements.",
  "已有配置展示锚点的59个缺线程位置均已补齐。纯LVQ4的1M/5M最高固定预算诊断也有1/8/16/32T；各非锚点扫描参数可能只测1T，逐参数看曲线。":
    "All 59 missing thread observations for displayed anchors were completed. Pure LVQ4 fixed-maximum-budget diagnostics on 1M/5M also include 1/8/16/32T. Some non-anchor scan parameters have only 1T measurements. Each curve records its available threads.",
  "查询工作区已按线程隔离；新增线程完整标签与原官方同图1T一致。冻结参数与完整查询五轮均保持，已包含1/8/16/32T。":
    "Query workspaces are thread-local. All new thread labels match the official same-graph 1T labels. Frozen parameters and five full-query rounds are retained, including 1/8/16/32T.",
  "108个补充位置均已下载、独立复算并合并。展示锚点包含1/8/16/32T；非锚点未测尾、接口不支持与内存下界分别保留。":
    "All 108 supplementary positions were downloaded, independently recomputed and merged. Displayed anchors include 1/8/16/32T. Missing non-anchor tails, unsupported interface metrics and memory lower bounds remain identified separately.",
  "从现有原始记录补入30个配置的训练/量化训练时间、9个配置的图插入时间，逐配置明确构建边界；共享图的时间不能重复归总。":
    "Existing raw records supplied training/quantization-training durations for 30 configurations and graph-insertion durations for nine configurations. Each configuration identifies its build boundary, and shared-graph time is counted once.",
  "16项加载记录未确认全冷，不能改标为全冷；缓存检查不代表所有外部向量或全进程冷启动。具体缓存比例见构建与加载明细。":
    "16 load records have unverified fully cold status. Their cache checks cover the specified components. Per-configuration build/load details retain the observed cache ratios.",
  "原始标签、延迟、协议、构建/加载记录与SHA-256绑定留存；召回、吞吐均值、CV与尾延迟已独立复算。":
    "Raw labels, latency, protocols and build/load records retain SHA-256 identities. Recall, mean throughput, CV and tail latency were independently recomputed.",
  "冻结标签、延迟、协议、构建/加载记录、监督时钟和源码哈希绑定留存。召回、五轮计数/墙钟、CV及已测尾分位数均从完整原始证据独立复算。":
    "Frozen labels, latency, protocols, build/load records, supervisor clocks and source hashes are retained together. Recall, five-round counts/wall times, CV and measured tail quantiles were independently recomputed from complete raw evidence.",
  "十九个余弦数据集，227个线程测量位置，95组参数；原813个外部基线位置保持，共1,040个位置。外部基线仅三个核心库，未做其他十六库全交叉。":
    "The source snapshot contains 19 cosine datasets, 227 thread observations and 95 parameter groups. Combined with 813 external observations, it contains 1,040 positions. External comparisons cover the three core datasets.",
  "90/95/98%目标档均来自冻结调参集，未按正式评估重新挑选。总览固定95%预选档，实际召回和最高已测召回分别列示。":
    "90/95/98% target settings come from frozen tuning queries. The overview uses the 95% preselected setting. Actual recall and highest measured recall are listed separately.",
  "构图为八核原生API；首次SM安装单列。加载列为图加载API加重载后SM安装，不冒充统一冷启动时长。构建单次，查询五轮。":
    "Graph construction uses the eight-CPU native API. Initial SM installation is separate. Loading combines the graph-load API with SM installation after reload. One build and five query rounds were measured.",
  "保留原BQ与运行期SM均计热组件小计，共享图计一次。原FP32只精排计逻辑冷，仍可能全部驻留。图文件不含运行期SM，部署文件合计不重复加入临时派生状态。":
    "Retained BQ and runtime SM are included in the hot-component subtotal, with the shared graph counted once. Original FP32 is used for reranking and logically cold, although it may remain fully resident. Graph files exclude runtime SM, and deployment totals exclude transient derived state.",
  "热组件小计为已确认持久数组，不含全部管理数组、哈希桶、分配器与动态工作区。查询驻留取该位置五轮快照最大值，不能称连续峰值。":
    "The hot-component subtotal covers confirmed persistent arrays. Some management arrays, hash buckets, allocator overhead and dynamic workspaces remain separate. Query residency is the maximum snapshot across the five rounds at that point.",
  "Arxiv、CodeSearch、GooAQ原全库真值不适用于固定一百万前缀库，保留原文件后完整FP64余弦穷举重算，协议绑定新SHA。":
    "Arxiv, CodeSearch and GooAQ originally had ground truth for their full datasets. After preserving those files, full FP64 exhaustive cosine search recomputed truth for the fixed 1M prefixes. Protocols bind the new SHA identities.",
  "原43条BQ回退已作为历史保留。当前相同位置使用同一原图、同一原生产二进制及min(64,ef)精排预算重新测量，完整五轮、完整查询和四线程一致性已核验，当前曲线均为SM-L2。没有重构图、重跑其余184点或按新评估选择参数。原工作簿仍为修正前快照，当前HTML采用已核验增量。":
    "The original 43 BQ fallback observations remain in history. The same positions were remeasured using the original graph, production binary and min(64,ef) reranking budget. Five complete rounds, full queries and four-thread consistency were verified. Current curves use SM-L2. The other 184 positions and parameter selections are retained. The original workbook is a pre-correction snapshot, while this dashboard includes the verified overlay.",
  "VLDB稿明确面向cosine-native对比学习embedding，SIFT/GIST为预期的不适用边界。当前两库原生L2适配结果各8点，单独按度量归类；不与旧稿归一化余弦成绩混比，不以全库95%达标率评价QuIVer贡献。SM-L2是余弦任务低位重建评分路径名称，不代表通用原生L2适用承诺。":
    "The VLDB manuscript targets cosine-native contrastive embeddings. SIFT/GIST serve as expected applicability controls. Native-L2 results contain eight positions per dataset and are grouped by metric. The old normalized-cosine results refer to another metric. SM-L2 names the low-bit reconstructed scoring path for cosine tasks.",
});
