use crate::client::{Callback, Client, MappedAddress};
use anyhow::{Error, Result};
use std::io;
use std::io::BufReader;
use std::net::SocketAddr;
use std::time::Duration;
use stun::agent::TransactionId;
use stun::message::{Getter, Message, BINDING_REQUEST, MAGIC_COOKIE, TRANSACTION_ID_SIZE};
use stun::xoraddr::XorMappedAddress;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpSocket, TcpStream, ToSocketAddrs};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::time;
use tokio::time::{sleep, sleep_until, Instant};
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
    keepalive_url: String,
    /// The interval in seconds between sending binding request messages.
    stun_interval: u64
});

impl Builder {
    pub fn new(name: String, local_addr: impl Into<String>, callback: Callback) -> Builder {
        Builder {
            name,
            local_addr: local_addr.into(),
            keepalive_url: "http://www.baidu.com".to_string(),
            stun_addrs: str2vec!("turn.cloud-rtc.com:80"),
            interval: 50,
            stun_interval: 300,
            callback,
        }
    }

    pub fn keepalive_url(mut self, url: impl Into<String>) -> Self {
        self.keepalive_url = url.into();
        self
    }

    pub fn stun_interval(mut self, interval: u64) -> Self {
        if interval > 0 {
            self.stun_interval = interval;
        }
        self
    }

    pub async fn build(self) -> Result<Client> {
        worker(
            self.name,
            self.local_addr.parse()?,
            self.keepalive_url.to_string(),
            self.stun_addrs,
            self.interval,
            self.stun_interval,
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
    stun_interval: u64,
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
    let stun_name = name.clone();
    let (reset_tx, mut reset_rx) = mpsc::channel(1);
    let (stun_tx, mut stun_rx) = watch::channel(());
    let (addr_tx, mut addr_rx) = watch::channel(MappedAddress::default());
    let stun_handle = tokio::spawn(async move {
        let mut i = 0;
        loop {
            tokio::select! {
                res = async {
                    stun_rx.changed().await?;
                    let stun_addr = stun_addrs.get(i).unwrap();
                    match map_address(local_addr, stun_addr).await {
                        Ok(addr) => {
                            addr_tx.send_replace(MappedAddress::from(addr));
                        }
                        Err(e) => error!(op = "stun", stun = stun_addr, mapper = stun_name, "{e}")
                    }
                    Ok::<(), Error>(())
                } => {
                    if res.is_err() {
                        return;
                    }
                    i = (i + 1) % stun_addrs.len();
                },
                _ = reset_rx.recv() => {
                    stun_rx.mark_unchanged();
                }
            }
        }
    });
    let worker_handle = tokio::spawn(async move {
        let payload = format!(
            "HEAD {} HTTP/1.1\r\nHost: {}\r\nConnection: keep-alive\r\n\r\n",
            &url[Position::BeforePath..],
            host
        );
        loop {
            match new_connection(local_addr, &remote_addr).await {
                Err(e) => {
                    error!(op = "connect", mapper = worker_name, "{e}");
                    sleep(Duration::from_secs(RETRY_INTERVAL)).await;
                }
                Ok(mut stream) => {
                    let (mut reader, mut writer) = stream.split();
                    let mut interval = time::interval(Duration::from_secs(interval));
                    let mut stun = time::interval(Duration::from_secs(stun_interval));
                    let mut deadline: Option<Instant> = None;
                    let mut mapped_addr = None;
                    addr_rx.mark_unchanged();
                    let mut buf = vec![0; 1024];
                    let mut total: u64 = 0;
                    loop {
                        tokio::select! {
                            res = reader.read(&mut buf) => {
                                match res {
                                    Ok(n) => {
                                        total += n as u64;
                                        if n == 0 {
                                            error!(
                                                op = "read",
                                                mapper = worker_name,
                                                "connection unexpectedly closed with {total} bytes received"
                                            );
                                            break;
                                        } else {
                                            deadline = None;
                                        }
                                    }
                                    Err(e) => {
                                        error!(op = "read", mapper = worker_name, "{e}");
                                        break;
                                    }
                                }
                            }
                            _ = interval.tick(), if deadline.is_none() => {
                                if let Err(e) = writer.write_all(payload.as_bytes()).await {
                                    error!(op = "write", mapper = worker_name, "{e}");
                                    break;
                                }
                                deadline = Some(Instant::now() + Duration::from_secs(RETRY_INTERVAL));
                            }
                            _ = sleep_until(deadline.unwrap_or(Instant::now())), if deadline.is_some() => {
                                error!(
                                    op = "read",
                                    mapper = worker_name,
                                    "timed out waiting for a response from keepalive server"
                                );
                                break;
                            }
                            _ = stun.tick() => {
                                stun_tx.send_replace(());
                            }
                            _ = addr_rx.changed() => {
                                let new_addr = addr_rx.borrow_and_update().clone();
                                if let Some(ref addr) = mapped_addr {
                                    if &new_addr != addr {
                                        warn!(
                                            op = "stun",
                                            mapper = worker_name,
                                            "connection is closing because mapped address has changed"
                                        );
                                        break;
                                    }
                                } else {
                                    mapped_addr = Some(new_addr.clone());
                                    // Only the first mapped address is sent to the callback.
                                    // A different mapped address might indicate that the mapping is broken.
                                    if callback.send(new_addr.into()).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                    drop(stream);
                    // Cancel the current STUN request.
                    if reset_tx.send(()).await.is_err() {
                        return;
                    }
                    sleep(Duration::from_secs(RETRY_INTERVAL)).await;
                }
            }
        }
    });
    Ok(Client {
        name,
        local_addr,
        tasks: vec![worker_handle, stun_handle],
    })
}
