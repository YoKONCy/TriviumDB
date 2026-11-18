//! 非交互子命令的共享实现层。
//!
//! 这些函数同时被 CLI 子命令、REPL 元命令复用。

pub mod compact;
pub mod exec;
pub mod export;
pub mod import;
pub mod info;
pub mod repair;

use triviumdb::error::{ErrorInfo, TriviumError, WriteErrorInfo, WriteOutcome, WritePhase};

#[derive(Debug)]
pub(crate) struct CheckpointAfterCommit(TriviumError);

impl std::fmt::Display for CheckpointAfterCommit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "事务已提交，后续检查点失败 (Transaction committed and subsequent checkpoint failed): {}",
            self.0
        )
    }
}

impl std::error::Error for CheckpointAfterCommit {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl CheckpointAfterCommit {
    pub(crate) fn info(&self) -> ErrorInfo {
        let mut info = self.0.info();
        if let Some(checkpoint_write) = info.write {
            info.details["checkpoint_write"] = serde_json::json!(checkpoint_write);
        }
        info.write = Some(WriteErrorInfo {
            phase: WritePhase::Publish,
            outcome: WriteOutcome::Committed,
            transaction_consumed: None,
        });
        info.message = self.to_string();
        info
    }
}

/// 业务事务和后续检查点属于两个完成点。
fn checkpoint_after_commit(
    flush: impl FnOnce() -> triviumdb::Result<()>,
) -> Result<(), CheckpointAfterCommit> {
    flush().map_err(CheckpointAfterCommit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 检查点失败分别报告事务和检查点的结果() {
        let error = checkpoint_after_commit(|| {
            Err(TriviumError::WriteFailure {
                source: Box::new(TriviumError::CapacityAllocationFailed {
                    reason: "测试工作区准入 (Test workspace admission)".into(),
                }),
                write: WriteErrorInfo {
                    phase: WritePhase::Admission,
                    outcome: WriteOutcome::NotCommitted,
                    transaction_consumed: None,
                },
            })
        })
        .unwrap_err();
        let info = serde_json::to_value(error.info()).unwrap();
        assert_eq!(
            info["write"],
            serde_json::json!({"phase":"publish","outcome":"committed"})
        );
        assert_eq!(
            info["details"]["checkpoint_write"],
            serde_json::json!({"phase":"admission","outcome":"not_committed"})
        );
        assert_eq!(info["code"], "TDB_CAPACITY_ALLOCATION_FAILED");
    }
}
