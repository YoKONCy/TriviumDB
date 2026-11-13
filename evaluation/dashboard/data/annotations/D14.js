// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.annotations["D14"] = [
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
    "result": "a90181e5fa5a94dba21f1351568b31c0558dd785407334d6f77e8a8f2fc04dc5",
    "protocol": "e726057feb71f952def7124628b1318c5179b574ecee484c3f910a9c40f5d7e5"
  },
  {
    "热组件口径": "实际头部与底图容量小计",
    "导航与精排": "FP32参与图搜索",
    "内存组件": "冻结SDK详情总长度匹配索引；实际主数组capacity=5013504。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热",
    "未归因开销": "SDK已报告总量尚有422.7 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "637f49f0d75b51b4952b9529a3084d9a41b7173061e8d543c0f6878e8a9f1b11",
    "protocol": "5e11ffb0f871215646d8e860bda181b88ef559f7cc95a029a03c4b91db146998"
  },
  {
    "热组件口径": "实际头部与底图容量小计",
    "导航与精排": "SQ8导航；自有FP32精排",
    "内存组件": "冻结SDK详情总长度匹配索引；实际主数组capacity=5013504。底图每行含计数；精排预留容量也计入冷组件。SDK包含其他组件，不等于纯热",
    "未归因开销": "SDK已报告总量尚有422.7 MiB未分配到上述主数组，含上层图、锁及label_remap等；此外完整allocator/工作区未闭合",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "371e9531d2df1daad0c06392de796d9eb6c01c3eede6a9042fa931a3c1fcc552",
    "protocol": "093f0db1f1f7e805d419a92de569eebad6d7d7cab80c762a748c6cbbd7c0eba2"
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
    "result": "13876b9527b33339ef5e6e0494f15b596597f80603c66fe2a78fe113775963a3",
    "protocol": "6f674d0112c8fa76ead344abd1ed3544944b4380728611e4722fe56575a216fe"
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
    "result": "a6e6029c4784665f775605921372061dfc3945560f5e93e1d2a0d0b384869994",
    "protocol": "bb208ab920a2f2e391c68a9d93df786140bbab1e543182ad12e306b8dae9dfb5"
  },
  {
    "result": "e7c814ea8a48bfcbccdade09210f8b9d78bc93269397014d8a530efc04f6959b",
    "protocol": "c0120e307ab022eb8511462538ee0763de03190a621bf22e3a24b4b0e376ed8c"
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
    "result": "3eac0547998bca753badba0b97162f557f552999dd01ce8734f84c173142f975",
    "protocol": "a18504a892b4ea48e544bcb682f93dc107cd3de9cfc597a2cf926d54b1a01040"
  },
  {
    "result": "12e02dc05f89dc2d1a5c5f6d908effe1d031fa4d0eb43919fa7465f305a22466",
    "protocol": "cb98070b248650beb8fe0cb8c12b6718d3f41b65ec0c177b37240f63a698014c"
  },
  {
    "result": "f294aec0cbc07231417f30cffd004dd4f91fc58c7b9a7fcaa886d077a5438003",
    "protocol": "ae50efd7f68e659c67dd6a7444316dadddaa9b5799c1eb3c38da9efd071abb67"
  },
  {
    "result": "8a1b032ce6a97e9c5d687af18320e4cfff031ab838423c7e22a4e4cfb47c9756",
    "protocol": "9b36f9b40c17d82e8ca4d8d8e5589a3fc11cd961cd23a40207fc2fc66e219c8d"
  },
  {
    "result": "5028c30eb0bb6bf0a52ec935872470eb15fdef13929a51c7863aefe012ac65c5",
    "protocol": "fe4d38f4806e1b19266b737ba72c0aa8bf706fd2db3001d1efa33b4010aa2dbd"
  },
  {
    "热组件口径": "对齐图与导航主数组小计",
    "导航与精排": "FP32参与图搜索",
    "内存组件": "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=384字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图",
    "未归因开销": "图锁、删除标记、PQ码本/实际容量、搜索队列/visited与分配器；主数组不是完整进程内存",
    "构建计时": "官方图插入total_time加PQ量化训练时长，二者单列；读入和保存额外含于原生进程墙钟，不将总进程时间冒充独立加载或保存API",
    "保存计时": "保存包含在原生进程墙钟内，现有记录不能分离出保存时长",
    "加载计时": "纯DiskANNIndex::load_with的异步API墙钟；不含CLI启动、输入检查、配置元数据读取、证据导出、查询及退出",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "810d1064a8311e19ff5e2b7e81cff75830827f65d0b52aa036f2ae378bb4d24a",
    "protocol": "02f1096a0114d4686b7249326a11926aa123f160744303ef2bbd8003613d76ca"
  },
  {
    "热组件口径": "对齐图与导航主数组小计",
    "导航与精排": "PQ导航；自有FP32只用于最终精排",
    "内存组件": "冻结加载源码与24字节头核对：图行按64字节补齐，实际stride=384字节，含1个起点及尾部对齐余量；图文件字节不再替代内存图",
    "未归因开销": "图锁、删除标记、PQ码本/实际容量、搜索队列/visited与分配器；主数组不是完整进程内存",
    "构建计时": "官方图插入total_time加PQ量化训练时长，二者单列；读入和保存额外含于原生进程墙钟，不将总进程时间冒充独立加载或保存API",
    "保存计时": "保存包含在原生进程墙钟内，现有记录不能分离出保存时长",
    "加载计时": "纯DiskANNIndex::load_with的异步API墙钟；不含CLI启动、输入检查、配置元数据读取、证据导出、查询及退出",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "eee91a54887bf88c6621f2b5646b8860f430e042ddd427c301f93a85cb19616e",
    "protocol": "aabdd0a3cd21750a8a29742021d7dd10b853466eeb00b66a1d7a7f9416bc15be"
  },
  {
    "result": "4cf74fe47359f1dee3687c942c1f302d867958d8b7e410ada05d3f6c967e6cc4",
    "protocol": "83a29d2da6f0f834918fc2123b367cd576a48bb49a8dafaf7142ac0d8042c918"
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
    "result": "1193fa0065b0e463db14415b961d06ae79234a17773fc4b43b82c2c7be4114dd",
    "protocol": "559d90c02ec41368dc22062f331c60f81ebcb52cc8907a910f921cf057f3cd3a"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "1-bit导航；外部FP32精排",
    "内存组件": "已逐数组读取size/capacity：含上层边/偏移、倒排目录、FastScan块补齐与预计算表（按实际类型）；非文件字节估计",
    "未归因开销": "仍缺类对象、分配器隐藏开销和查询工作区；重载数组容量不是构建峰值",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "0288fc9545fb14d99a8ff4a93a8958b96fe9e306939aea0870b59ef5c56fd910",
    "protocol": "88441d2f56eb85ad9f7704d473fe5cba3bc8f69a9f01f4ba2d0f793852e9c3e7"
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
    "result": "1cbbbe7347e891419fce28b4a6413a26e3af1a30056eb563f25e96264a615c83",
    "protocol": "247177c9da16eadd73c6f4d89147484b51bd09155e510985d5af8dbc04a559d0"
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
    "result": "295efa03c9ae9ea8c066528280cfcca67dcf31e0e36ecdad8dff1d4380bef1f2",
    "protocol": "c7f7d0b8a420f4b4c6b687d394f72b72646b8c8543b7b6a2807aa1a40d9e2717"
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
    "result": "4c74e6b9595a021999af32302c4b6d2bba13f03d9e8df4c0b2050a0c3ac728e1",
    "protocol": "a8875e4d1fd580caaa6c8cd2857e4e6df3a315a3de013192fe29b30046e7d27f"
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
    "result": "66d895d978bf4aad6247d49c9f21fb749711151950c1ffaae3198ac529b4544f",
    "protocol": "4072d4366c01fdd8ad1cd764062c0891f445a04f762f7df7a54a6ab35ddadae9"
  },
  {
    "result": "3cc47724a492eea0b7e2f1dbc0824acbb6a0319229ae0704733bbe15d06e7d00",
    "protocol": "1dd9baaac0fb7e1b797035322ddd497faaba73c832f9615c1221f185af247c6a"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "主码导航；残差码仅最终重排；无全库FP32",
    "内存组件": "图搜索用PrimaryAccessor；随后get_datum重排才读残差。残差计逻辑冷，量化码总量含主码与残差；驻留已加hugetlb",
    "未归因开销": "查询工作区、容器容量及分配器开销未完整拆分",
    "构建计时": "复用原官方LVQ量化与Vamana构建API实测墙钟；未重新建图",
    "保存计时": "保存包装另含组件哈希，原生API时间独立列示",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "7d3b0712d4fa43abcc17ca8b246c443e3513abc918e8a522f525223e41b57cff",
    "protocol": "9fd895136365b7139bda3b4c47496c9e673b982b6b862890a7b17aff8edc07dc"
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
    "result": "f499d632e947c30a9478e256f5d7286c7e7b3935adfda2b6c280ef0a40fa8551",
    "protocol": "bca0c6604b044f805b52432e23f123f2a277dab178fbd1462233ec8df60c1d59"
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
    "result": "00a425203b80ceb19a9ba33f3d65b8c69fcce7127352fe55144d825236f9591c",
    "protocol": "1417bbc91dbb0819df4178f0da3d5a4848f91f593dfe89b41ca6beff78a6534b"
  },
  {
    "热组件口径": "自有tape已分配容量小计",
    "导航与精排": "FP32参与导航",
    "内存组件": "官方同版本memory_stats实测：未用预留1.765 GiB、对齐/旧arena空隙237.2 MiB，均已包含在分配容量中，不能再相加；未触碰页不等于驻留",
    "未归因开销": "节点指针/锁、向量lookup与key哈希、实际查询队列/visited、线程上下文及自有mmap另计；mallinfo2不覆盖自有mmap",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "原始时钟仅包围保存函数；包含其序列化/文件写出及绑定，完整文件哈希在计时外",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "b9607f17a6b61eb8ef320b64fcbf8925b4c546c3d17c4018debf205dbf8e01a6",
    "protocol": "0f4fe04fd46855434e3cf69a810efd4c74634f35fa021967f0f558e76df2736c"
  },
  {
    "热组件口径": "量化图与树导航数组下界",
    "导航与精排": "树自有内部FP32 pivot选叶；局部量化图导航；全库FP32仅最终精排。叶pivot保留但SearchLeaf不读取。",
    "内存组件": "三库TreeQuantization=True、非classic路径已核实；树节点自有FP32 pivot单列热，全库原向量仅最终精排；所有qg/grp及tre解析长度严格匹配",
    "未归因开销": "仍需另计原图/叶pivot等加载保留、量化查表与旋转器、对象分配对齐、visited池/候选缓存；非查询必要保留不能算纯热",
    "构建计时": "ANNG、图重构及量化图创建/构建命令墙钟；命令内部已持久化，文件哈希另计",
    "保存计时": "保存已包含在构建命令或commit中；不另填0秒",
    "加载计时": "保持原包装加载边界，逐库定义另核对；不擅自改称统一纯C++API或完全预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后快照最大值，非连续峰值；FAISS无逐点快照则保留加载/门禁后驻留，不插值补数。"
  },
  {
    "result": "64155ef5eefc44aa677e0ee049690ad435018a4e55f90157e2e5a861ced94271",
    "protocol": "1efa26e44d9b87d878108a5d7d026bacc9427a36199cbb6ad8133be959715d6e"
  },
  {
    "热组件口径": "重载后数组容量小计",
    "导航与精排": "1-bit导航；外部FP32仅精排",
    "内存组件": "同版本只读工具实读码、上下层图、层级、偏移和中心容量；不含隐藏运行时开销",
    "未归因开销": "类对象、分配器和查询工作区仍另计",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "保存包装时钟；不把缺少的内层API时钟补造为0",
    "加载计时": "冻结库包装加载边界；外部向量映射按需触页",
    "冷加载确认": true,
    "驻留口径": "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值"
  },
  {
    "result": "08ac2d98dcb964c7e12fff6f5dedebd9bff9e128d26117d2f83681f746c84fc0",
    "protocol": "4ea3e8ee3432bb5ca0d1672a71bee8ffc57e41fc25bf73031677e333a94cb436"
  },
  {
    "热组件口径": "持久组件精确合计",
    "导航与精排": "LVQ主码导航；无全库FP32",
    "内存组件": "已核完整组件SHA及文件头；主码含对齐/缩放、图含容量、centroid计热；剔除文件头",
    "未归因开销": "类对象、分配器和查询工作区仍另计",
    "构建计时": "官方LVQ量化和Vamana构建API墙钟；冻结源哈希、格式转换及保存独立记录",
    "保存计时": "保存包装墙钟含完整组件哈希；官方保存API时长单列",
    "加载计时": "冻结库包装加载边界；外部向量映射按需触页",
    "冷加载确认": true,
    "驻留口径": "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值"
  },
  {
    "result": "cb48aa9ba98241ccf76822deec2d7dd7188760002383506f407f7445b9dd9df6",
    "protocol": "ac6626a00f418398eecbda8c3f1af034eb1cd6f59c5c3625b34850c63c155907"
  },
  {
    "热组件口径": "图行与容器描述符下界",
    "导航与精排": "FP32参与导航",
    "内存组件": "完整SHA、逐行度数、行数和文件末尾核对通过；含邻居ID及至少每行24字节vector描述符",
    "未归因开销": "外层vector额外容量、boost bitset、候选队列、分配器及映射驻留另计",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "保存包装时钟；不把缺少的内层API时钟补造为0",
    "加载计时": "冻结库包装加载边界；外部向量映射按需触页",
    "冷加载确认": true,
    "驻留口径": "普通RSS加私有与共享hugetlb；保存的最大快照不是连续峰值"
  },
  {
    "result": "468773f2ff5bae518e029838ea77e10b5f8253408b39b5816c9ece5aa3aefc16",
    "protocol": "f4e4c6336650e9a22b6ef6416cdb787e50fe29995c2d22d8d1263ea37e73f822"
  },
  {
    "热组件口径": "图行与容器描述符下界",
    "导航与精排": "FP32参与导航",
    "内存组件": "逐行解析完整图且核对SHA、行数和文件末尾；邻居ID与至少每行24字节vector描述符计热，完整FP32用于导航",
    "未归因开销": "外层vector额外容量、实际查询boost bitset/候选队列、分配器及映射驻留另计；映射回收不减少逻辑导航组件",
    "构建计时": "新进程中官方构建API墙钟，含首次读取训练mmap；索引序列化和输入完整哈希另计",
    "保存计时": "保存时钟包围仅委托原生保存的函数，包含Python包装/绑定；不是更窄的C++内部时钟，完整哈希在计时外",
    "加载计时": "包装创建图及FP32映射的墙钟；图文件和导航FP32均先驱逐并核0驻留，映射按需触页，非全库预载",
    "冷加载确认": true,
    "驻留口径": "保存的查询后RSS加私有/共享hugetlb快照；最大快照不是连续峰值"
  },
  {
    "result": "ecb1d75c5aab15652024141aa9c73bc6226244559434dd5a7548c27753c05023",
    "protocol": "205f29d9bee9fa4996846f00b013b61450ec9e01e8aa2e25849f18bfb25d476f"
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
    "result": "6058ae8e380838e4d982f31d4ce910827fdf5fa0880156eb7499fed5b62d790f",
    "protocol": "987968fbc397ccb0d5342fea7557b9ddfa41215aeb40c896ce6991b73ed6fc07",
    "index": "ad51d6c3cac39e25b53c900d816951785a5b816e9fc11ab2887220bcfec7b435",
    "archive": "e84671bce736f5d56f6e904efa6389d2a55aa7c0191ed1c0899d2bf6302976e0"
  },
  {
    "吞吐边界": "Java批量查询核心墙钟，含任务派发、结果数组和标签复制；不含跨进程IPC、查询文件读入与结果文件写出，原始请求墙钟另记",
    "延迟边界": "Java单查询服务时长，含Lucene图查询与FP32精排，不含任务排队和IPC",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "a90181e5fa5a94dba21f1351568b31c0558dd785407334d6f77e8a8f2fc04dc5",
    "protocol": "e726057feb71f952def7124628b1318c5179b574ecee484c3f910a9c40f5d7e5",
    "archive": "3e01ba892dc9a95f3ef7b3acfbaa9dc821fef13b3a6ac90f1329b1123401bf01"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含查询Dataset准备、官方搜索及标签复制；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "637f49f0d75b51b4952b9529a3084d9a41b7173061e8d543c0f6878e8a9f1b11",
    "protocol": "5e11ffb0f871215646d8e860bda181b88ef559f7cc95a029a03c4b91db146998",
    "archive": "b4f9d3e38fbd415432c098ff176e866e959558af063ca693220c0e474cec5d88"
  },
  {
    "result": "371e9531d2df1daad0c06392de796d9eb6c01c3eede6a9042fa931a3c1fcc552",
    "protocol": "093f0db1f1f7e805d419a92de569eebad6d7d7cab80c762a748c6cbbd7c0eba2",
    "archive": "666d4717f1c51ae5bc8602dc1f09a5ec2e47ef6479a87156b0ce12f040a1874d"
  },
  {
    "吞吐边界": "主qps为官方批量Index::search原生墙钟；service另列包装吞吐，包含分配/线程创建/标签验证及开启的逐查询时钟",
    "延迟边界": "逐查询服务API纳秒时钟；对应service路径，与官方批量QPS不是同一执行路径",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "13876b9527b33339ef5e6e0494f15b596597f80603c66fe2a78fe113775963a3",
    "protocol": "6f674d0112c8fa76ead344abd1ed3544944b4380728611e4722fe56575a216fe",
    "archive": "e6921bba4c1ccf8e8b61644ecf87f636f8e71475426774cef6f6fba1f8f1495a"
  },
  {
    "result": "3a0884510550aea4f82585dcad386c432aa29b84b1896e2cb815a2bf43d26dcc",
    "protocol": "9e418bd2d820de46fe7067da689c0b6a175b0be2035bd4c935839b93b0116dea",
    "archive": "4b06a55066ca4999505378117c4e991ad955a4996e6d1ac7cf3872bfcc869415"
  },
  {
    "result": "a6e6029c4784665f775605921372061dfc3945560f5e93e1d2a0d0b384869994",
    "protocol": "bb208ab920a2f2e391c68a9d93df786140bbab1e543182ad12e306b8dae9dfb5",
    "archive": "6ea6874dbda358a0f3d193e0ab768ad466b83a2cf465514d19263237fc331979"
  },
  {
    "result": "97fd572b5bd72e40ac7f427f16cc45d131ef8b709d012b4c0b2bf45770999a0b",
    "protocol": "70b25d312df408e53b30f1b54c4bda4388bd309421487aec0dbf3adcf83cfe3b",
    "archive": "f28e9d257a90c8208bca9db5bea6a41f880aad4053ceaec80ef2b3e7ddbbb197"
  },
  {
    "result": "e7c814ea8a48bfcbccdade09210f8b9d78bc93269397014d8a530efc04f6959b",
    "protocol": "c0120e307ab022eb8511462538ee0763de03190a621bf22e3a24b4b0e376ed8c",
    "archive": "96fd61bc4d00989bfa7bbbf77ac3f895ab28ba9aaf80c0f36b6b5e2bafe12936"
  },
  {
    "result": "620ea970c38d56f4f27bbf7377da656f17830ba6627da2e891c4ecec65c8fc96",
    "protocol": "edd735afb4c2d3c0934a786ac5e7c9ca1b938fe1c4e0f2ffb1bf5bae181daff2",
    "archive": "532328ce385ab5540a9fe2f1fcd03c144271948904b11bac7c81a7c4c94ecad6"
  },
  {
    "result": "3eac0547998bca753badba0b97162f557f552999dd01ce8734f84c173142f975",
    "protocol": "a18504a892b4ea48e544bcb682f93dc107cd3de9cfc597a2cf926d54b1a01040",
    "archive": "5e0ca0a3fbabf4166b0d03a53325c74de47fb99b19186ef7dc7783ddbaf8f977"
  },
  {
    "result": "850ce8082c19bfb5a5f78f2cbc1b11d088a3ad5c9585e9b4285e4f143040ba7a",
    "protocol": "2409431cfe1e87dbb50ddaf9bfeaf70069fdaa57e5595135d8e0130f9e242c27",
    "archive": "d6a6ec5d169acd4840ff619b7e5e8c4b4045ce2676bb55a3f63aa6cade5a06e9"
  },
  {
    "result": "12e02dc05f89dc2d1a5c5f6d908effe1d031fa4d0eb43919fa7465f305a22466",
    "protocol": "cb98070b248650beb8fe0cb8c12b6718d3f41b65ec0c177b37240f63a698014c",
    "archive": "a49836f955bee3ea434b9923f77668eb6cb8392c32d801e2f976efd13d2d7e2e"
  },
  {
    "result": "3a6862457b1f6a528d9143d6276e9b364595e6b184318ffffca406d7c79871c2",
    "protocol": "7fc80bee3514072e5394eb1edeaea80ad1759b446eba4d092e56ab71383f65fd",
    "archive": "fde99a9b6c3c7c059e3efb657698b1e8742d2bb3bd0f7e82bc0ba8236a01dafe"
  },
  {
    "result": "f294aec0cbc07231417f30cffd004dd4f91fc58c7b9a7fcaa886d077a5438003",
    "protocol": "ae50efd7f68e659c67dd6a7444316dadddaa9b5799c1eb3c38da9efd071abb67",
    "archive": "ad57b06c9f2b06625a0352375f61c63241e94a59054f01566b28662109080980"
  },
  {
    "result": "f04399dbe1332eebd4ac45e3c486e6b4fad686e4b9a26444c8b9aa31ccf97a27",
    "protocol": "768355c8e12f0127ab4738360980395bc16fda0fc3ca287d701b7ae3e11de0f3",
    "archive": "e6ebaed05e0bff279306ba75f1360c7817a39988a377a27aaa30e62254454e02"
  },
  {
    "result": "8a1b032ce6a97e9c5d687af18320e4cfff031ab838423c7e22a4e4cfb47c9756",
    "protocol": "9b36f9b40c17d82e8ca4d8d8e5589a3fc11cd961cd23a40207fc2fc66e219c8d",
    "archive": "00cccf64bbcb702cf33ec70dd8c3eb085589d99a984664cdef471221e7fe3710"
  },
  {
    "result": "b443725bc6908a86b7344e5908860dc01c697244f82b0ae5714b334979bf085b",
    "protocol": "80a56220579d328ef791405633a0dc1ee215d58f1e34d2f3253433244c375d66",
    "archive": "915929b4fc8fd081fa8ef6ef8cabdae999579b23d8a5c3aa8c3e626777dc0dca"
  },
  {
    "result": "5028c30eb0bb6bf0a52ec935872470eb15fdef13929a51c7863aefe012ac65c5",
    "protocol": "fe4d38f4806e1b19266b737ba72c0aa8bf706fd2db3001d1efa33b4010aa2dbd",
    "archive": "1acb88a86abcaf0bcdfbba50c9b9290987e190062d9851d1e14780441f9cd9d8"
  },
  {
    "result": "4570518b421fdc4c475d66dcd21ab1c8f385f5a2cf9fa86dd045cbb49abbdca6",
    "protocol": "f9c7d3b6f5e2dc78206c04e9d222bb5b84939bdb98ad4b3f73f1882b63d9d1c7",
    "archive": "ecc7d9283db5b287425f42dba842692ec64a7b370370b1176356f54ccd0bc191"
  },
  {
    "吞吐边界": "原生每轮重复官方批API的墙钟，含任务调度、结果分配与释放；逐查询时钟关闭；不含加载、文件IO导出和真值计算",
    "延迟边界": "官方逐查询服务整数微秒，单批完整查询五轮，含搜索与编号缓冲写入，不含排队及Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "810d1064a8311e19ff5e2b7e81cff75830827f65d0b52aa036f2ae378bb4d24a",
    "protocol": "02f1096a0114d4686b7249326a11926aa123f160744303ef2bbd8003613d76ca",
    "archive": "cd5e6346a6f519e49e36b3e49705e6ce735940fad37d2c38cbfb88eef0f64b9f"
  },
  {
    "吞吐边界": "原生每轮重复官方批API的墙钟，含任务调度、结果分配与释放；逐查询时钟关闭；不含加载、文件IO导出和真值计算",
    "延迟边界": "官方逐查询服务整数微秒，单批完整查询五轮，含搜索与编号缓冲写入，不含排队及Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "eee91a54887bf88c6621f2b5646b8860f430e042ddd427c301f93a85cb19616e",
    "protocol": "aabdd0a3cd21750a8a29742021d7dd10b853466eeb00b66a1d7a7f9416bc15be",
    "archive": "dcea317eaa9efb080f343d7e4020bc5d39d4ef984efd82cb44567264169f387b"
  },
  {
    "result": "4cf74fe47359f1dee3687c942c1f302d867958d8b7e410ada05d3f6c967e6cc4",
    "protocol": "83a29d2da6f0f834918fc2123b367cd576a48bb49a8dafaf7142ac0d8042c918",
    "archive": "c9a8fc433585cee274063c59723ba1935771a33d509b3396e78d9f0df0d4a5c5"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含查询Dataset准备、官方搜索及标签复制；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "1193fa0065b0e463db14415b961d06ae79234a17773fc4b43b82c2c7be4114dd",
    "protocol": "559d90c02ec41368dc22062f331c60f81ebcb52cc8907a910f921cf057f3cd3a",
    "archive": "e704efaf294263a43332db1a92f9dd03dd43fa076a5766da56bbf174c9f33c91"
  },
  {
    "result": "3563f42ee69c3500df6551d955a22bd92565bdb1ee35b252c7c91aa37ee9eb18",
    "protocol": "73a512a605617b1ba54e7db94d72360eeb35efd941ea49b5cb4ba7a6a22a4743",
    "archive": "d32b6f54ad8ea89dc88d16deff6c011c6860ab1cba7171a09757bfc0a215621f"
  },
  {
    "result": "0288fc9545fb14d99a8ff4a93a8958b96fe9e306939aea0870b59ef5c56fd910",
    "protocol": "88441d2f56eb85ad9f7704d473fe5cba3bc8f69a9f01f4ba2d0f793852e9c3e7",
    "archive": "7ed2d3d68b7504cc0b44f0a2c4bd0a0f322ffff70014119a2c539e97d2376359"
  },
  {
    "result": "1cbbbe7347e891419fce28b4a6413a26e3af1a30056eb563f25e96264a615c83",
    "protocol": "247177c9da16eadd73c6f4d89147484b51bd09155e510985d5af8dbc04a559d0",
    "archive": "b4b25b71094c9d6589b47968ef67034879b5954704ce3f28771fa06265cf41f2"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "295efa03c9ae9ea8c066528280cfcca67dcf31e0e36ecdad8dff1d4380bef1f2",
    "protocol": "c7f7d0b8a420f4b4c6b687d394f72b72646b8c8543b7b6a2807aa1a40d9e2717",
    "archive": "c569073ea4d6c01070da0a71d320c2895ffaf65af11e0ec192b0975d69f7338a"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方单查询搜索及写出编号/距离；不含应用排队和Python",
    "延迟可用性": "unsupported_author_global_statistics",
    "线程环境待核": false
  },
  {
    "result": "4c74e6b9595a021999af32302c4b6d2bba13f03d9e8df4c0b2050a0c3ac728e1",
    "protocol": "a8875e4d1fd580caaa6c8cd2857e4e6df3a315a3de013192fe29b30046e7d27f",
    "archive": "64adc07813973dda2343cf0f400e792531b2935b9651f711d7aabe1c8b1ed517"
  },
  {
    "result": "66d895d978bf4aad6247d49c9f21fb749711151950c1ffaae3198ac529b4544f",
    "protocol": "4072d4366c01fdd8ad1cd764062c0891f445a04f762f7df7a54a6ab35ddadae9",
    "archive": "5604f53dac09569de6cd14ca6a54d8c11a4c23c20cd649cb0e1239cdbbb6b3db"
  },
  {
    "result": "3cc47724a492eea0b7e2f1dbc0824acbb6a0319229ae0704733bbe15d06e7d00",
    "protocol": "1dd9baaac0fb7e1b797035322ddd497faaba73c832f9615c1221f185af247c6a",
    "archive": "5b45fb0ce54a3f1d9b35c0666943b893948c667757f0a6e5e7275cbd42b1cf89"
  },
  {
    "吞吐边界": "官方SVS Python批量API包装墙钟，含结果分配、缓存参数与编号核验；逐查询时钟关闭，官方常驻线程池",
    "延迟边界": "SVS官方SDK原生逐查询计时未支持，所有P95/P99明确保留缺项",
    "延迟可用性": "unsupported_official_sdk_native_latency",
    "线程环境待核": false
  },
  {
    "result": "7d3b0712d4fa43abcc17ca8b246c443e3513abc918e8a522f525223e41b57cff",
    "protocol": "9fd895136365b7139bda3b4c47496c9e673b982b6b862890a7b17aff8edc07dc",
    "archive": "48b15f3e443bf0c559b796e1087646f9a6e4dd289e56be782899e3a328ee00b1"
  },
  {
    "吞吐边界": "官方SVS Python批量API包装墙钟，含结果分配、缓存参数与编号核验；逐查询时钟关闭，官方常驻线程池",
    "延迟边界": "SVS官方SDK原生逐查询计时未支持，所有P95/P99明确保留缺项",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "result": "f499d632e947c30a9478e256f5d7286c7e7b3935adfda2b6c280ef0a40fa8551",
    "protocol": "bca0c6604b044f805b52432e23f123f2a277dab178fbd1462233ec8df60c1d59",
    "archive": "74f432355f5551ed3a3f468e5d150cc4f5345b8d533446a90fe6ca753402661e"
  },
  {
    "result": "00a425203b80ceb19a9ba33f3d65b8c69fcce7127352fe55144d825236f9591c",
    "protocol": "1417bbc91dbb0819df4178f0da3d5a4848f91f593dfe89b41ca6beff78a6534b",
    "archive": "abdf853a45334452da02d5d56374608110f54cd33d0c13c2e3b45184db3a6ba9"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方cosine搜索及编号/距离复制；不含应用排队和Python",
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "b9607f17a6b61eb8ef320b64fcbf8925b4c546c3d17c4018debf205dbf8e01a6",
    "protocol": "0f4fe04fd46855434e3cf69a810efd4c74634f35fa021967f0f558e76df2736c",
    "archive": "ebfa47ecd930cf095ae616cf51660d30ba591e0214284ffe3ec25f4ae66cff7c"
  },
  {
    "result": "02750a477d9c532893d60a7e77b089625d2cf074628fb9c3cf6e375d03ba400f",
    "protocol": "ac03d358ef40ab2f534476f48ea6c5bffffb373849fff75c746adf1e01bc1def",
    "archive": "f4c02807d9779d552e8767338859565e9d53987d7a0d242d9b07714e1bcbf41c"
  },
  {
    "吞吐边界": "Python调用C++批量搜索的墙钟，含线程创建、返回数组及跨语言开销；逐查询时钟关闭",
    "延迟边界": "C++内单查询服务时长，含官方cosine搜索及编号/距离复制；不含应用排队和Python",
    "延迟可用性": "not_measured_by_authorized_scope",
    "线程环境待核": false
  },
  {
    "吞吐边界": null,
    "延迟边界": null,
    "延迟可用性": "measured_native",
    "线程环境待核": false
  },
  {
    "result": "64155ef5eefc44aa677e0ee049690ad435018a4e55f90157e2e5a861ced94271",
    "protocol": "1efa26e44d9b87d878108a5d7d026bacc9427a36199cbb6ad8133be959715d6e",
    "archive": null
  },
  {
    "result": "08ac2d98dcb964c7e12fff6f5dedebd9bff9e128d26117d2f83681f746c84fc0",
    "protocol": "4ea3e8ee3432bb5ca0d1672a71bee8ffc57e41fc25bf73031677e333a94cb436",
    "archive": "4ec66fe75b71988d41ae1785b3ec381eed4e4cf5b8c7a2fc557817980faea8f5"
  },
  {
    "result": "cb48aa9ba98241ccf76822deec2d7dd7188760002383506f407f7445b9dd9df6",
    "protocol": "ac6626a00f418398eecbda8c3f1af034eb1cd6f59c5c3625b34850c63c155907",
    "archive": "70d120d74c9b1e49b2702c0408a8ce833736b165c41b912045aac02ffcfa68f6"
  },
  {
    "result": "7ff3428aa8a3a5b3c36fee85e1443be8bd48984be1a18e79498351e66989d52a",
    "protocol": "e123a61a9323b3989b62bbd8a5fc0a00d256eb27dd9e11c9f9e1ed6c9d17f6ba",
    "archive": "4d88c20ab56dcc1d71428226ef43a407eed1c678eaacebc388af58c7502ba291"
  },
  {
    "result": "468773f2ff5bae518e029838ea77e10b5f8253408b39b5816c9ece5aa3aefc16",
    "protocol": "f4e4c6336650e9a22b6ef6416cdb787e50fe29995c2d22d8d1263ea37e73f822",
    "archive": "6b55ed8c682f829710a7e6be8b37480647f7da95a2b1bf2d73535141866fffc5"
  },
  {
    "result": "ecb1d75c5aab15652024141aa9c73bc6226244559434dd5a7548c27753c05023",
    "protocol": "205f29d9bee9fa4996846f00b013b61450ec9e01e8aa2e25849f18bfb25d476f",
    "archive": "730fef4e7be7fa78c14bdbbaaea4ee19dc67fe188004c753160711bc2b68ef7c"
  },
  {
    "预算修正": "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。",
    "参数选择": "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。",
    "原BQ对照": "原QPS 6502.414110342；原Recall@10 0.860215053763；原证据身份 6058ae8e380838e4d982f31d4ce910827fdf5fa0880156eb7499fed5b62d790f。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。"
  },
  {
    "result": "a85a7301d10f1267cce2da747b17186b49ea57537593d9efe5646ab8ed1bbc3b",
    "protocol": "9c225e9a5f9575769a0d83a34c1ea31923369024d897ca54d6b6582a5a1180e7",
    "archive": "01ae9acf8cd86136e87adca47ca100eae554cda4632fbe403949a42a6b66866a",
    "index": "ad51d6c3cac39e25b53c900d816951785a5b816e9fc11ab2887220bcfec7b435"
  },
  {
    "吞吐边界": "原生请求完整搜索与精排；五轮均值。",
    "延迟边界": "单查询服务时钟，五轮完整评估查询。",
    "适用范围": null,
    "低宽度路由": null
  }
];
