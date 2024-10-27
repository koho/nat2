use crate::client::{Callback, Client};
use anyhow::Result;
use hex::ToHex;
use std::io::BufReader;
use std::net::SocketAddr;
use stun::agent::TransactionId;
use stun::message::{Getter, Message, BINDING_REQUEST};
use stun::xoraddr::XorMappedAddress;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::time::{self, Duration};
use tracing::error;

/// Send a binding request to STUN server.
async fn send_request<A: ToSocketAddrs>(sock: &UdpSocket, stun_addr: A) -> Result<TransactionId> {
    let mut msg = Message::new();
    let id = TransactionId::new();
    msg.build(&[Box::new(id), Box::new(BINDING_REQUEST)])?;
    msg.encode();
    sock.send_to(&msg.raw, stun_addr).await?;
    Ok(id)
}

builder!(Builder {});

impl Builder {
    pub fn new(name: String, local_addr: impl Into<String>, callback: Callback) -> Builder {
        Builder {
            name,
            local_addr: local_addr.into(),
            stun_addrs: str2vec!(
                "stun.chat.bilibili.com:3478",
                "stun.douyucdn.cn:18000",
                "stun.hitv.com:3478",
                "stun.miwifi.com:3478"
            ),
            interval: 20,
            callback,
        }
    }

    pub async fn build(self) -> Result<Client> {
        worker(
            self.name,
            self.local_addr.parse()?,
            self.stun_addrs,
            self.interval,
            self.callback,
        )
        .await
    }
}

/// Returns a UDP hole punching client.
async fn worker(
    name: String,
    local_addr: SocketAddr,
    stun_addrs: Vec<String>,
    interval: u64,
    callback: Callback,
) -> Result<Client> {
    let sock = UdpSocket::bind(local_addr).await?;
    let local_addr = sock.local_addr()?;
    let worker_name = name.clone();
    let handle = tokio::spawn(async move {
        let mut buf = [0; 1024];
        let mut req: Option<TransactionId> = None;
        let mut interval = time::interval(Duration::from_secs(interval));
        let mut i = 0;
        let mut first_request = true;
        let mut stun_addr = stun_addrs.get(i).unwrap();
        loop {
            tokio::select! {
                Ok((len, _)) = sock.recv_from(&mut buf) => {
                    if req.is_none() {
                        continue;
                    }
                    let mut msg = Message::new();
                    let mut reader = BufReader::new(&buf[..len]);
                    if let Err(e) = msg.read_from(&mut reader) {
                        error!(stun = stun_addr, mapper = name, "{e}");
                        continue;
                    }
                    if let Some(r) = req {
                        // Ignore outdated or invalid response.
                        if msg.transaction_id != r {
                            continue;
                        }
                        req = None;
                    }
                    let mut addr = XorMappedAddress::default();
                    if let Err(e) = addr.get_from(&msg) {
                        error!(
                            transaction_id = msg.transaction_id.0.encode_hex::<String>(),
                            stun = stun_addr,
                            mapper = name,
                            "{e}"
                        );
                        continue;
                    }
                    if callback.send(addr).await.is_err() {
                        return;
                    }
                }
                _ = interval.tick() => {
                    if let Some(r) = req {
                        error!(
                            transaction_id = r.0.encode_hex::<String>(),
                            stun = stun_addr,
                            mapper = name,
                            "no response from stun server"
                        );
                    }
                    if first_request {
                        first_request = false;
                    } else {
                        i = (i + 1) % stun_addrs.len();
                        stun_addr = stun_addrs.get(i).unwrap();
                    }
                    match send_request(&sock, stun_addr).await {
                        Ok(id) => {
                            req = Some(id);
                        }
                        Err(e) => error!(stun = stun_addr, mapper = name, "{e}")
                    };
                }
            }
        }
    });
    Ok(Client {
        name: worker_name,
        local_addr,
        handle,
    })
}
