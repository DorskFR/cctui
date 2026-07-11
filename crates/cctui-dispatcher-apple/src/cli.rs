//! Thin wrapper around the Apple `container` CLI so spawn mechanics are
//! mockable off-macOS (the binary does not exist on Linux/CI).
//!
//! The real impl shells out; tests substitute a mock that records the argv and
//! returns canned output.

use std::ffi::OsStr;

#[derive(Debug, Clone)]
pub struct CliOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    #[must_use]
    pub const fn ok(&self) -> bool {
        matches!(self.code, Some(0))
    }
}

pub trait ContainerCli: Send + Sync {
    /// Run `container <args>` and capture its output. A non-zero exit is not an
    /// error at this layer — the caller inspects [`CliOutput::code`]/stderr to
    /// distinguish "already exists" from a real failure.
    fn exec(
        &self,
        args: Vec<String>,
    ) -> impl std::future::Future<Output = anyhow::Result<CliOutput>> + Send;
}

/// Shells out to the configured `container` binary (default `container`).
pub struct RealCli {
    bin: String,
}

impl RealCli {
    #[must_use]
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }
}

impl ContainerCli for RealCli {
    async fn exec(&self, args: Vec<String>) -> anyhow::Result<CliOutput> {
        let out = tokio::process::Command::new(&self.bin)
            .args(args.iter().map(OsStr::new))
            .output()
            .await
            .map_err(|e| anyhow::anyhow!("spawning `{}`: {e}", self.bin))?;
        Ok(CliOutput {
            code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
