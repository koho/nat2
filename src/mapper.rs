use crate::client;
use crate::client::{Callback, Client};
use crate::config::{Config, Tcp, Udp};
use crate::upnp::{PortMap, Upnp};
use crate::watcher::alidns::AliDns;
use crate::watcher::cf::Cloudflare;
use crate::watcher::dnspod::DnsPod;
use crate::watcher::http::Http;
use crate::watcher::script::Script;
use crate::watcher::Watcher;
use anyhow::{anyhow, Result};
use futures::future::join_all;
use igd_next::PortMappingProtocol::{TCP, UDP};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::ops::Deref;
use std::sync::Arc;
use stun::xoraddr::XorMappedAddress;
use stun::Error::ErrSchemeType;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc::channel;
use tokio::task::JoinHandle;
use tracing::{error, info};
use url::ParseError::{EmptyHost, InvalidPort};
use url::Url;

struct Mapper {
    /// Protocol and local socket binding address.
    protocol: &'static str,
    /// NAT client.
    handle: Client,
    /// Public IPv4 address and port.
    public: Option<String>,
}

impl Mapper {
    async fn new_tcp(
        name: String,
        local_addr: String,
        option: &Option<Tcp>,
        callback: Callback,
    ) -> Result<Mapper> {
        let mut c = client::tcp::Builder::new(name, local_addr.clone(), callback);
        if let Some(opt) = option {
            if let Some(addrs) = &opt.stun {
                c = c.stun_addrs(addrs);
            }
            if let Some(url) = &opt.keepalive {
                c = c.keepalive_url(url);
            }
            if let Some(sec) = opt.interval {
                c = c.interval(sec);
            }
            if let Some(sec) = opt.stun_interval {
                c = c.stun_interval(sec);
            }
        }
        Ok(Mapper {
            protocol: "tcp",
            handle: c.build().await?,
            public: None,
        })
    }

