use crate::config::Metadata;
use crate::watcher::{format_value, Watcher};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use stun::xoraddr::XorMappedAddress;
use tokio::process::Command;
use tracing::debug;

/// Run a script or program.
pub struct Script {
    /// Instance name.
    name: String,
    /// Path to executable file.
    path: String,
    /// Arguments to pass to the program.
    /// If the `value` field in watcher metadata is not empty, it
    /// will be passed to the program as the last argument.
    args: Vec<String>,
}

impl Script {
    pub fn new(name: String, path: String, args: Vec<String>) -> Self {
        Self { name, path, args }
    }
}

#[async_trait]
impl Watcher for Script {
    fn kind(&self) -> &'static str {
        "script"
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    async fn new_address(&self, addr: &XorMappedAddress, md: &Metadata) -> Result<()> {
        let mut command = Command::new(&self.path);
        command.args(&self.args);
        if !md.value.is_empty() {
            command.arg(format_value(&md.value, addr));
        }
        debug!(name = self.name(), "starting new {:?}", command);
        let output = command.output().await?;
        debug!(
            name = self.name(),
            "process finished with {}", output.status
        );
        if output.status.success() {
            Ok(())
        } else if !output.stderr.is_empty() {
            Err(anyhow!("{}", String::from_utf8_lossy(&output.stderr)))
        } else {
            Err(anyhow!("process finished with {}", output.status))
        }
    }

    fn validate(&self, _: &Metadata) -> Result<()> {
        Ok(())
    }
}
