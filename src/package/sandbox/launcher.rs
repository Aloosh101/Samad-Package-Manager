use std::process::{Child, Output};

use crate::error::{SpmError, SpmResult};

pub struct SandboxProcess {
    pub child: Child,
}

impl SandboxProcess {
    pub fn wait(self) -> SpmResult<Output> {
        self.child.wait_with_output().map_err(|e| {
            SpmError::command_failed(format!("Failed to wait for sandboxed process: {e}"))
        })
    }

    pub fn kill(&mut self) -> SpmResult<()> {
        self.child.kill().map_err(|e| {
            SpmError::command_failed(format!("Failed to kill sandboxed process: {e}"))
        })
    }

    pub fn try_wait(&mut self) -> SpmResult<Option<std::process::ExitStatus>> {
        self.child.try_wait().map_err(|e| {
            SpmError::command_failed(format!("Failed to check sandboxed process status: {e}"))
        })
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }
}
