//! HTTP proxy server that intercepts and forwards requests.

use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::models::Exchange;

pub struct ProxyServer {
    listen_addr: SocketAddr,
    exchange_tx: mpsc::Sender<Exchange>,
}

impl ProxyServer {
    pub fn new(listen_addr: SocketAddr, exchange_tx: mpsc::Sender<Exchange>) -> Self {
        Self {
            listen_addr,
            exchange_tx,
        }
    }

    pub async fn run(self: Arc<Self>) -> Result<()> {
        let listener = TcpListener::bind(self.listen_addr).await?;
        eprintln!("[ledger] proxy listening on {}", self.listen_addr);

        loop {
            let (_stream, _remote) = listener.accept().await?;
            let tx = self.exchange_tx.clone();
            tokio::spawn(async move {
                let _ = tx;
                todo!(
                    "Implement HTTP/1.1 request parsing, forwarding, and response capture via hyper"
                );
            });
        }
    }
}

pub fn parse_addr(addr: &str) -> Result<SocketAddr> {
    addr.parse::<SocketAddr>()
        .map_err(|_| anyhow::anyhow!("invalid address: {addr}"))
}
