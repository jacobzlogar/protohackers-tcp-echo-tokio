use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

use std::borrow::{BorrowMut};
use std::error::Error;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Debug)]
struct Message {
    user: String,
    msg: String,
}

/// struct that represents client connection
#[derive(Debug)]
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
type ClientTx = mpsc::UnboundedSender<Message>;
/// watch receiver channel for clients to receive new messages from the server.
type ClientRx = watch::Receiver<String>;

/// mpsc receiver channel for the server to receive messages from clients
type ServerRx = mpsc::UnboundedReceiver<Message>;
/// watch sender channel for the server to broadcast new messages to clients.
type ServerTx = watch::Sender<String>;

type StateTx = mpsc::UnboundedSender<Connection>;
type StateRx = mpsc::UnboundedReceiver<Connection>;

type ConnectionTx = mpsc::UnboundedSender<Connection>;
type ConnectionRx = mpsc::UnboundedReceiver<Connection>;

#[derive(Debug)]
struct Connection {
    addr: SocketAddr,
    name: String,
}

struct State {
    state_rx: StateRx,
    connection_tx: ConnectionTx,
    connections: Vec<Connection>,
}

impl State {
    fn create(state_rx: StateRx, connection_tx: ConnectionTx) -> Self {
        Self {
            state_rx,
            connection_tx,
            connections: vec![],
        }
    }
    async fn handle_state(&mut self) -> io::Result<String> {
        //TODO: handle joins and quits
        println!("handling state");
        while let Some(state) = self.state_rx.recv().await {
            println!("user: {:?}", state);
            self.connection_tx.send(state);
        }
        panic!("state handling failed");
    }
}

struct Server {
    server_tx: ServerTx,
    server_rx: ServerRx,
    client_rx: ClientRx,
    connection_rx: ConnectionRx,
}

impl Server {
    /// create a new Server instance to handle connections
    /// create watch channel to communicate with connected clients
    fn create(server_rx: ServerRx, connection_rx: ConnectionRx) -> Self {
        let (server_tx, client_rx) = watch::channel("client".to_string());
        Self {
            server_tx,
            server_rx,
            client_rx,
            connection_rx,
        }
    }

    async fn handle_messages(&mut self) -> io::Result<String> {
        println!("handling messages");
        while let Some(server_msg) = self.server_rx.recv().await {
            let msg = format!("{}: {}", server_msg.user.trim(), server_msg.msg);
            dbg!(&msg);
            self.server_tx.send(msg);
            //return Ok(server_msg);
        }
        panic!("message handling failed")
    }
}

impl Client {
    async fn create(
        address: SocketAddr,
        stream: TcpStream,
        client_rx: ClientRx,
        client_tx: ClientTx,
    ) -> Client {
        let (recv_half, wr_half) = stream.into_split();
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

    async fn listen(recv_half: &mut OwnedReadHalf, buf: &BytesMut) -> io::Result<String> {
        while let Ok(_) = recv_half.readable().await {
            let is_buf = &buf.is_empty();
            println!("{:?}", &buf);
            let wr_buf = &mut buf.to_vec();
            recv_half.read_buf(wr_buf).await?;
            let msg = String::from_utf8_lossy(wr_buf).to_string();
            println!("{}", msg);
            return Ok(msg);
        }
        panic!("disconnected")
    }

    async fn join(&mut self, state_tx: StateTx) -> io::Result<()> {
        self.wr_half.write(b"Name?").await?;
        self.recv_half.read_buf(&mut self.buf).await?;
        let buf = &self.buf.to_vec();
        self.buf.clear();
        let name = String::from_utf8_lossy(buf);
        self.name = Some(name.to_string());
        let connection = Connection {
            name: name.to_string(),
            addr: self.address,
        };
        state_tx.send(connection).unwrap_or_else(|e| {
            println!("state error {}", e);
        });
        Ok(())
    }

    /// write messages received from the server to the client
    async fn write_msg(wr_half: &mut OwnedWriteHalf, msg: String) -> io::Result<()> {
        wr_half.borrow_mut().write(&msg.into_bytes()).await?;
        Ok(())
    }

    /// listen for messages published from the server
    /// spawn handles to process client read/writes
    async fn process(mut self) -> io::Result<()> {
        let listen_handle = tokio::spawn(async move {
            while let Ok(recv) = Self::listen(self.recv_half.borrow_mut(), &self.buf).await {
                let msg = Message {
                    user: self.name.as_ref().unwrap().to_string(),
                    msg: recv,
                };
                self.client_tx.send(msg).unwrap_or_else(|e| {
                    println!("client error {}", e);
                });
                //drop(client_tx);
            }
        });
        let write_handle = tokio::spawn(async move {
            while let Ok(_) = self.client_rx.changed().await {
                let msg = self.client_rx.borrow().to_string();
                dbg!(&msg);
                Self::write_msg(self.wr_half.borrow_mut(), msg).await;
            }
        });
        write_handle.await?;
        listen_handle.await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let subscriber = tracing_subscriber::FmtSubscriber::new();
    tracing::subscriber::set_global_default(subscriber)?;

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 6969);
    let listener = TcpListener::bind(&addr).await.unwrap();
    let (state_tx, state_rx) = mpsc::unbounded_channel::<Connection>();
    let (connection_tx, connection_rx) = mpsc::unbounded_channel::<Connection>();
    let mut state = State::create(state_rx, connection_tx.clone());

    tokio::spawn(async move {
        println!("spawning state handle");
        state.handle_state().await;
    });

    let (client_tx, server_rx) = mpsc::unbounded_channel::<Message>();

    let mut server = Server::create(server_rx, connection_rx);
    let client_tx = client_tx.clone();
    let client_rx = server.client_rx.clone();

    tokio::spawn(async move {
        println!("spawning server handle");
        server.handle_messages().await;
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
            client.join(state_tx).await.unwrap();
            tokio::spawn(async move {
                client.process().await.unwrap();
            });
        }
    });

    accept_handle.await?;

    Ok(())
}
