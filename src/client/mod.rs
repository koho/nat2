use std::net::SocketAddr;
use tokio::task::JoinHandle;

/// Hole punching client.
pub struct Client {
    /// Name of the client.
    name: String,
    /// Socket binding address.
    /// The port wouldn't be zero.
    local_addr: SocketAddr,
    /// Task handle.
    handle: JoinHandle<()>,
}

impl Client {
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn close(&self) {
        self.handle.abort()
    }
}

pub mod udp;
pub mod tcp;
