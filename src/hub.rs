use crate::client;
use crate::client::Client;
use crate::config::{Config, Metadata, Tcp, Udp};
use crate::upnp::{PortMap, Upnp};
use crate::watcher::dnspod::DnsPod;
use crate::watcher::http::Http;
use crate::watcher::Watcher;
use anyhow::{anyhow, Result};
use futures::{future::select_all, FutureExt};
use igd_next::PortMappingProtocol::{TCP, UDP};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use stun::xoraddr::XorMappedAddress;
use stun::Error::ErrSchemeType;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::Receiver;
use tracing::{error, info};
use url::ParseError::{EmptyHost, InvalidPort};
use url::Url;

/// Hub manages all the port mappings and watchers.
pub struct Hub {
    mappers: Vec<Mapper>,
    watchers: HashMap<String, Box<dyn Watcher>>,
    upnp: Option<Upnp>,
}

struct Mapper {
    /// Protocol and local socket binding address.
    bind: Url,
    /// List of watcher metadata.
    metadata: Vec<Metadata>,
    /// NAT client.
    handle: Client,
    /// Public IPv4 address and port.
    public: Option<String>,
    /// UPnP mapping.
    upnp: Option<PortMap>,
    /// Mapped address receiver.
    rx: Receiver<XorMappedAddress>,
}

impl Mapper {
    async fn new_tcp(
        name: String,
        local_addr: String,
        metadata: Vec<Metadata>,
        option: &Option<Tcp>,
        upnp: Option<PortMap>,
    ) -> Result<Mapper> {
        let (tx, rx) = channel(1);
        let mut c = client::tcp::Builder::new(name, local_addr.clone(), tx);
        if let Some(opt) = option {
            if let Some(addr) = &opt.stun {
                c = c.stun_addr(addr);
            }
            if let Some(url) = &opt.keepalive {
                c = c.keepalive_url(url);
            }
            if let Some(sec) = opt.interval {
                c = c.interval(sec);
            }
        }
        Ok(Mapper {
            bind: Url::parse(format!("tcp://{local_addr}").as_str())?,
            metadata,
            handle: c.build().await?,
            public: None,
            upnp,
            rx,
        })
    }

    async fn new_udp(
        name: String,
        local_addr: String,
        metadata: Vec<Metadata>,
        option: &Option<Udp>,
        upnp: Option<PortMap>,
    ) -> Result<Mapper> {
        let (tx, rx) = channel(1);
        let mut c = client::udp::Builder::new(name, local_addr.clone(), tx);
        if let Some(opt) = option {
            if let Some(addr) = &opt.stun {
                c = c.stun_addr(addr);
            }
            if let Some(sec) = opt.interval {
                c = c.interval(sec);
            }
        }
        Ok(Mapper {
            bind: Url::parse(format!("udp://{local_addr}").as_str())?,
            metadata,
            handle: c.build().await?,
            public: None,
            upnp,
            rx,
        })
    }

    /// Return true if the new mapped address is different from the old one.
    fn changed(&mut self, addr: &XorMappedAddress) -> bool {
        let mut changed = true;
        if let Some(old_addr) = &self.public {
            if old_addr.deref() == addr.to_string() {
                changed = false;
            }
        }
        self.public = Some(addr.to_string());
        changed
    }

    /// Returns the original mapping name in config.
    fn name(&self) -> &str {
        self.handle.name()
    }

    /// Returns the forwarded address or the socket binding address.
    fn local_addr(&self) -> SocketAddr {
        if let Some(upnp) = &self.upnp {
            upnp.forward_addr
        } else {
            self.handle.local_addr()
        }
    }
}

