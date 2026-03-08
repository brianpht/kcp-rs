// examples/echo.rs
use kcp_rs::{Kcp, KcpConfig, KcpOutput, KcpResult, KcpError};
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Instant;

struct UdpOutput {
    socket: Arc<UdpSocket>,
    target: std::net::SocketAddr,
}

impl KcpOutput for UdpOutput {
    fn output(&mut self, data: &[u8]) -> KcpResult<usize> {
        self.socket
            .send_to(data, self.target)
            .map_err(|_| KcpError::BufferFull)
    }
}

fn main() -> std::io::Result<()> {
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:12345")?);
    socket.set_nonblocking(true)?;

    let target = "127.0.0.1:12346".parse().unwrap();
    let output = UdpOutput {
        socket: socket.clone(),
        target,
    };

    let mut kcp = Kcp::with_config(1, output, KcpConfig::fast());

    // Send data
    kcp.send(b"Hello KCP!").expect("send failed");

    let start = Instant::now();
    let mut buf = [0u8; 2048];
    let mut recv_buf = [0u8; 4096];

    loop {
        let current = start.elapsed().as_millis() as u32;

        // Receive from UDP
        match socket.recv_from(&mut buf) {
            Ok((size, _)) => {
                if kcp.input(&buf[..size]).is_err() {
                    eprintln!("input error");
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }

        // Update KCP
        if let Err(e) = kcp.update(current) {
            eprintln!("update error: {:?}", e);
            break;
        }

        // Receive from KCP
        match kcp.recv(&mut recv_buf) {
            Ok(size) => {
                println!("Received: {}", String::from_utf8_lossy(&recv_buf[..size]));
            }
            Err(KcpError::WouldBlock) => {}
            Err(e) => {
                eprintln!("recv error: {:?}", e);
                break;
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    Ok(())
}