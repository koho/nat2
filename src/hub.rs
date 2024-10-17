use crate::client::Client;
use crate::config::{Config, Metadata, Tcp, Udp};
use crate::watcher::dnspod::DnsPod;
use crate::watcher::Watcher;
use crate::{client};
use futures::{future::select_all, FutureExt};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use stun::Error::ErrSchemeType;
use stun::xoraddr::XorMappedAddress;
use tokio::sync::mpsc::channel;
use tokio::sync::mpsc::Receiver;
use url::ParseError::{EmptyHost, InvalidPort};
use url::Url;
use tracing::{error, info};
use anyhow::{anyhow, Result};
use igd_next::PortMappingProtocol::{TCP, UDP};
use crate::upnp::{PortMap, Upnp};
use crate::watcher::http::Http;

pub struct Hub {
    mappers: Vec<Mapper>,
    watchers: HashMap<String, Box<dyn Watcher>>,
    upnp: Option<Upnp>,
}

struct Mapper {
    bind: Url,
    metadata: Vec<Metadata>,
    handle: Client,
    public: Option<String>,
    upnp: Option<PortMap>,
    rx: Receiver<XorMappedAddress>,
}

impl Mapper {
    async fn new_tcp(name: String, local_addr: String, metadata: Vec<Metadata>, option: &Option<Tcp>, upnp: Option<PortMap>) -> Result<Mapper> {
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

    async fn new_udp(name: String, local_addr: String, metadata: Vec<Metadata>, option: &Option<Udp>, upnp: Option<PortMap>) -> Result<Mapper> {
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

    fn name(&self) -> &str {
        self.handle.name()
    }

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
        let mut watchers: HashMap<String, Box<dyn Watcher>> = HashMap::new();
        for (key, value) in cfg.dnspod.into_iter() {
            watchers.insert(key, Box::new(DnsPod::new(value.secret_id, value.secret_key)));
        }
        for (key, value) in cfg.http.into_iter() {
            watchers.insert(key, Box::new(Http::new(value.url, value.method.as_str(), value.body, value.headers)?));
        }
        let global_upnp = !matches!(cfg.upnp, Some(false));
        let mut upnp: Option<Upnp> = None;
        if global_upnp {
            upnp = Some(Upnp::new().await?);
        }
        let mut mappers = vec![];
        for (key, value) in cfg.map.into_iter() {
            let url = Url::parse(key.as_str())?;
            let ip = url.host().ok_or(anyhow!("{EmptyHost} in {key}"))?;
            let port = url.port().ok_or(anyhow!("{InvalidPort} in {key}"))?;
            let mut local_addr = format!("{ip}:{port}");
            for (i, md) in value.iter().enumerate() {
                if let Some(watcher) = watchers.get(&md.name) {
                    watcher.validate(md).map_err(|e| anyhow!("{e} in {key} at index {i}"))?;
                } else {
                    return Err(anyhow!("no watcher named `{}` in {key} at index {i}", md.name));
                }
            }
            let mut pm: Option<PortMap> = None;
            let mapper = match url.scheme() {
                "tcp" | "tcp+upnp" | "upnp+tcp" => {
                    if url.scheme() != "tcp" || global_upnp {
                        if upnp.is_none() {
                            upnp = Some(Upnp::new().await?);
                        }
                        let map = upnp.as_mut().unwrap().add_port(TCP, local_addr.parse()?).await?;
                        local_addr = format!("0.0.0.0:{}", map.external_port);
                        pm = Some(map);
                    }
                    Mapper::new_tcp(key, local_addr, value, &cfg.tcp, pm).await?
                },
                "udp" | "udp+upnp" | "upnp+udp" => {
                    if url.scheme() != "udp" || global_upnp {
                        if upnp.is_none() {
                            upnp = Some(Upnp::new().await?);
                        }
                        let map = upnp.as_mut().unwrap().add_port(UDP, local_addr.parse()?).await?;
                        local_addr = format!("0.0.0.0:{}", map.external_port);
                        pm = Some(map);
                    }
                    Mapper::new_udp(key, local_addr, value, &cfg.udp, pm).await?
                },
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
            let (addr, index, _) = select_all(
                self.mappers.iter_mut().map(|v| v.rx.recv().boxed())
            ).await;
            if addr.is_none() {
                continue;
            }
            let addr = addr.unwrap();
            if let Some(mapper) = self.mappers.get_mut(index) {
                if let Some(upnp) = mapper.upnp.as_mut() {
                    if let Err(e) = self.upnp.as_ref().unwrap().renew_port(upnp).await {
                        error!(mapper=mapper.name(), "{e}");
                    }
                }
                if mapper.changed(&addr) {
                    let scheme = mapper.bind.scheme();
                    if let Some(upnp) = &mapper.upnp {
                        info!(mapper=mapper.name(), "{scheme}://{} <-- upnp://{}:{} --> {scheme}://{}",
                            mapper.local_addr(), self.upnp.as_ref().unwrap().external_ip().await.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                            upnp.external_port, addr);
                    } else {
                        info!(mapper=mapper.name(), "{scheme}://{} <--> {scheme}://{}", mapper.local_addr(), addr);
                    }
                    for md in mapper.metadata.iter() {
                        if let Some(watcher) = self.watchers.get(&md.name) {
                            if let Err(e) = watcher.new_address(&addr, md).await {
                                error!(mapper=mapper.name(), watcher=watcher.name(), name=&md.name, "{e}");
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn close(&mut self) {
        if let Some(upnp) = &self.upnp {
            for mapper in self.mappers.iter_mut() {
                if let Some(pm) = mapper.upnp.as_mut() {
                    let _ = upnp.remove_port(pm).await;
                }
                mapper.handle.close();
            }
        }
    }
}
