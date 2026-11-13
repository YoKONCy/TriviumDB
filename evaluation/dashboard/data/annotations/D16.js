// 由 tools/update_data.py 生成，权威数据见 results/benchmark-snapshot.json。
window.QuiverBenchmarkParts.annotations["D16"] = [
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
    "result": "b941c6a693d7487da2a8e42148cec6e1ca7821abab1ebdbd4ee43a0d9eb66483",
    "protocol": "3a30aa4da4da6480260c970fbac3810a99ab6694c9f32366385c6376c683b65e",
    "index": "937a47574927e8f8c58dc240d98c509ce60b1b435a87b0ed5caaf4072d3ab8bf",
    "archive": "c2877b65d51aa3256cb611f41110c37fe161715f5338ae39e0b6b078c4356166"
  },
  {
    "预算修正": "原图、原二进制、原ef和完整查询，仅将精排请求改为min(64,ef)；直接SM参考和四线程门禁通过。",
    "参数选择": "仍使用原冻结位置；低ef旧调参走BQ的历史保留，不根据新评估成绩重选参数。",
    "原BQ对照": "原QPS 21988.285259836；原Recall@10 0.125537634409；原证据身份 b941c6a693d7487da2a8e42148cec6e1ca7821abab1ebdbd4ee43a0d9eb66483。旧成绩保存在内嵌历史对象，不再作为当前纯SM-L2曲线。"
  },
  {
    "result": "aa813feb02f8c2f0e00aa7182fd34b010e2d1eb82ac9cf6ef9cc24f971e98066",
    "protocol": "92b0307e2357cf5b07a9bfe54f51530d7303742d49a366507f12c827461b524a",
    "archive": "81c5d0c143a534b1577c82e8159011af9c75fbb89d966cd2fa418167ba2abcba",
    "index": "937a47574927e8f8c58dc240d98c509ce60b1b435a87b0ed5caaf4072d3ab8bf"
  },
  {
    "吞吐边界": "原生请求完整搜索与精排；五轮均值。",
    "延迟边界": "单查询服务时钟，五轮完整评估查询。",
    "适用范围": null,
    "低宽度路由": null
  }
];
