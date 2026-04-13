use ak::Addr;
use crate::actors::world::WorldActor;
use crate::messages::world_msgs::{PlayerConnect, PlayerDisconnect, PlayerCommand};

use tokio_net::tcp::TcpStream;
use tokio_io::{AsyncReadExt, AsyncWriteExt};

const BANNER: &str = "\
\r\n\
============================================\r\n\
     Welcome to Factorio Memo RPG!\r\n\
============================================\r\n\
A persistent-world RPG with Factorio-style\r\n\
factory building, resource gathering, and\r\n\
crafting. Your adventure is recorded in\r\n\
your personal Memo Journal.\r\n\
\r\n\
To begin, type: connect <your_name>\r\n\
============================================\r\n\
> ";

/// Handle a single TCP connection (one player session).
/// This is a plain async function, not an actor - it bridges TCP I/O to actor messages.
pub async fn handle_connection(mut stream: TcpStream, world: Addr<WorldActor>, peer: String) {
    // Send banner
    if stream.write_all(BANNER.as_bytes()).await.is_err() {
        return;
    }

    let mut player_name: Option<String> = None;
    let mut line_buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];

    loop {
        // Read one byte at a time until newline (simple line protocol)
        match stream.read(&mut byte).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                if byte[0] == b'\n' {
                    let line = String::from_utf8_lossy(&line_buf).trim().to_string();
                    line_buf.clear();

                    if line.is_empty() {
                        let _ = stream.write_all(b"> ").await;
                        continue;
                    }

                    // Parse the command
                    let parts: Vec<&str> = line.splitn(2, char::is_whitespace).collect();
                    let cmd = parts[0].to_lowercase();

                    if cmd == "connect" && player_name.is_none() {
                        let name = parts.get(1).map(|s| s.trim().to_string()).unwrap_or_default();
                        if name.is_empty() {
                            let _ = stream.write_all(b"Usage: connect <player_name>\r\n> ").await;
                            continue;
                        }

                        match world.send(PlayerConnect { name: name.clone() }).await {
                            Ok(Ok(welcome)) => {
                                player_name = Some(name);
                                let response = format!("{}\r\n> ", welcome);
                                let _ = stream.write_all(response.as_bytes()).await;
                            }
                            Ok(Err(e)) => {
                                let response = format!("ERR: {}\r\n> ", e);
                                let _ = stream.write_all(response.as_bytes()).await;
                            }
                            Err(_) => {
                                let _ = stream.write_all(b"ERR: Server error\r\n> ").await;
                            }
                        }
                    } else if cmd == "quit" || cmd == "exit" || cmd == "q" {
                        let _ = stream.write_all(b"Farewell, adventurer!\r\n").await;
                        break;
                    } else if let Some(ref name) = player_name {
                        // Dispatch command to world actor
                        match world.send(PlayerCommand {
                            player_name: name.clone(),
                            command: line.clone(),
                        }).await {
                            Ok(response) => {
                                if response == "QUIT" {
                                    let _ = stream.write_all(b"Farewell, adventurer!\r\n").await;
                                    break;
                                }
                                // Convert \n to \r\n for telnet
                                let telnet_response = response.replace('\n', "\r\n");
                                let output = format!("{}\r\n> ", telnet_response);
                                let _ = stream.write_all(output.as_bytes()).await;
                            }
                            Err(_) => {
                                let _ = stream.write_all(b"ERR: Server error\r\n> ").await;
                            }
                        }
                    } else {
                        let _ = stream.write_all(b"Please connect first: connect <player_name>\r\n> ").await;
                    }
                } else if byte[0] != b'\r' {
                    line_buf.push(byte[0]);
                    // Prevent buffer overflow
                    if line_buf.len() > 1024 {
                        line_buf.clear();
                        let _ = stream.write_all(b"\r\nERR: Line too long\r\n> ").await;
                    }
                }
            }
            Err(_) => break,
        }
    }

    // Cleanup: disconnect player
    if let Some(name) = player_name {
        let _ = world.send(PlayerDisconnect { name: name.clone() }).await;
        println!("[Server] {} disconnected ({})", name, peer);
    } else {
        println!("[Server] Anonymous connection closed ({})", peer);
    }
}
