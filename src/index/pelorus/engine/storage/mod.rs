//! 不可变映射基础层、内存增量层及冷向量编码。

mod config;
pub use config::*;
mod codec;
mod scoring;
pub(crate) use codec::{ColdStore, RerankScratch};
pub(crate) mod binary;

use crate::index::pelorus::engine::{Error, Result};
use bytemuck::Pod;
use memmap2::Mmap;
use std::ops::Range;
use std::sync::Arc;

enum Base<T> {
    Owned(Vec<T>),
    Mapped {
        mapping: Arc<Mmap>,
        range: Range<usize>,
        source: Option<(Arc<std::fs::File>, usize)>,
        _temporary: Option<Arc<tempfile::TempPath>>,
    },
}

/// 基础层长度在追加期间保持不变，一条记录始终完整属于基础层或增量层。
pub(crate) struct Buffer<T: Pod> {
    base: Base<T>,
    delta: Vec<T>,
}

impl<T: Pod> Buffer<T> {
    pub(crate) fn owned(values: Vec<T>) -> Self {
        Self {
            base: Base::Owned(values),
            delta: Vec::new(),
        }
    }

    pub(crate) fn mapped(mapping: Arc<Mmap>, range: Range<usize>) -> Result<Self> {
        let bytes = mapping
            .get(range.clone())
            .ok_or(Error::CorruptIndex("映射区间越界"))?;
        bytemuck::try_cast_slice::<u8, T>(bytes)
            .map_err(|_| Error::CorruptIndex("映射数据长度或对齐错误"))?;
        Ok(Self {
            base: Base::Mapped {
                mapping,
                range,
                _temporary: None,
                source: None,
            },
            delta: Vec::new(),
        })
    }

    pub(crate) fn map_file(file: &std::fs::File, offset: usize, length: usize) -> Result<Self> {
        if length == 0 {
            return Ok(Self::owned(Vec::new()));
        }
        // SAFETY：区间长度已经由快照解析器验证，文件属于不可变代际，映射仅覆盖当前向量区间。
        let mapping = unsafe {
            memmap2::MmapOptions::new()
                .offset(offset as u64)
                .len(length)
                .map(file)?
        };
        let mut result = Self::mapped(Arc::new(mapping), 0..length)?;
        if let Base::Mapped { source, .. } = &mut result.base {
            *source = Some((Arc::new(file.try_clone()?), offset));
        }
        Ok(result)
    }

    pub(crate) fn slices(&self) -> (&[T], &[T]) {
        let base = match &self.base {
            Base::Owned(values) => values.as_slice(),
            Base::Mapped { mapping, range, .. } => bytemuck::cast_slice(&mapping[range.clone()]),
        };
        (base, &self.delta)
    }

    pub(crate) fn len(&self) -> usize {
        let (base, delta) = self.slices();
        base.len() + delta.len()
    }

    pub(crate) fn row(&self, slot: usize, stride: usize) -> &[T] {
        let (base, delta) = self.slices();
        let offset = slot * stride;
        if offset < base.len() {
            &base[offset..offset + stride]
        } else {
            &delta[offset - base.len()..offset - base.len() + stride]
        }
    }

    pub(crate) fn reserve(&mut self, count: usize) -> Result<()> {
        self.delta
            .try_reserve(count)
            .map_err(|_| Error::CapacityExceeded)
    }

    pub(crate) fn append(&mut self, values: &[T]) {
        self.delta.extend_from_slice(values);
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        let base = match &self.base {
            Base::Owned(values) => values.len(),
            Base::Mapped { .. } => 0,
        };
        (base + self.delta.len()) * std::mem::size_of::<T>()
    }

    pub(crate) fn mapped_bytes(&self) -> usize {
        match &self.base {
            Base::Mapped { range, .. } => range.len(),
            Base::Owned(_) => 0,
        }
    }

    pub(crate) fn is_mapped(&self) -> bool {
        matches!(self.base, Base::Mapped { .. })
    }

    pub(crate) fn spill(&self) -> Result<Self> {
        use std::io::Write;
        if self.len() == 0 {
            return Ok(Self::owned(Vec::new()));
        }
        let mut file = tempfile::NamedTempFile::new()?;
        let (base, delta) = self.slices();
        file.write_all(bytemuck::cast_slice(base))?;
        file.write_all(bytemuck::cast_slice(delta))?;
        file.as_file().sync_all()?;
        // SAFETY：临时文件由此仓库独占，映射建立后仅只读访问，文件路径活到最后一个映射释放。
        let mapping = Arc::new(unsafe { Mmap::map(file.as_file())? });
        let length = mapping.len();
        let source = Some((Arc::new(file.as_file().try_clone()?), 0));
        let owner = Arc::new(file.into_temp_path());
        Ok(Self {
            base: Base::Mapped {
                mapping,
                range: 0..length,
                _temporary: Some(owner),
                source,
            },
            delta: Vec::new(),
        })
    }

    pub(crate) fn to_owned(&self) -> Self {
        let (base, delta) = self.slices();
        let mut values = Vec::with_capacity(base.len() + delta.len());
        values.extend_from_slice(base);
        values.extend_from_slice(delta);
        Self::owned(values)
    }

    /// 已有只读基础层重建独立映射，释放准备阶段触及的驻留页；内存或增量层先落盘。
    pub(crate) fn file_backed(&self) -> Result<Self> {
        if self.delta.is_empty()
            && let Base::Mapped {
                source: Some((file, offset)),
                range,
                _temporary,
                ..
            } = &self.base
        {
            let mut result = Self::map_file(file, offset + range.start, range.len())?;
            if let Base::Mapped {
                _temporary: owner, ..
            } = &mut result.base
            {
                *owner = _temporary.clone();
            }
            return Ok(result);
        }
        self.spill()
    }

    pub(crate) fn file_source(&self) -> Option<(Arc<std::fs::File>, usize, usize)> {
        match &self.base {
            Base::Mapped {
                source: Some((file, offset)),
                range,
                ..
            } => Some((Arc::clone(file), offset + range.start, range.len())),
            _ => None,
        }
    }
}
