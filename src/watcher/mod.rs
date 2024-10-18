pub mod dnspod;
pub mod http;

use crate::config;
use async_trait::async_trait;
use stun::xoraddr::XorMappedAddress;
use anyhow::Result;

/// A `Watcher` watches the update of mapped address.
/// The watcher get notified when the mapped address is updated,
/// and then it can perform specific task.
#[async_trait]
pub trait Watcher {
    /// Type name of the watcher.
    fn kind(&self) -> &'static str;
    /// Instance name.
    fn name(&self) -> &str;
    /// Mapped address is updated with new value.
    async fn new_address(&self, addr: &XorMappedAddress, md: &config::Metadata) -> Result<()>;
    /// Verify whether the metadata configuration is legal.
    fn validate(&self, md: &config::Metadata) -> Result<()>;
}

/// Replace `{ip}` and `{port}` with the actual value.
pub fn format_value(value: &String, addr: &XorMappedAddress) -> String {
    value.replace("{ip}", addr.ip.to_string().as_str()).replace("{port}", addr.port.to_string().as_str())
}
