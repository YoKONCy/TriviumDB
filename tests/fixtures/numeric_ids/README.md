# 完整 u64 ID 的跨语言样本

`wal_v3.bin` 是维度为 2、向量类型为 f32 的 WAL v3 文件。测试先建立相同 dtype/维度的空检查点，再将其复制为该数据库的 `.wal`，由实际安装的内核回放。

样本包含 ID `1`、`9007199254740991`、`9007199254740992`、`9007199254740993`；每个向量为 `[1, 0]`，Payload 为 `{"group":"fixture","exact_id":"<十进制 ID>"}`。节点 `1` 分别以标签 `next`、权重 `1`、Metadata `null` 指向最后两个节点。最后一个节点以标签 `back`、权重 `1`、Metadata `null` 指向节点 `1`。

样本通过 Rust 的 `Wal::open` 和 `Wal::append` 生成，依次追加四条 `WalEntry::Insert` 和三条 `WalEntry::Link`。Rust 回归检查全部记录的完整 ID、Payload、向量与边；Node 回归检查字符串 ID 及数字接口的精度边界。

`full_u64_v3.bin` 独立按 WAL v3 的固定宽度小端 bincode 帧和 CRC32 编码。它包含一个 f32 节点：ID `18446744073709551613`，向量 `[1, 0]`，Payload 为 `{"kind":"native_uint","unsigned":18446744073709551615,"nested":{"large":18446744073709551613}}`。Rust 直接回放并核对全部事实，Node 通过实际原生模块核对一等无符号整数、条件过滤和嵌套投影。
