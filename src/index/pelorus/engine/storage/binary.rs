//! 快照的有界小端读写，所有变长分配先按剩余输入长度校验。

use crate::index::pelorus::engine::{Error, Result};

pub(crate) trait Value: Sized {
    const SIZE: usize;
    fn append(self, output: &mut Vec<u8>);
    fn decode(bytes: &[u8]) -> Self;
}

macro_rules! value {
    ($type:ty, $size:literal) => {
        impl Value for $type {
            const SIZE: usize = $size;
            fn append(self, output: &mut Vec<u8>) {
                output.extend_from_slice(&self.to_le_bytes());
            }
            fn decode(bytes: &[u8]) -> Self {
                Self::from_le_bytes(bytes.try_into().unwrap())
            }
        }
    };
}
value!(u16, 2);
value!(u32, 4);
value!(u64, 8);
value!(f32, 4);
value!(f64, 8);

pub(crate) fn put<T: Value>(output: &mut Vec<u8>, value: T) {
    value.append(output);
}
pub(crate) fn put_usize(output: &mut Vec<u8>, value: usize) {
    put(output, value as u64);
}
pub(crate) fn put_vec<T: Value + Copy>(output: &mut Vec<u8>, values: &[T]) {
    put_usize(output, values.len());
    for &value in values {
        put(output, value);
    }
}

pub(crate) struct Reader<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) position: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    pub(crate) fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(Error::CapacityExceeded)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(Error::CorruptIndex("快照字段被截断"))?;
        self.position = end;
        Ok(bytes)
    }
    pub(crate) fn get<T: Value>(&mut self) -> Result<T> {
        Ok(T::decode(self.take(T::SIZE)?))
    }
    pub(crate) fn usize(&mut self) -> Result<usize> {
        usize::try_from(self.get::<u64>()?).map_err(|_| Error::CapacityExceeded)
    }
    pub(crate) fn boolean(&mut self) -> Result<bool> {
        match self.get::<u32>()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::CorruptIndex("布尔字段无效")),
        }
    }
    pub(crate) fn vector<T: Value>(&mut self) -> Result<Vec<T>> {
        let count = self.usize()?;
        let size = count.checked_mul(T::SIZE).ok_or(Error::CapacityExceeded)?;
        let bytes = self.take(size)?;
        Ok(bytes.chunks(T::SIZE).map(T::decode).collect())
    }
    pub(crate) fn finish(&self) -> Result<()> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::CorruptIndex("快照含多余数据"))
        }
    }
}
