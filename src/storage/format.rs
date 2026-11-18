//! 当前格式家族的准入。历史解码由外部迁移工具维护。

use crate::{Result, TriviumError};
use std::fs::File;
use std::io::Read;

pub(crate) fn require(component: &'static str, found: u32, current: u32) -> Result<()> {
    if found == current {
        return Ok(());
    }
    if found > 0 && found < current {
        return Err(TriviumError::MigrationRequired {
            component,
            found,
            current,
        });
    }
    Err(TriviumError::UnsupportedSidecarVersion {
        component,
        found,
        minimum_supported: current,
        current,
        action: "请使用支持该格式的工具检查来源 (Inspect the source with a tool supporting this format)",
    })
}

/// 创建协调文件或执行发布恢复前检查格式身份；取得租约后再次检查。
pub(crate) fn admit(path: &str) -> Result<()> {
    if std::fs::symlink_metadata(format!("{path}.migration")).is_ok() {
        return Err(TriviumError::DatabaseLocked("迁移目标等待发布确认，请通过 scripts/migrate.py 重试 (Migration destination awaits publication confirmation; retry scripts/migrate.py)".into()));
    }
    let recovering = std::path::Path::new(&format!("{path}.publish")).exists();
    for (suffix, magic, width, current, component) in [
        (".publish", &b"TPUB"[..], 1, 3, "publication"),
        (
            "",
            &b"TVDB"[..],
            2,
            super::file_format::CURRENT_VERSION as u32,
            "database",
        ),
        (
            ".wal",
            &b"TVWL"[..],
            2,
            super::wal::WAL_VERSION as u32,
            "wal",
        ),
        (
            ".flush_ok",
            &b"TFMK"[..],
            1,
            u32::from(super::file_format::FLUSH_MARKER_VERSION),
            "checkpoint_marker",
        ),
    ] {
        let mut file = match File::open(format!("{path}{suffix}")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let mut header = [0u8; 6];
        let complete = file.read_exact(&mut header[..4 + width]);
        if recovering
            && matches!(component, "database" | "checkpoint_marker")
            && (complete.is_err() || &header[..4] != magic)
        {
            // 当前发布日志持有备份；完整恢复验证负责判定撕裂检查点。
            continue;
        }
        complete.map_err(|error| {
            TriviumError::CorruptedFile(format!(
                "格式头不完整 (Incomplete format header): {component}: {error}"
            ))
        })?;
        if &header[..4] != magic {
            if component == "wal" {
                return Err(TriviumError::UnsupportedWalVersion {
                    found: 0,
                    supported: current as u16,
                });
            }
            return Err(TriviumError::CorruptedFile(format!(
                "格式魔数无效 (Invalid format magic): {component}"
            )));
        }
        let found = if width == 1 {
            u32::from(header[4])
        } else {
            u32::from(u16::from_le_bytes([header[4], header[5]]))
        };
        if component == "database" && (found == 0 || found > current) {
            return Err(TriviumError::UnsupportedDatabaseVersion {
                found: found as u16,
                minimum_supported: current as u16,
                current: current as u16,
            });
        }
        if component == "wal" && (found == 0 || found > current) {
            return Err(TriviumError::UnsupportedWalVersion {
                found: found as u16,
                supported: current as u16,
            });
        }
        if component != "checkpoint_marker"
            || found != u32::from(super::file_format::LEGACY_FLUSH_MARKER_VERSION)
        {
            require(component, found, current)?;
        }
    }
    if !recovering && std::path::Path::new(path).is_file() {
        for (suffix, magic, offset, width, current, component) in [
            (".quiver", &b"QUIV"[..], 4, 4, 2, "quiver"),
            (".quiver.meta", &b"QMET"[..], 4, 4, 2, "quiver_metadata"),
            (".text", &b"TIDX"[..], 4, 4, 2, "text_index"),
            (".text.meta", &b"TMET"[..], 4, 4, 2, "text_metadata"),
            (".pidx", &b"TPDX"[..], 4, 2, 8, "property_index"),
            (".gidx", &b"TGIX"[..], 4, 2, 3, "graph_index"),
            (".qssd", &b"QVSSD001"[..], 8, 4, 2, "compressed_vectors"),
        ] {
            let Some(part) = super::index_container::open(path, suffix).map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    TriviumError::CorruptedFile(format!(
                        "索引容器无效 (Invalid index container): {error}"
                    ))
                } else {
                    error.into()
                }
            })?
            else {
                continue;
            };
            let mut bytes = [0u8; 12];
            let length = part.length;
            let mut reader = part.reader()?;
            if component == "text_metadata" && length == 52 {
                let mut metadata = [0; 52];
                reader.read_exact(&mut metadata)?;
                if crc32fast::hash(&metadata[..48])
                    != u32::from_le_bytes(
                        metadata[48..52].try_into().map_err(std::io::Error::other)?,
                    )
                {
                    return Err(TriviumError::CorruptedFile(
                        "文本元数据 CRC 无效 (Invalid text metadata CRC)".into(),
                    ));
                }
                if &metadata[..4] != magic {
                    continue;
                }
                require(
                    component,
                    u32::from_le_bytes(metadata[4..8].try_into().map_err(std::io::Error::other)?),
                    current,
                )?;
                continue;
            }
            // 损坏组件仍由正式加载器按事实/派生索引职责处理；识别到历史身份时禁止隐式转换。
            if reader.read_exact(&mut bytes[..offset + width]).is_err()
                || &bytes[..magic.len()] != magic
            {
                continue;
            }
            let found = if width == 2 {
                u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as u32
            } else {
                u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ])
            };
            require(component, found, current)?;
            if component == "property_index" {
                let mut key = [0; 2];
                if reader.read_exact(&mut key).is_ok() {
                    require("property_key", u16::from_le_bytes(key).into(), 2)?;
                }
            }
        }
    }
    Ok(())
}
