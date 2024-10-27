use crate::client::{Callback, Client};
use anyhow::Result;
use std::io;
use std::io::BufReader;
use std::net::SocketAddr;
use std::time::Duration;
use stun::agent::TransactionId;
use stun::message::{Getter, Message, BINDING_REQUEST, MAGIC_COOKIE, TRANSACTION_ID_SIZE};
use stun::xoraddr::XorMappedAddress;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpSocket, TcpStream, ToSocketAddrs};
use tokio::time;
use tokio::time::sleep;
use tracing::{error, warn};
use url::ParseError::{EmptyHost, InvalidPort};
use url::{Position, Url};

/// The amount of time (in seconds) to wait before retrying.
const RETRY_INTERVAL: u64 = 10;

/// Creates a new TCP connection.
async fn new_connection<A: ToSocketAddrs>(
    local_addr: SocketAddr,
    remote_addr: A,
) -> io::Result<TcpStream> {
    let mut last_err = None;
    for addr in lookup_host(remote_addr).await? {
        if let SocketAddr::V4(_) = addr {
            let sock = TcpSocket::new_v4()?;
            sock.set_reuseaddr(true)?;
            sock.bind(local_addr)?;
            match sock.connect(addr).await {
                Ok(stream) => return Ok(stream),
                Err(e) => last_err = Some(e),
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve to any address",
        )
    }))
}

/// Read the mapped address from STUN server.
async fn map_address<A: ToSocketAddrs>(
    local_addr: SocketAddr,
    remote_addr: A,
) -> Result<XorMappedAddress> {
    let mut stream = new_connection(local_addr, remote_addr).await?;
    let mut msg = Message::new();
    msg.build(&[Box::new(TransactionId::new()), Box::new(BINDING_REQUEST)])?;
    msg.encode();
    stream.write_all(&msg.raw).await?;
    let message_type = stream.read_u16().await?;
    let message_len = stream.read_u16().await?;
    let mut buf = vec![0; message_len as usize + TRANSACTION_ID_SIZE + size_of_val(&MAGIC_COOKIE)];
    stream.read_exact(&mut buf).await?;
    let mut msg = Message::new();
    let payload = [
        &message_type.to_be_bytes(),
        &message_len.to_be_bytes(),
        buf.as_slice(),
    ]
    .concat();
    let mut reader = BufReader::new(payload.as_slice());
    msg.read_from(&mut reader)?;
    let mut xor_addr = XorMappedAddress::default();
    xor_addr.get_from(&msg)?;
    Ok(xor_addr)
}

builder!(Builder {
    /// The url used to maintain a long-lived TCP connection.
    keepalive_url: String
});

impl Builder {
    pub fn new(name: String, local_addr: impl Into<String>, callback: Callback) -> Builder {
        Builder {
            name,
            local_addr: local_addr.into(),
            keepalive_url: "http://www.baidu.com".to_string(),
            stun_addrs: str2vec!("turn.cloud-rtc.com:80", "stun.xiaoyaoyou.xyz:3478"),
            interval: 50,
            callback,
        }
    }

    pub fn keepalive_url(mut self, url: impl Into<String>) -> Self {
        self.keepalive_url = url.into();
        self
    }

    pub async fn build(self) -> Result<Client> {
        worker(
            self.name,
            self.local_addr.parse()?,
            self.keepalive_url.to_string(),
            self.stun_addrs,
            self.interval,
            self.callback,
        )
        .await
    }
}

/// Returns a TCP hole punching client.
async fn worker(
    name: String,
    local_addr: SocketAddr,
    keepalive_url: String,
    stun_addrs: Vec<String>,
    interval: u64,
    callback: Callback,
) -> Result<Client> {
    let url = Url::parse(keepalive_url.as_str())?;
    let mut host = url.host().ok_or(EmptyHost)?.to_string();
    let port = url.port_or_known_default().ok_or(InvalidPort)?.to_string();
    let remote_addr = format!("{}:{}", host, port);
    host.push_str(
        &url.port()
            .map_or(String::new(), |v| format!(":{}", v.to_string())),
    );
    // Determine the local binding address and reuse it in further connections.
    let sock = TcpSocket::new_v4()?;
    sock.set_reuseaddr(true)?;
    sock.bind(local_addr)?;
    let local_addr = sock.local_addr()?;
    let worker_name = name.clone();
    let handle = tokio::spawn(async move {
        let mut discard = tokio::io::empty();
        let payload = format!(
            "HEAD {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n\r\n",
            &url[Position::BeforePath..],
            host
        );
        loop {
            let mut i = 0;
            match new_connection(local_addr, &remote_addr).await {
                Err(e) => {
                    error!(op = "connect", mapper = name, "{e}");
                    sleep(Duration::from_secs(RETRY_INTERVAL)).await;
                }
                Ok(mut stream) => {
                    let (mut reader, mut writer) = stream.split();
                    let read = tokio::io::copy(&mut reader, &mut discard);
                    tokio::pin!(read);
                    let mut interval = time::interval(Duration::from_secs(interval));
                    loop {
                        tokio::select! {
                            res = &mut read => {
                                match res {
                                    Ok(n) => warn!(op = "read", mapper = name, "connection unexpectedly closed with {n} bytes received"),
                                    Err(e) => error!(op = "read", mapper = name, "{e}")
                                }
                                break;
                            }
                            _ = interval.tick() => {
                                if let Err(e) = writer.write(payload.as_bytes()).await {
                                    error!(op = "write", mapper = name, "{e}");
                                    break;
                                }
                                let stun_addr = stun_addrs.get(i).unwrap();
                                match map_address(local_addr, stun_addr).await {
                                    Ok(addr) => {
                                        if callback.send(addr).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => error!(op = "stun", stun = stun_addr, mapper = name, "{e}")
                                }
                                i = (i + 1) % stun_addrs.len();
                            }
                        }
                    }
                    sleep(Duration::from_secs(RETRY_INTERVAL)).await;
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
