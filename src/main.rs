use bytes::BytesMut;
use futures::channel::mpsc::UnboundedReceiver;
use futures::TryFutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio::sync::{mpsc, watch};

use std::borrow::{Borrow, BorrowMut};
use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

/// struct that represents client connection
struct Client {
    // client -> server
    client_tx: ClientTx,
    // server -> client
    client_rx: ClientRx,

    address: SocketAddr,
    name: Option<String>,

    buf: BytesMut,

    recv_half: OwnedReadHalf,
    wr_half: OwnedWriteHalf,
}

/// mpsc sender channel for clients to broadcast their messages to the server.
type ClientTx = mpsc::UnboundedSender<String>;
/// watch receiver channel for clients to receive new messages from the server.
type ClientRx = watch::Receiver<String>;

/// mpsc receiver channel for the server to receive messages from clients
type ServerRx = mpsc::UnboundedReceiver<String>;
/// watch sender channel for the server to broadcast new messages to clients.
type ServerTx = watch::Sender<String>;

type StateTx = mpsc::UnboundedSender<Connection>;
type StateRx = mpsc::UnboundedReceiver<Connection>;

struct Server {
    server_tx: ServerTx,
    server_rx: ServerRx,
    state_rx: StateRx,
    client_rx: ClientRx,
    connections: HashMap<SocketAddr, String>,
}

impl Server {
    /// create a new Server instance to handle connections
    /// create watch channel to communicate with connected clients
    fn create(server_rx: ServerRx, state_rx: StateRx) -> Self {
        let (server_tx, client_rx) = watch::channel("client".to_string());
        let connections = HashMap::new();
        Self {
            server_tx,
            server_rx,
            state_rx,
            client_rx,
            connections,
        }
    }

    async fn process(mut self) {
        //TODO: handle joins and quits
        while let Some(state) = self.state_rx.recv().await {
            let user = state.name.trim();
            println!("{:?}", state);
            // update connection state, publish join message to connected clients
            self.connections.insert(state.address, user.to_string());
            let msg = Message {
                msg: format!("{:?} joined \r\n", user),
            };
            self.server_tx.send(msg.msg);
        }
        while let Some(server_msg) = self.server_rx.recv().await {
            println!("server msg- {}", server_msg);
        }
    }
}
#[derive(Debug)]
struct Message {
    msg: String,
}

#[derive(Debug)]
struct Connection {
    address: SocketAddr,
    name: String,
}

impl Connection {
    async fn create(address: SocketAddr, name: String) -> Self {
        Self { address, name }
    }
}

impl Client {
    async fn create(
        address: SocketAddr,
        stream: TcpStream,
        client_rx: ClientRx,
        client_tx: ClientTx,
    ) -> Client {
        let (recv_half, wr_half) = stream.split();
        let client = Client {
            client_tx,
            client_rx,
            address,
            name: None,
            buf: BytesMut::with_capacity(4096),
            recv_half,
            wr_half,
        };
        return client;
    }

    async fn join(&mut self, state_tx: StateTx) -> io::Result<()> {
        self.wr_half.write(b"Name?").await?;
        self.recv_half.read_buf(&mut self.buf).await?;
        let buf = &self.buf.to_vec();
        let name = String::from_utf8_lossy(buf);
        self.name = Some(name.to_string());
        let connection = Connection::create(self.address, name.to_string()).await;
        state_tx.send(connection).unwrap_or_else(|e| {
            panic!("{}", e);
        });
        Ok(())
    }

    /// write messages received from the server to the client
    async fn write(&mut self, msg: String) -> io::Result<()> {
        {
            self.wr_half.write(&msg.into_bytes()).await?;
        }
        Ok(())
    }

    /// listen for messages on the client's write half
    async fn listen(wr_half: OwnedWriteHalf, buf: BytesMut) -> io::Result<String> {
        while let Ok(msg) = wr_half.try_write(&buf) {
            return Ok(msg.to_string());
        }
        panic!("disconnected")
    }

    /// listen for messages published from the server
    async fn recv(self, mut client_rx: ClientRx) -> io::Result<String> {
        while client_rx.changed().await.is_ok() {
            let recv = client_rx.borrow().to_string();
            drop(client_rx);
            return Ok(recv.to_string());
        }
        panic!("bad")
    }

    /// spawn handles to process client read/writes
    async fn process(&mut self) -> io::Result<()> {
        let op = Client::listen(self.wr_half, self.buf.clone());
        tokio::pin!(op);

        let wr_half = self.wr_half.borrow();
        std::mem::swap(&mut self.wr_half, wr_half.borrow_mut());
        tokio::select! {
            Ok(recv_msg) = self.recv(self.client_rx.clone()) => {
                self.write(recv_msg).await.unwrap();
            },
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 6969);
    let listener = TcpListener::bind(&addr).await.unwrap();

    let (client_tx, server_rx) = mpsc::unbounded_channel::<String>();
    let (state_tx, state_rx) = mpsc::unbounded_channel::<Connection>();

    let server = Server::create(server_rx, state_rx);
    let client_tx = client_tx.clone();
    let client_rx = server.client_rx.clone();

    tokio::spawn(async move {
        server.process().await;
    });

    // spawn handle to accept connections
    let accept_handle = tokio::spawn(async move {
        loop {
            let (stream, addr) = listener.accept().await.unwrap();

            let addr = addr.clone();
            let mut client =
                Client::create(addr, stream, client_rx.clone(), client_tx.clone()).await;

            // spawn client handle
            let state_tx = state_tx.clone();
            tokio::spawn(async move {
                client.join(state_tx).await.unwrap();
                client.process().await.unwrap();
            });
        }
    });

    accept_handle.await?;

    Ok(())
}
