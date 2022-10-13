// async fn spawn_client_handle(
//     mut read_half: ReadHalf<TcpStream>,
//     s_tx: tokio::sync::mpsc::Sender<String>,
//     mut watch_rx: tokio::sync::watch::Receiver<HashMap<String, String>>,
// ) {
//     let mut watch_rx = watch_rx.clone();
//     let read_handle = tokio::spawn(async move {
//         let mut buf = vec![0; 4096];
//         loop {
//             let n = read_half.read(&mut buf).await.unwrap();
//             // println!("{:?}", n);

//             if n == 0 {
//                 println!("socket disconnected");
//                 break;
//             }
//             let b = buf.to_vec();
//             let text = String::from_utf8_lossy(&b[..n].to_vec()).to_string();
//             s_tx.send(text).await;
//         }
//     });

//     let write_handle = tokio::spawn(async move {
//         loop {
//             while watch_rx.changed().await.is_ok() {
//                 let borrow = watch_rx.borrow();
//                 println!("{:?}", borrow);
//             }
//         }
//     });

//     write_handle.await.unwrap();
//     read_handle.await.unwrap();
// }

// async fn spawn_write_handle(
//     mut write_half: WriteHalf,
//     mut watch_rx: tokio::sync::watch::Receiver<HashMap<String, String>>,
// ) {
//     println!("spawning write handle");
//     tokio::spawn(async move {
//         loop {
//             while watch_rx.changed().await.is_ok() {
//                 dbg!(watch_rx.clone());
//             }
//         }
//     });
// }
