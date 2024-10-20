use anyhow::{anyhow, Error, Result};
use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::AddAnyPortError::OnlyPermanentLeasesSupported;
use igd_next::{GetExternalIpError, PortMappingProtocol, SearchError, SearchOptions};
use local_ip_address::local_ip;
use std::net::{IpAddr, SocketAddr};
use time::OffsetDateTime;

/// The duration in seconds of a port mapping on the gateway.
const MAPPING_DURATION: u32 = 3600;

/// Renew the Mapping every half of `MAPPING_DURATION` to avoid the port being unmapped.
const MAPPING_TIMEOUT: i64 = MAPPING_DURATION as i64 / 2;

/// A port mapping on the gateway.
pub struct PortMap {
    /// TCP or UDP.
    pub protocol: PortMappingProtocol,
    /// Gateway forwards traffics from external port to the forward address.
    pub forward_addr: SocketAddr,
    /// The mapped external port on the gateway.
    pub external_port: u16,
    /// The duration in seconds of a port mapping on the gateway.
    /// Some gateway only supports permanent leases, so this value may be zero.
    timeout: u32,
    /// Last time the port mapping is sent.
    timestamp: i64,
}

/// Interface that interacts with the inner gateway.
pub struct Upnp {
    /// Local IPv4 address of the machine in the local network.
    local_ip: IpAddr,
    /// UPnP interface.
    gateway: Gateway<Tokio>,
}

impl Upnp {
    pub async fn new() -> Result<Self> {
        // Find the best WAN interface IPv4 address.
        let ip = local_ip()?;
        let gateway = search_gateway(SearchOptions {
            bind_addr: SocketAddr::new(ip, 0),
            ..Default::default()
        })
        .await
        .map_err(|e| {
            if matches!(e, SearchError::NoResponseWithinTimeout) {
                anyhow!("no available upnp server in this network")
            } else {
                Error::from(e)
            }
        })?;
        Ok(Self {
            local_ip: ip,
            gateway,
        })
    }

    /// Request a new port mapping in gateway.
    pub async fn add_port(
        &self,
        protocol: PortMappingProtocol,
        forward_addr: SocketAddr,
    ) -> Result<PortMap> {
        let description = description();
        let mut forward_addr = forward_addr.to_owned();
        // Forward to 0.0.0.0 is same as the local ip of this machine.
        if forward_addr.ip().is_unspecified() {
            forward_addr.set_ip(self.local_ip);
        }
        let mut timeout = MAPPING_DURATION;
        let mut external_port = self
            .gateway
            .add_any_port(protocol, forward_addr, timeout, description.as_str())
            .await;
        if let Err(ref e) = external_port {
            if matches!(e, OnlyPermanentLeasesSupported) {
                // Gateway only supports permanent leases.
                // Retry with a lease duration of 0.
                external_port = self
                    .gateway
                    .add_any_port(protocol, forward_addr, 0, description.as_str())
                    .await;
                timeout = 0;
            }
        }
        Ok(PortMap {
            protocol,
            forward_addr,
            external_port: external_port?,
            timestamp: OffsetDateTime::now_utc().unix_timestamp(),
            timeout,
        })
    }

    /// Renew a port mapping before the ttl.
    pub async fn renew_port(&self, pm: &mut PortMap) -> Result<()> {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        if pm.timestamp == 0 || now - pm.timestamp < MAPPING_TIMEOUT {
            return Ok(());
        }
        self.gateway
            .add_port(
                pm.protocol,
                pm.external_port,
                pm.forward_addr,
                pm.timeout,
                description().as_str(),
            )
            .await?;
        pm.timestamp = OffsetDateTime::now_utc().unix_timestamp();
        Ok(())
    }

    /// Remove a port mapping in gateway.
    pub async fn remove_port(&self, pm: &mut PortMap) -> Result<()> {
        self.gateway
            .remove_port(pm.protocol, pm.external_port)
            .await?;
        pm.external_port = 0;
        pm.timestamp = 0;
        Ok(())
    }

    /// Returns the external ip of the gateway.
    pub async fn external_ip(&self) -> Result<IpAddr, GetExternalIpError> {
        self.gateway.get_external_ip().await
    }
}

impl PortMap {
    /// Returns the local socket binding address.
    /// The external port is used because the NAT gateway usually
    /// keep the source port unchanged.
    pub fn local_addr(&self) -> String {
        format!("0.0.0.0:{}", self.external_port)
    }
}

fn description() -> String {
    if let Ok(name) = hostname::get() {
        format!("NAT2 - {}", name.into_string().unwrap())
    } else {
        "NAT2".to_string()
    }
}