impl Hub {
    pub async fn new(cfg: Config) -> Result<Self> {
        // Watcher list.
        let mut watchers: HashMap<String, Box<dyn Watcher>> = HashMap::new();
        for (key, value) in cfg.dnspod.into_iter() {
            watchers.insert(
                key.clone(),
                Box::new(DnsPod::new(key, value.secret_id, value.secret_key)),
            );
        }
        for (key, value) in cfg.http.into_iter() {
            watchers.insert(
                key.clone(),
                Box::new(Http::new(
                    key,
                    value.url,
                    value.method.as_str(),
                    value.body,
                    value.headers,
                )?),
            );
        }
        // UPnP feature.
        let global_upnp = !matches!(cfg.upnp, Some(false));
        let mut upnp: Option<Upnp> = None;
        if global_upnp {
            upnp = Some(Upnp::new().await?);
        }
        // Mapper list.
        let mut mappers = vec![];
        for (key, value) in cfg.map.into_iter() {
            let url = Url::parse(key.as_str())?;
            let ip = url.host().ok_or(anyhow!("{EmptyHost} in {key}"))?;
            let port = url.port().ok_or(anyhow!("{InvalidPort} in {key}"))?;
            let mut local_addr = format!("{ip}:{port}");
            // Validate watcher metadata.
            for (i, md) in value.iter().enumerate() {
                if let Some(watcher) = watchers.get(&md.name) {
                    watcher
                        .validate(md)
                        .map_err(|e| anyhow!("{e} in {key} at index {i}"))?;
                } else {
                    return Err(anyhow!(
                        "no watcher named `{}` in {key} at index {i}",
                        md.name
                    ));
                }
            }
            let mut pm: Option<PortMap> = None;
            let mapper = match url.scheme() {
                "tcp" | "tcp+upnp" | "upnp+tcp" => {
                    if url.scheme() != "tcp" || global_upnp {
                        if upnp.is_none() {
                            upnp = Some(Upnp::new().await?);
                        }
                        let map = upnp
                            .as_mut()
                            .unwrap()
                            .add_port(TCP, local_addr.parse()?)
                            .await?;
                        local_addr = map.local_addr();
                        pm = Some(map);
                    }
                    Mapper::new_tcp(key, local_addr, value, &cfg.tcp, pm).await?
                }
                "udp" | "udp+upnp" | "upnp+udp" => {
                    if url.scheme() != "udp" || global_upnp {
                        if upnp.is_none() {
                            upnp = Some(Upnp::new().await?);
                        }
                        let map = upnp
                            .as_mut()
                            .unwrap()
                            .add_port(UDP, local_addr.parse()?)
                            .await?;
                        local_addr = map.local_addr();
                        pm = Some(map);
                    }
                    Mapper::new_udp(key, local_addr, value, &cfg.udp, pm).await?
                }
                _ => Err(anyhow!("{ErrSchemeType} {}", url.scheme()))?,
            };
            mappers.push(mapper);
        }
        Ok(Self {
            mappers,
            watchers,
            upnp,
        })
    }

    pub async fn run(&mut self) {
        loop {
            let (addr, index, _) =
                select_all(self.mappers.iter_mut().map(|v| v.rx.recv().boxed())).await;
            if addr.is_none() {
                continue;
            }
            let addr = addr.unwrap();
            if let Some(mapper) = self.mappers.get_mut(index) {
                if let Some(upnp) = mapper.upnp.as_mut() {
                    if let Err(e) = self.upnp.as_ref().unwrap().renew_port(upnp).await {
                        error!(mapper = mapper.name(), "{e}");
                    }
                }
                if mapper.changed(&addr) {
                    let scheme = mapper.bind.scheme();
                    if let Some(upnp) = &mapper.upnp {
                        info!(
                            mapper = mapper.name(),
                            "{scheme}://{} <-- upnp://{}:{} --> {scheme}://{}",
                            mapper.local_addr(),
                            self.upnp
                                .as_ref()
                                .unwrap()
                                .external_ip()
                                .await
                                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                            upnp.external_port,
                            addr
                        );
                    } else {
                        info!(
                            mapper = mapper.name(),
                            "{scheme}://{} <--> {scheme}://{}",
                            mapper.local_addr(),
                            addr
                        );
                    }
                    for md in mapper.metadata.iter() {
                        if let Some(watcher) = self.watchers.get(&md.name) {
                            if let Err(e) = watcher.new_address(&addr, md).await {
                                error!(
                                    mapper = mapper.name(),
                                    watcher = watcher.kind(),
                                    name = &md.name,
                                    "{e}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn close(&mut self) {
        for mapper in self.mappers.iter_mut() {
            if let Some(pm) = mapper.upnp.as_mut() {
                let _ = self.upnp.as_ref().unwrap().remove_port(pm).await;
            }
            mapper.handle.close();
        }
    }
}
