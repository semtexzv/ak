use ak::Addr;
use crate::actors::world::WorldActor;
use crate::net::session::handle_connection;

use tokio_net::tcp::TcpListener;
use futures::StreamExt;

/// Start the TCP server that accepts player connections.
/// Each connection gets its own async session handler.
pub async fn run_server(world: Addr<WorldActor>, bind_addr: &str) {
    let addr: std::net::SocketAddr = bind_addr.parse()
        .expect("Invalid bind address");

    let mut listener = TcpListener::bind(&addr).await
        .expect("Failed to bind TCP listener");

    println!("=========================================");
    println!("  Factorio Memo RPG Server");
    println!("  Listening on {}", bind_addr);
    println!("  Connect with: telnet {} {}", addr.ip(), addr.port());
    println!("=========================================");
    println!();

    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        match stream {
            Ok(stream) => {
                let peer = stream.peer_addr()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| "unknown".into());
                println!("[Server] New connection from {}", peer);
                let world = world.clone();
                ak::rt::spawn(handle_connection(stream, world, peer));
            }
            Err(e) => {
                eprintln!("[Server] Accept error: {}", e);
            }
        }
    }
}
