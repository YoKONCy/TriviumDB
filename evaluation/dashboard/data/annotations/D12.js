// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.annotations["D12"] = [
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
    "result": "bc603a4be4586fdeaa1cc55677bc3b2571aade8f7bafc6146f9ead8505c9c414",
    "protocol": "f01363a8a8b768ac18bf8f7641aa79de561685cfc3a3fa8fecd0827136969b59",
    "index": "de13d6b8bf6fa76488a8f43abfbf4d81c7d63bb4e304e05f8d3c8fea5f3c7fdb",
    "archive": "2d24299af7d1fd65e4f2dac07565feedb416d747c8c1620647e08b9e956a9d86"
  },
  {
    "预算修正": "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。",
    "参数选择": "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。",
    "原BQ对照": "原QPS 5489.143399952；原Recall@10 0.303897849462；原证据身份 bc603a4be4586fdeaa1cc55677bc3b2571aade8f7bafc6146f9ead8505c9c414。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。"
  },
  {
    "result": "9559d3bb06f96c16b72d468d36a8efbdd082a35b060d0b75e6d4aa252cc11c2b",
    "protocol": "9ffe431672052e90a1f950e31949ca090c9bf8bb6b022cd79a7f34b9e342e930",
    "archive": "03afc10415409c7bd27be2c28a55eb40b8a79a0cad33561ab543f962920044da",
    "index": "de13d6b8bf6fa76488a8f43abfbf4d81c7d63bb4e304e05f8d3c8fea5f3c7fdb"
  },
  {
    "吞吐边界": "原生请求完整搜索与精排；五轮均值。",
    "延迟边界": "单查询服务时钟，五轮完整评估查询。",
    "适用范围": null,
    "低宽度路由": null
  }
];
