//! 按公开字节布局读取或变异测试组件，不调用生产解码器。
//! 这些辅助函数仅构造输入；事实、查询、恢复结果仍由各用例的独立预期验证。
#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};

const SUFFIXES: [&str; 7] = [
    ".quiver",
    ".quiver.meta",
    ".text",
    ".text.meta",
    ".pidx",
    ".gidx",
    ".qssd",
];

fn location(path: &Path) -> Option<(PathBuf, usize)> {
    let text = path.to_str()?;
    SUFFIXES.iter().enumerate().find_map(|(index, suffix)| {
        let base = text.strip_suffix(suffix)?;
        let container = PathBuf::from(format!("{base}.idx"));
        container.is_file().then_some((container, index))
    })
}

fn ranges(bytes: &[u8]) -> [std::ops::Range<usize>; 7] {
    assert!(bytes.len() >= 512);
    assert_eq!(&bytes[..8], b"TDBINDEX");
    std::array::from_fn(|index| {
        let at = 32 + index * 32;
        let offset = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap()) as usize;
        offset..offset + len
    })
}

pub fn read(path: impl AsRef<Path>) -> io::Result<Vec<u8>> {
    let path = path.as_ref();
    if let Some((container, index)) = location(path) {
        let bytes = std::fs::read(container)?;
        let range = ranges(&bytes)[index].clone();
        if bytes[32 + index * 32..36 + index * 32] == [0; 4] {
            return Err(io::ErrorKind::NotFound.into());
        }
        return Ok(bytes[range].to_vec());
    }
    std::fs::read(path)
}

pub fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    if location(path).is_some() {
        read(path).is_ok()
    } else {
        path.exists()
    }
}

pub fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some((container, index)) = location(path) {
        replace(&container, index, Some(contents.as_ref()))
    } else {
        std::fs::write(path, contents)
    }
}

pub fn remove_file(path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    if let Some((container, index)) = location(path) {
        if !exists(path) {
            return Err(io::ErrorKind::NotFound.into());
        }
        replace(&container, index, None)
    } else {
        std::fs::remove_file(path)
    }
}

pub fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<u64> {
    let bytes = read(from)?;
    write(to, &bytes)?;
    Ok(bytes.len() as u64)
}

fn replace(path: &Path, index: usize, replacement: Option<&[u8]>) -> io::Result<()> {
    let original = std::fs::read(path)?;
    let ranges = ranges(&original);
    let mut result = original[..512].to_vec();
    result[32..256].fill(0);
    for (slot, range) in ranges.into_iter().enumerate() {
        let data = if slot == index {
            replacement
        } else if range.is_empty() {
            None
        } else {
            Some(&original[range])
        };
        let Some(data) = data else { continue };
        let offset = result.len().div_ceil(65536) * 65536;
        result.resize(offset, 0);
        result.extend_from_slice(data);
        let at = 32 + slot * 32;
        result[at..at + 4].copy_from_slice(&((slot + 1) as u32).to_le_bytes());
        result[at + 8..at + 16].copy_from_slice(&(offset as u64).to_le_bytes());
        result[at + 16..at + 24].copy_from_slice(&(data.len() as u64).to_le_bytes());
        result[at + 24..at + 28].copy_from_slice(&crc32fast::hash(data).to_le_bytes());
    }
    let crc = crc32fast::hash(&result[..508]);
    result[508..512].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(path, result)
}
