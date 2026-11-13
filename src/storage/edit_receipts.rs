//! 有界编辑回执编码，与 WAL 事务及检查点共同保存。

use crate::{Result, TriviumError};

pub(crate) const MAX_RECEIPTS: usize = 64;
pub(crate) const MAX_HEAP_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_BLOCK_BYTES: usize = MAX_HEAP_BYTES + 64 * 1024;

pub(crate) struct Receipt {
    pub key: String,
    pub expected: String,
    pub fingerprint: Vec<u8>,
    pub ids: Vec<u64>,
    pub revision: String,
}

fn corrupt() -> TriviumError {
    TriviumError::CorruptedFile(
        "编辑回执格式或范围无效 (Invalid edit receipt format or bounds)".into(),
    )
}

fn take<'a>(input: &mut &'a [u8], count: usize) -> Result<&'a [u8]> {
    let bytes = input.get(..count).ok_or_else(corrupt)?;
    *input = &input[count..];
    Ok(bytes)
}

fn number(input: &mut &[u8], width: usize) -> Result<usize> {
    let mut bytes = [0; 8];
    bytes[..width].copy_from_slice(take(input, width)?);
    usize::try_from(u64::from_le_bytes(bytes)).map_err(|_| corrupt())
}

pub(crate) fn encode(
    key: &str,
    expected: &str,
    fingerprint: &[u8],
    ids: &[u64],
    revision: &str,
) -> Result<Vec<u8>> {
    if key.is_empty()
        || key.len() > 128
        || expected.len() != 49
        || revision.len() != 49
        || ids.len() > 1000
    {
        return Err(corrupt());
    }
    // 长度字段共 8 字节，两段版本令牌各 49 字节，节点 ID 每项 8 字节。
    let size = key
        .len()
        .checked_add(fingerprint.len())
        .and_then(|size| size.checked_add(ids.len() * 8 + 106))
        .ok_or_else(corrupt)?;
    if size > MAX_BLOCK_BYTES {
        return Err(corrupt());
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|error| TriviumError::CapacityAllocationFailed {
            reason: format!("回执编码分配失败 (Receipt encoding allocation failed): {error}"),
        })?;
    bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(fingerprint.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(ids.len() as u16).to_le_bytes());
    bytes.extend_from_slice(expected.as_bytes());
    bytes.extend_from_slice(revision.as_bytes());
    bytes.extend_from_slice(key.as_bytes());
    bytes.extend_from_slice(fingerprint);
    for id in ids {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    Ok(bytes)
}

pub(crate) fn decode(mut input: &[u8]) -> Result<Receipt> {
    if input.len() > MAX_BLOCK_BYTES {
        return Err(corrupt());
    }
    let key_len = number(&mut input, 2)?;
    let fingerprint_len = number(&mut input, 4)?;
    let ids_len = number(&mut input, 2)?;
    if key_len == 0 || key_len > 128 || ids_len > 1000 {
        return Err(corrupt());
    }
    let expected = std::str::from_utf8(take(&mut input, 49)?)
        .map_err(|_| corrupt())?
        .to_owned();
    let revision = std::str::from_utf8(take(&mut input, 49)?)
        .map_err(|_| corrupt())?
        .to_owned();
    let key = std::str::from_utf8(take(&mut input, key_len)?)
        .map_err(|_| corrupt())?
        .to_owned();
    let raw_fingerprint = take(&mut input, fingerprint_len)?;
    std::str::from_utf8(raw_fingerprint).map_err(|_| corrupt())?;
    let mut fingerprint = Vec::new();
    fingerprint
        .try_reserve_exact(fingerprint_len)
        .map_err(|error| TriviumError::CapacityAllocationFailed {
            reason: format!("回执指纹分配失败 (Receipt fingerprint allocation failed): {error}"),
        })?;
    fingerprint.extend_from_slice(raw_fingerprint);
    if input.len() != ids_len * 8 {
        return Err(corrupt());
    }
    let mut ids = Vec::new();
    ids.try_reserve_exact(ids_len)
        .map_err(|error| TriviumError::CapacityAllocationFailed {
            reason: format!("回执 ID 分配失败 (Receipt ID allocation failed): {error}"),
        })?;
    for chunk in input.as_chunks::<8>().0 {
        ids.push(u64::from_le_bytes(*chunk));
    }
    Ok(Receipt {
        key,
        expected,
        fingerprint,
        ids,
        revision,
    })
}

/// 尾部长度与 CRC 独立校验，唯一约束块继续保留自己的校验和。
pub(crate) fn split_checkpoint(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let footer = bytes.len().checked_sub(16).ok_or_else(corrupt)?;
    if &bytes[footer + 12..] != b"TREC" {
        return Err(corrupt());
    }
    let mut tail = &bytes[footer..footer + 8];
    let len = number(&mut tail, 8)?;
    if len > MAX_BLOCK_BYTES {
        return Err(corrupt());
    }
    let start = footer.checked_sub(len).ok_or_else(corrupt)?;
    let data = &bytes[start..footer];
    let mut crc = [0; 4];
    crc.copy_from_slice(&bytes[footer + 8..footer + 12]);
    if crc32fast::hash(data) != u32::from_le_bytes(crc) {
        return Err(corrupt());
    }
    Ok((&bytes[..start], data))
}

pub(crate) fn records(mut bytes: &[u8]) -> Result<Vec<&[u8]>> {
    let count = number(&mut bytes, 2)?;
    if count > MAX_RECEIPTS {
        return Err(corrupt());
    }
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        let len = number(&mut bytes, 4)?;
        records.push(take(&mut bytes, len)?);
    }
    if !bytes.is_empty() {
        return Err(corrupt());
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 固定回执样例保留完整整数并拒绝每个截断前缀() {
        let expected = "00000000000000000000000000000000:0000000000000000";
        let revision = "11111111111111111111111111111111:0000000000000001";
        let mut fixture = vec![1, 0, 2, 0, 0, 0, 2, 0];
        fixture.extend_from_slice(expected.as_bytes());
        fixture.extend_from_slice(revision.as_bytes());
        fixture.extend_from_slice(b"k[]");
        fixture.extend_from_slice(&[7, 0, 0, 0, 0, 0, 0, 0]);
        fixture.extend_from_slice(&[255; 8]);
        assert_eq!(
            encode("k", expected, b"[]", &[7, u64::MAX], revision).unwrap(),
            fixture
        );
        let decoded = decode(&fixture).unwrap();
        assert_eq!(decoded.key, "k");
        assert_eq!(decoded.expected, expected);
        assert_eq!(decoded.fingerprint, b"[]");
        assert_eq!(decoded.ids, vec![7, u64::MAX]);
        assert_eq!(decoded.revision, revision);
        for end in 0..fixture.len() {
            assert!(decode(&fixture[..end]).is_err(), "截断位置 {end}");
        }
        fixture[6..8].copy_from_slice(&1001u16.to_le_bytes());
        assert!(decode(&fixture).is_err());
    }

    #[test]
    fn 回执尾部拒绝无界长度错误校验和及非法计数() {
        let data = [0u8, 0];
        let mut checkpoint = b"prefix".to_vec();
        checkpoint.extend_from_slice(&data);
        checkpoint.extend_from_slice(&2u64.to_le_bytes());
        checkpoint.extend_from_slice(&crc32fast::hash(&data).to_le_bytes());
        checkpoint.extend_from_slice(b"TREC");
        let (prefix, payload) = split_checkpoint(&checkpoint).unwrap();
        assert_eq!(prefix, b"prefix");
        assert!(records(payload).unwrap().is_empty());
        checkpoint[6] ^= 1;
        assert!(split_checkpoint(&checkpoint).is_err());
        assert!(records(&65u16.to_le_bytes()).is_err());
        let mut malformed = b"prefix".to_vec();
        malformed.extend_from_slice(&u64::MAX.to_le_bytes());
        malformed.extend_from_slice(&[0; 4]);
        malformed.extend_from_slice(b"TREC");
        assert!(split_checkpoint(&malformed).is_err());
    }
}
