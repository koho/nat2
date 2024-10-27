use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::Path;

/// Configuration for Nat2.
#[derive(Deserialize)]
pub struct Config {
    /// TCP mapping global options.
    pub tcp: Option<Tcp>,
    /// UDP mapping global options.
    pub udp: Option<Udp>,
    /// NAT Mapping List.
    pub map: HashMap<String, Vec<Metadata>>,
    /// Use UPnP feature. Default is true.
    /// You can also use scheme `tcp+upnp://` or `udp+upnp://`
    /// to enable UPnP for specific mapping.
    pub upnp: Option<bool>,
    /// Configuration for DNSPod provider watcher.
    #[serde(default)]
    pub dnspod: HashMap<String, DnsPod>,
    /// Configuration for AliDNS provider watcher.
    #[serde(default)]
    pub alidns: HashMap<String, AliDNS>,
    /// Configuration for HTTP watcher.
    #[serde(default)]
    pub http: HashMap<String, Http>,
    /// Configuration for script watcher.
    #[serde(default)]
    pub script: HashMap<String, Script>,
}

/// Configuration for DNSPod provider.
#[derive(Deserialize)]
pub struct DnsPod {
    /// Similar to username.
    pub secret_id: String,
    /// Similar to password.
    pub secret_key: String,
}

/// Configuration for AliDNS provider.
#[derive(Deserialize)]
pub struct AliDNS {
    /// The request URL may vary by region.
    pub url: Option<String>,
    /// Similar to username.
    pub secret_id: String,
    /// Similar to password.
    pub secret_key: String,
}

/// Configuration for HTTP API.
#[derive(Deserialize)]
pub struct Http {
    /// Request url could contain placeholder `{ip}` and `{port}` which
    /// will be replaced with real value before sending the request.
    pub url: String,
    /// Request method.
    pub method: String,
    /// Request body could be JSON string, plain text, etc...
    /// Placeholder `{ip}` and `{port}` are supported.
    pub body: Option<String>,
    /// Request headers.
    /// For example, `Content-Type` should be set based on the content in the `body`.
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// Configuration for script.
#[derive(Deserialize)]
pub struct Script {
    /// Path to executable file.
    pub path: String,
    /// Arguments to pass to the program.
    /// If the `value` field in watcher metadata is not empty, it
    /// will be passed to the program as the last argument.
    #[serde(default)]
    pub args: Vec<String>,
}

/// TCP mapping global options.
#[derive(Deserialize)]
pub struct Tcp {
    /// TCP STUN server address:port pair.
    /// The server must support STUN over TCP protocol.
    pub stun: Option<String>,
    /// Internet connectivity check url. Only HTTP protocol is supported.
    /// We will periodically fetch this url to maintain a long-lived TCP connection.
    pub keepalive: Option<String>,
    /// The interval in seconds between sending binding request messages
    /// and fetching the keepalive url.
    pub interval: Option<u64>,
}

/// UDP mapping global options.
#[derive(Deserialize)]
pub struct Udp {
    /// UDP STUN server address:port pair.
    pub stun: Option<String>,
    /// The interval in seconds between sending binding request messages.
    pub interval: Option<u64>,
}

/// Metadata of watcher.
#[derive(Deserialize)]
pub struct Metadata {
    /// Name of the watcher defined in the watcher list.
    pub name: String,
    /// Value could contain placeholder `{ip}` and `{port}` which
    /// will be replaced with real value in the watcher.
    pub value: String,
    /// Domain name.
    pub domain: Option<String>,
    /// Record type.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Record priority.
    /// This field is required for record type SVCB and HTTPS.
    pub priority: Option<u8>,
    /// DNS record id.
    /// This field disables the automatic creation of dns records.
    pub rid: Option<String>,
    /// TTL to use for dns records.
    pub ttl: Option<u32>,
}

pub(crate) fn load<P: AsRef<Path>>(path: P) -> io::Result<Config> {
    let f = File::open(path)?;
    let cfg = serde_json::from_reader(f)?;
    Ok(cfg)
}
