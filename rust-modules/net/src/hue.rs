//! Optional loopback Hue lease transport. Its caller owns UI policy and failure notification.
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn request(command: &str, token: &str) -> std::io::Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 8976));
    let mut socket = TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    socket.set_read_timeout(Some(Duration::from_secs(8)))?;
    socket.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(socket, "POST /api/playback/{command} HTTP/1.0\r\nHost: 127.0.0.1:8976\r\nX-LG-Hue-Test: 1\r\nX-LG-Hue-Session: {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
    let mut response = Vec::new();
    socket.take(65536).read_to_end(&mut response)?;
    let first = response.split(|byte| *byte == b'\n').next().unwrap_or_default();
    if !first.starts_with(b"HTTP/1.0 200 ") && !first.starts_with(b"HTTP/1.1 200 ") {
        return Err(std::io::Error::other("local Hue lease refused"));
    }
    Ok(())
}

pub fn start(on_failure: impl FnOnce() + Send + 'static) -> Option<Sender<()>> {
    let (sender, receiver) = mpsc::channel();
    let token = format!("{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
    match std::thread::Builder::new().name("hue-lease".into()).spawn(move || {
        let result = request("start", &token).and_then(|()| {
            loop {
                match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                    Err(RecvTimeoutError::Timeout) => request("heartbeat", &token)?,
                }
            }
        });
        // Always release, even if the start timed out after the server accepted it.
        let _ = request("stop", &token);
        if result.is_err() {
            on_failure();
        }
    }) {
        Ok(_) => Some(sender),
        Err(_) => {
            plx_base::eventlog::log("Hue: worker could not start; playback continues");
            None
        }
    }
}
