pub mod dnspod;
pub mod http;

use crate::config;
use async_trait::async_trait;
use stun::xoraddr::XorMappedAddress;
use anyhow::Result;

#[async_trait]
pub trait Watcher {
    fn name(&self) -> &'static str;
    async fn new_address(&self, addr: &XorMappedAddress, md: &config::Metadata) -> Result<()>;
    fn validate(&self, md: &config::Metadata) -> Result<()>;
}

pub fn format_value(value: &String, addr: &XorMappedAddress) -> String {
    value.replace("{ip}", addr.ip.to_string().as_str()).replace("{port}", addr.port.to_string().as_str())
}
