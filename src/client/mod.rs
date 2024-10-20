use std::net::SocketAddr;
use stun::xoraddr::XorMappedAddress;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;

/// Callback for receiving the mapped address.
pub type Callback = Sender<XorMappedAddress>;

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

pub mod tcp;
pub mod udp;
