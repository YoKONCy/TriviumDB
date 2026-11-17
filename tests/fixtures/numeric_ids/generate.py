"""按独立的当前 WAL 帧规范生成完整整数契约样本。"""

import json
from pathlib import Path
import struct
import zlib


ROOT = Path(__file__).parent


def string(value):
    encoded = value.encode("utf-8")
    return struct.pack("<Q", len(encoded)) + encoded


def insert(node_id, payload):
    return (
        struct.pack("<IQQff", 2, node_id, 2, 1.0, 0.0)
        + string(json.dumps(payload, separators=(",", ":")))
    )


def link(source, target, label):
    return (
        struct.pack("<IQQ", 3, source, target)
        + string(label)
        + struct.pack("<f", 1.0)
        + string("null")
    )


def write(name, records):
    data = bytearray(b"TVWL" + struct.pack("<H", 7))
    for record in records:
        data += struct.pack("<I", len(record))
        data += record
        data += struct.pack("<I", zlib.crc32(record))
    (ROOT / name).write_bytes(data)


if __name__ == "__main__":
    ids = [1, 9007199254740991, 9007199254740992, 9007199254740993]
    write(
        "wal.bin",
        [insert(i, {"group": "fixture", "exact_id": str(i)}) for i in ids]
        + [link(1, ids[2], "next"), link(1, ids[3], "next"), link(ids[3], 1, "back")],
    )
    write(
        "full_u64.bin",
        [insert(18446744073709551613, {
            "kind": "native_uint",
            "unsigned": 18446744073709551615,
            "nested": {"large": 18446744073709551613},
        })],
    )
