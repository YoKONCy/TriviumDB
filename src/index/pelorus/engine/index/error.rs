//! 公开错误与结果类型。

use super::*;
#[derive(Debug, Clone)]
pub enum Error {
    InvalidDimensions,
    VectorLength {
        expected: usize,
        actual: usize,
    },
    IdCount {
        expected: usize,
        actual: usize,
    },
    InvalidBuildConfig(&'static str),
    InvalidSearchConfig(&'static str),
    InvalidMetric(&'static str),
    InvalidRerankConfig(&'static str),
    OutsideManifold {
        index: usize,
    },
    DistanceOverflow,
    DuplicateId(u64),
    CapacityExceeded,
    NonFiniteVector {
        index: usize,
    },
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },
    Host(std::sync::Arc<crate::TriviumError>),
    CorruptIndex(&'static str),
    UnsupportedVersion(u32),
}
impl Display for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDimensions => write!(formatter, "向量维度必须大于零"),
            Self::VectorLength { expected, actual } => {
                write!(
                    formatter,
                    "向量数据长度错误：期望 {expected}，实际 {actual}"
                )
            }
            Self::IdCount { expected, actual } => {
                write!(formatter, "ID 数量错误：期望 {expected}，实际 {actual}")
            }
            Self::InvalidBuildConfig(reason) => write!(formatter, "构建配置无效：{reason}"),
            Self::InvalidSearchConfig(reason) => write!(formatter, "搜索配置无效：{reason}"),
            Self::InvalidMetric(reason) => write!(formatter, "几何配置无效：{reason}"),
            Self::InvalidRerankConfig(reason) => write!(formatter, "重排配置无效：{reason}"),
            Self::OutsideManifold { index } => write!(formatter, "第 {index} 条向量不在开双曲球内"),
            Self::DistanceOverflow => write!(formatter, "双曲距离超出结果分数的有限数范围"),
            Self::DuplicateId(id) => write!(formatter, "ID 重复：{id}"),
            Self::CapacityExceeded => write!(formatter, "索引容量超出平台或文件格式限制"),
            Self::NonFiniteVector { index } => {
                write!(formatter, "向量数据在位置 {index} 不是有限数")
            }
            Self::Io { message, .. } => write!(formatter, "输入输出错误：{message}"),
            Self::Host(error) => Display::fmt(error, formatter),
            Self::CorruptIndex(reason) => write!(formatter, "索引文件损坏：{reason}"),
            Self::UnsupportedVersion(version) => write!(formatter, "不支持的索引版本：{version}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;