    async fn new_udp(
        name: String,
        local_addr: String,
        option: &Option<Udp>,
        callback: Callback,
    ) -> Result<Mapper> {
        let mut c = client::udp::Builder::new(name, local_addr.clone(), callback);
        if let Some(opt) = option {
            if let Some(addrs) = &opt.stun {
                c = c.stun_addrs(addrs);
            }
            if let Some(sec) = opt.interval {
                c = c.interval(sec);
            }
        }
        Ok(Mapper {
            protocol: "udp",
            handle: c.build().await?,
            public: None,
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

    /// Returns the mapper name.
    fn name(&self) -> &str {
        self.handle.name()
    }

    /// Returns the socket binding address.
    fn local_addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    /// Stop the internal NAT client.
    fn close(&self) {
        self.handle.close()
    }
}

fn upnp_enabled(url: &String) -> bool {
    if let Ok(url) = Url::parse(url.as_str()) {
        match url.scheme() {
            "tcp+upnp" | "upnp+tcp" | "udp+upnp" | "upnp+udp" => true,
            _ => false,
        }
    } else {
        false
    }
}

pub struct Closer {
    tasks: Vec<JoinHandle<()>>,
    close: Sender<()>,
}

impl Closer {
    pub async fn close(self) {
        self.close.send(()).unwrap();
        join_all(self.tasks).await;
    }
}

/// Merge different watchers into a single hashmap.
macro_rules! map_watcher {
    ($($bind:pat = $cfg:expr => $watcher:expr),*) => {{
        let mut watchers: HashMap<String, Arc<dyn Watcher + Send + Sync>> = HashMap::new();
        $(
            for (key, value) in $cfg.into_iter() {
                let name = key.clone();
                let $bind = (key, value);
                watchers.insert(name, Arc::new($watcher));
            }
        )*
        watchers
    }};
}

pub async fn run(cfg: Config) -> Result<Closer> {
    // Watcher list.
    let watcher_map = map_watcher!(
        (key, value) = cfg.dnspod => DnsPod::new(key, value.secret_id, value.secret_key),
        (key, value) = cfg.http => Http::new(key, value.url, value.method.as_str(), value.body, value.headers)?,
        (key, value) = cfg.script => Script::new(key, value.path, value.args),
        (key, value) = cfg.alidns => AliDns::new(key, value.secret_id, value.secret_key, value.url)?,
        (key, value) = cfg.cf => Cloudflare::new(key, value.token)
    );
    // UPnP feature.
    let global_upnp = !matches!(cfg.upnp, Some(false));
    let upnp = if global_upnp || cfg.map.keys().any(upnp_enabled) {
        Some(Arc::new(Upnp::new().await?))
    } else {
        None
    };
    let (close, _) = broadcast::channel(1);
    let mut tasks = Vec::with_capacity(cfg.map.len());
    // Mapper list.
    for (key, value) in cfg.map.into_iter() {
        let url = Url::parse(key.as_str())?;
        let ip = url.host().ok_or(anyhow!("{EmptyHost} in {key}"))?;
        let port = url.port().ok_or(anyhow!("{InvalidPort} in {key}"))?;
        let mut local_addr = format!("{ip}:{port}");
        // Validate watcher metadata.
        let mut watchers = Vec::with_capacity(value.len());
        for (i, md) in value.into_iter().enumerate() {
            if let Some(watcher) = watcher_map.get(&md.name) {
                watcher
                    .validate(&md)
                    .map_err(|e| anyhow!("{e} in {key} at index {i}"))?;
                watchers.push((watcher.clone(), md));
            } else {
                return Err(anyhow!(
                    "no watcher named `{}` in {key} at index {i}",
                    md.name
                ));
            }
        }
        let mut pm: Option<(Arc<Upnp>, PortMap)> = None;
        let (tx, mut rx) = channel(1);
        let mut mapper = match url.scheme() {
            "tcp" | "tcp+upnp" | "upnp+tcp" => {
                if url.scheme() != "tcp" || global_upnp {
                    let upnp = upnp.as_ref().unwrap();
                    let map = upnp.add_port(TCP, local_addr.parse()?).await?;
                    local_addr = map.local_addr();
                    pm = Some((upnp.clone(), map));
                }
                Mapper::new_tcp(key, local_addr, &cfg.tcp, tx).await?
            }
            "udp" | "udp+upnp" | "upnp+udp" => {
                if url.scheme() != "udp" || global_upnp {
                    let upnp = upnp.as_ref().unwrap();
                    let map = upnp.add_port(UDP, local_addr.parse()?).await?;
                    local_addr = map.local_addr();
                    pm = Some((upnp.clone(), map));
                }
                Mapper::new_udp(key, local_addr, &cfg.udp, tx).await?
            }
            _ => Err(anyhow!("{ErrSchemeType} {}", url.scheme()))?,
        };
        let mut close = close.subscribe();
        tasks.push(tokio::spawn(async move {
            let mut failed: isize = -1;
            loop {
                tokio::select! {
                    Some(addr) = rx.recv() => {
                        if let Some((upnp, pm)) = pm.as_mut() {
                            if let Err(e) = upnp.renew_port(pm).await {
                                error!(mapper = mapper.name(), upnp="renew", "{e}");
                            }
                        }
                        let changed = mapper.changed(&addr);
                        if changed {
                            let scheme = mapper.protocol;
                            if let Some((upnp, pm)) = pm.as_ref() {
                                info!(
                                    mapper = mapper.name(),
                                    "{scheme}://{} <-- upnp://{}:{} --> {scheme}://{}",
                                    pm.forward_addr,
                                    upnp.external_ip()
                                        .await
                                        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                                    pm.external_port,
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
                        }
                        if changed || failed >= 0 {
                            let i = if changed { 0 } else { failed as usize };
                            failed = -1;
                            for (j, (watcher, md)) in watchers[i..].iter().enumerate() {
                                if let Err(e) = watcher.new_address(&addr, md).await {
                                    failed = (i + j) as isize;
                                    error!(
                                        mapper = mapper.name(),
                                        watcher = watcher.kind(),
                                        name = &md.name,
                                        "{e}"
                                    );
                                    break;
                                }
                            }
                        }
                    },
                    _ = close.recv() => {
                        if let Some((upnp, pm)) = pm.as_mut() {
                            let _ = upnp.remove_port(pm).await;
                        }
                        mapper.close();
                        break;
                    },
                }
            }
        }));
    }
    Ok(Closer { tasks, close })
}
