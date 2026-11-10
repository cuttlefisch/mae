//! Framing and socket tuning for request/response traffic: no reply may be
//! held back by Nagle's algorithm waiting on the peer's delayed ACK.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Records every non-empty write.
#[derive(Default)]
struct Recorder(Vec<Vec<u8>>);

impl AsyncWrite for Recorder {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if !buf.is_empty() {
            self.0.push(buf.to_vec());
        }
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[tokio::test]
async fn a_frame_is_one_write() {
    let mut w = Recorder::default();
    crate::write_framed(&mut w, b"{\"jsonrpc\":\"2.0\"}", Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(w.0.len(), 1, "a frame went out in {} writes", w.0.len());
    assert_eq!(
        w.0[0],
        b"Content-Length: 17\r\n\r\n{\"jsonrpc\":\"2.0\"}".to_vec()
    );
}

#[tokio::test]
async fn tuning_a_request_stream_disables_nagle() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).await.unwrap();
    let (s, _) = l.accept().await.unwrap();
    assert!(!s.nodelay().unwrap(), "precondition: Nagle on by default");
    crate::tune_request_stream(&c);
    assert!(c.nodelay().unwrap());
    // The accept-site form the daemon uses.
    let (s, ()) = crate::tuned((s, ()));
    assert!(s.nodelay().unwrap());
}

/// Real sockets, Nagle LEFT ON at both ends, the peer waiting for each reply
/// before sending the next request: the pattern a polling client produces.
/// With the header and body in separate writes this measured ~42 ms per
/// request (the delayed-ACK timer); in one write, well under 1 ms. The bound
/// sits far below the 40 ms floor so load on a CI runner cannot fail it.
#[tokio::test]
async fn back_to_back_requests_are_not_held_by_nagle() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        let (s, _) = l.accept().await.unwrap();
        let (r, mut w) = s.into_split();
        let mut r = BufReader::new(r);
        let mut line = String::new();
        while r.read_line(&mut line).await.unwrap_or(0) > 0 {
            line.clear();
            crate::write_framed(&mut w, &[b'x'; 300], Duration::from_secs(5))
                .await
                .unwrap();
        }
    });
    let c = TcpStream::connect(addr).await.unwrap();
    let (r, mut w) = c.into_split();
    let mut r = BufReader::new(r);
    let mut times = Vec::new();
    for _ in 0..20 {
        let t0 = Instant::now();
        w.write_all(b"req\n").await.unwrap();
        let (mut hdr, mut blank) = (String::new(), String::new());
        r.read_line(&mut hdr).await.unwrap();
        r.read_line(&mut blank).await.unwrap();
        let n: usize = hdr
            .trim()
            .trim_start_matches("Content-Length: ")
            .parse()
            .unwrap();
        let mut body = vec![0; n];
        r.read_exact(&mut body).await.unwrap();
        times.push(t0.elapsed());
    }
    times.sort();
    let median = times[times.len() / 2];
    assert!(
        median < Duration::from_millis(20),
        "median back-to-back request took {median:?}"
    );
}
