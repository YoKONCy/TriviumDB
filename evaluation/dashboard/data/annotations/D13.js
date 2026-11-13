// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.annotations["D13"] = [
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
    "result": "130d9d28749c730af44739db981cd5d2de6b51bf4157ae61c7fe600aec27bf7e",
    "protocol": "66619460a9753f21db523b24d5de2688092a2727b855ce462654f7138b461fbb",
    "index": "0571faf9cbccb36e806d5c480057b32c1522cc30fd4edc36aa770651a5ef91b3",
    "archive": "ab7df0fb824e6788365f175295545fe12f679cd0436e9f452b247eb083fdeee8"
  },
  {
    "预算修正": "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。",
    "参数选择": "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。",
    "原BQ对照": "原QPS 5660.942552791；原Recall@10 0.566564039409；原证据身份 130d9d28749c730af44739db981cd5d2de6b51bf4157ae61c7fe600aec27bf7e。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。"
  },
  {
    "result": "ae79030e6f53cb8a056fd47d6ac77b53fd6e4a3ccbe24413679861d8c853e709",
    "protocol": "323ef231901f03e7cb64e827fbff9cf3c96ff08d55ccb803f65a48ae03a79f2f",
    "archive": "c1f1a07ca00e739d7a036a9f23b97cc537fea9210780cd2a5c4ec216ce19adf7",
    "index": "0571faf9cbccb36e806d5c480057b32c1522cc30fd4edc36aa770651a5ef91b3"
  },
  {
    "吞吐边界": "原生请求完整搜索与精排；五轮均值。",
    "延迟边界": "单查询服务时钟，五轮完整评估查询。",
    "适用范围": null,
    "低宽度路由": null
  }
];
