use std::net::SocketAddr;
use tokio::task::JoinHandle;

pub struct Client {
    name: String,
    local_addr: SocketAddr,
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
