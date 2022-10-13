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
struct Client<'a> {
    // client -> server
    client_tx: ClientTx,
    // server -> client
    client_rx: ClientRx,

    address: SocketAddr,
    name: Option<String>,

    buf: BytesMut,

    recv_half: ReadHalf<'a>,
    wr_half: WriteHalf<'a>,
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

impl<'a> Client<'a> {
    async fn create(
        address: SocketAddr,
        mut stream: TcpStream,
        client_rx: ClientRx,
        client_tx: ClientTx,
    ) -> Client<'a> {
        let (mut recv_half, mut wr_half) = stream.split();
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

    async fn write(mut wr_half: OwnedWriteHalf, msg: String) -> io::Result<()> {
        {
            wr_half.write(&msg.into_bytes()).await?;
        }
        Ok(())
    }

    async fn listen(&mut self) -> io::Result<String> {
        while let Ok(msg) = self.wr_half.try_write(&self.buf) {
            return Ok(msg.to_string());
        }
        panic!("disconnected")
        //self.client_tx.send(dst.to_string());
    }

    /// listen for messages published from the server
    async fn recv(&mut self, mut client_rx: ClientRx) -> io::Result<String> {
        while client_rx.changed().await.is_ok() {
            let recv = self.client_rx.borrow().to_string();
            return Ok(recv.to_string());
        }
        panic!("bad")
    }

    /// spawn handles to process client read/writes
    async fn process(&mut self) -> io::Result<()> {
        let client_rx = self.client_rx.clone();
        let operation = self.recv(client_rx);
        tokio::pin!(operation);

        loop {
            tokio::select! {
                msg = &mut operation => { Client::write(self.wr_half, msg.unwrap()); },
                Ok(wr_msg) = self.listen() => { println!("{:?}", wr_msg) },
            }
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
