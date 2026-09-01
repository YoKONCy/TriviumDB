#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileRole {
    Tdb,
    Vec,
    FlushMarker,
    Wal,
    PropertyIndex,
    GraphIndex,
    Quiver,
    QuiverMeta,
    Text,
    TextMeta,
    Manifest,
}

impl FileRole {
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Tdb => "",
            Self::Vec => ".vec",
            Self::FlushMarker => ".flush_ok",
            Self::Wal => ".wal",
            Self::PropertyIndex => ".pidx",
            Self::GraphIndex => ".gidx",
            Self::Quiver => ".quiver",
            Self::QuiverMeta => ".quiver.meta",
            Self::Text => ".text",
            Self::TextMeta => ".text.meta",
            Self::Manifest => ".manifest.json",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldEncoding {
    Bytes,
    U8,
    U16Le,
    U32Le,
    U64Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub offset: usize,
    pub width: usize,
    pub encoding: FieldEncoding,
}

impl FieldSpec {
    pub const fn end(self) -> usize {
        self.offset + self.width
    }
}

pub const TDB_HEADER: &[FieldSpec] = &[
    FieldSpec {
        name: "magic",
        offset: 0,
        width: 4,
        encoding: FieldEncoding::Bytes,
    },
    FieldSpec {
        name: "version",
        offset: 4,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
    FieldSpec {
        name: "dim",
        offset: 6,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
    FieldSpec {
        name: "next_id",
        offset: 10,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "slot_count",
        offset: 18,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "payload_offset",
        offset: 26,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "vector_offset",
        offset: 34,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "edge_offset",
        offset: 42,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "bq_offset",
        offset: 50,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
];

pub const FLUSH_MARKER_V2: &[FieldSpec] = &[
    FieldSpec {
        name: "magic",
        offset: 0,
        width: 4,
        encoding: FieldEncoding::Bytes,
    },
    FieldSpec {
        name: "version",
        offset: 4,
        width: 1,
        encoding: FieldEncoding::U8,
    },
    FieldSpec {
        name: "generation",
        offset: 5,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "tdb_size",
        offset: 13,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "vec_size",
        offset: 21,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "tdb_crc",
        offset: 29,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
    FieldSpec {
        name: "vec_crc",
        offset: 33,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
    FieldSpec {
        name: "marker_crc",
        offset: 37,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
];

pub const WAL_HEADER: &[FieldSpec] = &[
    FieldSpec {
        name: "magic",
        offset: 0,
        width: 4,
        encoding: FieldEncoding::Bytes,
    },
    FieldSpec {
        name: "version",
        offset: 4,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
];

pub const PROPERTY_HEADER: &[FieldSpec] = &[
    FieldSpec {
        name: "magic",
        offset: 0,
        width: 4,
        encoding: FieldEncoding::Bytes,
    },
    FieldSpec {
        name: "format_version",
        offset: 4,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
    FieldSpec {
        name: "key_encoding",
        offset: 6,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
    FieldSpec {
        name: "main_size",
        offset: 8,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "main_crc",
        offset: 16,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
    FieldSpec {
        name: "node_count",
        offset: 20,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "field_count",
        offset: 28,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
    FieldSpec {
        name: "reserved",
        offset: 32,
        width: 4,
        encoding: FieldEncoding::U32Le,
    },
];

pub const GRAPH_HEADER: &[FieldSpec] = &[
    FieldSpec {
        name: "magic",
        offset: 0,
        width: 4,
        encoding: FieldEncoding::Bytes,
    },
    FieldSpec {
        name: "version",
        offset: 4,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
    FieldSpec {
        name: "reserved",
        offset: 6,
        width: 2,
        encoding: FieldEncoding::U16Le,
    },
    FieldSpec {
        name: "node_count",
        offset: 8,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
    FieldSpec {
        name: "block_count",
        offset: 16,
        width: 8,
        encoding: FieldEncoding::U64Le,
    },
];

#[test]
fn 所有固定规格字段不重叠且完全落在声明头部内() {
    for (name, fields, size) in [
        ("tdb", TDB_HEADER, 58),
        ("flush", FLUSH_MARKER_V2, 41),
        ("wal", WAL_HEADER, 6),
        ("property", PROPERTY_HEADER, 36),
        ("graph", GRAPH_HEADER, 24),
    ] {
        assert_eq!(fields.last().unwrap().end(), size, "{name} 头部大小漂移");
        for pair in fields.windows(2) {
            assert!(pair[0].end() <= pair[1].offset, "{name} 字段重叠: {pair:?}");
        }
    }
}
