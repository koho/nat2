use std::io::BufReader;
use std::net::{SocketAddr};
use stun::agent::TransactionId;
use stun::message::{Getter, Message, BINDING_REQUEST};
use stun::xoraddr::XorMappedAddress;
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio::sync::mpsc::Sender;
use tokio::time::{self, Duration};
use crate::client::Client;
use anyhow::Result;
use tracing::error;

async fn send_request<A: ToSocketAddrs>(sock: &UdpSocket, stun_addr: A) -> Result<TransactionId> {
    let mut msg = Message::new();
    let id = TransactionId::new();
    msg.build(&[Box::new(id), Box::new(BINDING_REQUEST)])?;
    msg.encode();
    sock.send_to(&msg.raw, stun_addr).await?;
    Ok(id)
}

pub struct Builder {
    name: String,
    local_addr: String,
    stun_addr: String,
    interval: u64,
    callback: Sender<XorMappedAddress>,
}

impl Builder {
    pub fn new(name: String, local_addr: impl Into<String>, callback: Sender<XorMappedAddress>) -> Builder {
        Builder {
            name,
            local_addr: local_addr.into(),
            stun_addr: "stun.chat.bilibili.com:3478".to_string(),
            interval: 50,
            callback,
        }
    }

    pub fn stun_addr(mut self, addr: impl Into<String>) -> Self {
        self.stun_addr = addr.into();
        self
    }

    pub fn interval(mut self, interval: u64) -> Self {
        self.interval = interval;
        self
    }

    pub async fn build(self) -> Result<Client> {
        worker(self.name, self.local_addr.parse()?, self.stun_addr.to_string(), self.interval, self.callback).await
    }
}

async fn worker(name: String, local_addr: SocketAddr, stun_addr: String, interval: u64, callback: Sender<XorMappedAddress>) -> Result<Client> {
    let sock = UdpSocket::bind(local_addr).await?;
    let local_addr= sock.local_addr()?;
    let worker_name = name.clone();
    let handle = tokio::spawn(async move {
        let mut buf = [0; 1024];
        let mut req: Option<TransactionId> = None;
        let mut interval = time::interval(Duration::from_secs(interval));
        loop {
            tokio::select! {
                Ok((len, _)) = sock.recv_from(&mut buf) => {
                    let mut msg = Message::new();
                    let mut reader = BufReader::new(&buf[..len]);
                    if let Err(_) = msg.read_from(&mut reader) {
                        continue;
                    }
                    if let Some(r) = req {
                        if msg.transaction_id != r {
                            continue;
                        }
                        req = None;
                    }
                    let mut addr = XorMappedAddress::default();
                    if let Err(e) = addr.get_from(&msg) {
                        error!(mapper=name, "{e}");
                        continue;
                    }
                    if callback.send(addr).await.is_err() {
                        return;
                    }
                }
                _ = interval.tick() => {
                    if req.is_some() {
                        error!(mapper=name, "no response from stun server");
                    }
                    match send_request(&sock, &stun_addr).await {
                        Ok(id) => {
                            req = Some(id);
                        }
                        Err(e) => error!(mapper=name, "{e}")
                    };
                }
            }
        }
    });
    Ok(Client{
        name: worker_name,
        local_addr,
        handle,
    })
}
