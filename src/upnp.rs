use anyhow::Result;
use igd_next::aio::tokio::{search_gateway, Tokio};
use igd_next::aio::Gateway;
use igd_next::AddAnyPortError::OnlyPermanentLeasesSupported;
use igd_next::{GetExternalIpError, PortMappingProtocol, SearchOptions};
use local_ip_address::local_ip;
use std::net::{IpAddr, SocketAddr};
use time::OffsetDateTime;

const MAPPING_DURATION: u32 = 3600;

const MAPPING_TIMEOUT: i64 = MAPPING_DURATION as i64 / 2;

pub struct PortMap {
    pub protocol: PortMappingProtocol,
    pub forward_addr: SocketAddr,
    pub external_port: u16,
    timestamp: i64,
}

pub struct Upnp {
    local_ip: IpAddr,
    gateway: Gateway<Tokio>,
    timeout: u32,
}

impl Upnp {
    pub async fn new() -> Result<Self> {
        let ip = local_ip()?;
        let gateway = search_gateway(SearchOptions{
            bind_addr: SocketAddr::new(ip, 0),
            ..Default::default()
        }).await?;
        Ok(Self{
            local_ip: ip,
            gateway,
            timeout: MAPPING_DURATION,
        })
    }

    pub async fn add_port(&mut self, protocol: PortMappingProtocol, forward_addr: SocketAddr) -> Result<PortMap> {
        let description = description();
        let mut forward_addr = forward_addr.to_owned();
        if forward_addr.ip().is_unspecified() {
            forward_addr.set_ip(self.local_ip);
        }
        let mut external_port = self.gateway.add_any_port(protocol, forward_addr, self.timeout, description.as_str()).await;
        if let Err(ref e) = external_port {
            if matches!(e, OnlyPermanentLeasesSupported) {
                external_port = self.gateway.add_any_port(protocol, forward_addr, 0, description.as_str()).await;
                self.timeout = 0;
            }
        }
        Ok(PortMap{
            protocol,
            forward_addr,
            external_port: external_port?,
            timestamp: OffsetDateTime::now_utc().unix_timestamp(),
        })
    }

    pub async fn renew_port(&self, pm: &mut PortMap) -> Result<()> {
        if pm.timestamp == 0 || OffsetDateTime::now_utc().unix_timestamp() - pm.timestamp < MAPPING_TIMEOUT {
            return Ok(());
        }
        self.gateway.add_port(pm.protocol, pm.external_port, pm.forward_addr, self.timeout, description().as_str()).await?;
        pm.timestamp = OffsetDateTime::now_utc().unix_timestamp();
        Ok(())
    }

    pub async fn remove_port(&self, pm: &mut PortMap) -> Result<()> {
        self.gateway.remove_port(pm.protocol, pm.external_port).await?;
        pm.external_port = 0;
        pm.timestamp = 0;
        Ok(())
    }

    pub async fn external_ip(&self) -> Result<IpAddr, GetExternalIpError> {
        self.gateway.get_external_ip().await
    }
}

fn description() -> String {
    if let Ok(name) = hostname::get() {
        format!("NAT2 - {}", name.into_string().unwrap())
    } else {
        "NAT2".to_string()
    }
}
