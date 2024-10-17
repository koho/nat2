use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::Path;
use serde::{Deserialize};

#[derive(Deserialize)]
pub struct Config {
    pub tcp: Option<Tcp>,
    pub udp: Option<Udp>,
    pub map: HashMap<String, Vec<Metadata>>,
    pub upnp: Option<bool>,
    #[serde(default)]
    pub dnspod: HashMap<String, DnsPod>,
    #[serde(default)]
    pub http: HashMap<String, Http>,
}

#[derive(Deserialize)]
pub struct DnsPod {
    pub secret_id: String,
    pub secret_key: String,
}

#[derive(Deserialize)]
pub struct Http {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Deserialize)]
pub struct Tcp {
    pub stun: Option<String>,
    pub keepalive: Option<String>,
    pub interval: Option<u64>,
}

#[derive(Deserialize)]
pub struct Udp {
    pub stun: Option<String>,
    pub interval: Option<u64>,
}

#[derive(Deserialize)]
pub struct Metadata {
    pub name: String,
    pub value: String,
    pub domain: Option<String>,
    #[serde(rename="type")]
    pub kind: Option<String>,
    pub priority: Option<u8>,
    pub rid: Option<u64>,
    pub ttl: Option<u32>,
}

pub(crate) fn load<P: AsRef<Path>>(path: P) -> io::Result<Config> {
    let f = File::open(path)?;
    let cfg = serde_json::from_reader(f)?;
    Ok(cfg)
}
