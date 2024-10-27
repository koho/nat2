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

/// A `builder` facilitates the creation of hole punching client.
macro_rules! builder {
    ($name:ident {$($(#[$meta:meta])*$field:ident:$ty:ty),*}) => {
        pub struct $name {
            /// Name of the client.
            name: String,
            /// Request binding address.
            /// The port may be zero.
            local_addr: String,
            /// STUN server address:port pairs.
            /// It selects hosts based on round-robin ordering.
            stun_addrs: Vec<String>,
            /// The interval in seconds between sending request.
            interval: u64,
            /// Callback for receiving the mapped address.
            callback: Callback,
            $(
                $(#[$meta])*
                $field: $ty,
            )*
        }

        impl $name {
            pub fn stun_addrs(mut self, addrs: impl IntoIterator<Item = impl Into<String>>) -> Self {
                let addrs: Vec<String> = addrs.into_iter().map(|v| v.into()).collect();
                if !addrs.is_empty() {
                    self.stun_addrs = addrs;
                }
                self
            }

            pub fn interval(mut self, interval: u64) -> Self {
                if interval > 0 {
                    self.interval = interval;
                }
                self
            }
        }
    };
}

/// Convert a list of `&str` to a `String` vector.
macro_rules! str2vec {
    ($($s:literal),*) => {
        vec![
            $(
                $s.to_string(),
            )*
        ]
    };
}

pub mod tcp;
pub mod udp;
