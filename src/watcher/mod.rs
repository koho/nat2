pub mod alidns;
pub mod dnspod;
pub mod http;
pub mod script;

use crate::config;
use anyhow::Result;
use async_trait::async_trait;
use stun::xoraddr::XorMappedAddress;

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
    value
        .replace("{ip}", addr.ip.to_string().as_str())
        .replace("{port}", addr.port.to_string().as_str())
}

pub mod dns {
    use anyhow::anyhow;
    use url::ParseError::InvalidDomainCharacter;

    /// Validate basic DNS metadata.
    pub fn validate(md: &super::config::Metadata) -> super::Result<()> {
        let domain = md
            .domain
            .as_ref()
            .ok_or(anyhow!("missing field `domain`"))?;
        let record_type = md
            .kind
            .as_ref()
            .ok_or(anyhow!("missing field `type`"))?
            .to_lowercase();
        if (record_type == "svcb" || record_type == "https") && md.priority.is_none() {
            return Err(anyhow!("missing field `priority`"));
        }
        split_domain_name(domain).ok_or(InvalidDomainCharacter)?;
        Ok(())
    }

    /// Split domain name into host record and SLD.
    pub fn split_domain_name(domain: &String) -> Option<(String, String)> {
        let mut labels: Vec<_> = domain.split(".").collect();
        if let Some(&"") = labels.last() {
            labels.remove(labels.len() - 1);
        }
        let len = labels.len();
        if len < 2 || labels.last()?.is_empty() || labels.get(len - 2)?.is_empty() {
            return None;
        }
        let domain = &labels[len - 2..];
        let subdomain = &labels[..len - 2];
        Some((domain.join("."), subdomain.join(".")))
    }
}
