//! Serving the connections of a listener: what axum's `serve` does, with a header read timeout.
//!
//! axum gives hyper no timer, so hyper's header read timeout never applies: a client that
//! connects and sends nothing, or keeps a connection open between two requests, would hold it
//! forever. Here a request's headers must arrive within the timeout, which also closes a
//! keep-alive connection idle that long, since hyper is then waiting for the next headers.
//! HTTP/1.1 only: devices and reverse proxies speak it, and hyper's version detection would wait
//! for a first byte without any timeout.

use std::{future::Future, io, net::SocketAddr, time::Duration};

use axum::{Router, extract::ConnectInfo, serve::Listener};
use hyper::{body::Incoming, server::conn::http1, service::service_fn};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::graceful::GracefulShutdown,
};
use tower::ServiceExt as _;

/// How long a request's headers may take to arrive, counted from the connection or the end of
/// the previous answer: devices and proxies open a new connection when theirs was closed.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Serves `router` on `listener` until `shutdown` resolves, then lets requests in flight finish
/// and closes idle connections. Routes see the peer as `ConnectInfo<SocketAddr>`.
pub async fn serve<L>(
    mut listener: L,
    router: Router,
    header_read_timeout: Duration,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()>
where
    L: Listener<Addr = SocketAddr>,
{
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(header_read_timeout);
    let graceful = GracefulShutdown::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        let (io, peer) = tokio::select! {
            accepted = listener.accept() => accepted,
            () = &mut shutdown => break,
        };
        let router = router.clone();
        let service = service_fn(move |mut request: hyper::Request<Incoming>| {
            request.extensions_mut().insert(ConnectInfo(peer));
            router.clone().oneshot(request)
        });
        let connection = graceful.watch(builder.serve_connection(TokioIo::new(io), service));
        tokio::spawn(async move {
            // Timeouts and clients that go away end up here: nothing to act on.
            if let Err(error) = connection.await {
                tracing::debug!(%error, %peer, "connection ended");
            }
        });
    }
    drop(listener);
    graceful.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use axum::routing::get;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use super::*;

    async fn listening(
        timeout: Duration,
    ) -> (
        SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<io::Result<()>>,
    ) {
        let tcp = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = tcp.local_addr().unwrap();
        let router = Router::new().route(
            "/",
            get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                format!("peer {}", peer.ip())
            }),
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(serve(tcp, router, timeout, async {
            let _ = stopped.await;
        }));
        (address, stop, server)
    }

    /// Everything the server sends until it closes the connection, or `None` if it is still
    /// open after 5 seconds.
    async fn until_closed(stream: &mut tokio::net::TcpStream) -> Option<String> {
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .ok()?
            .ok()?;
        Some(String::from_utf8_lossy(&answer).into_owned())
    }

    #[tokio::test]
    async fn a_silent_connection_is_closed() {
        let (address, _stop, _server) = listening(Duration::from_millis(300)).await;
        let mut silent = tokio::net::TcpStream::connect(address).await.unwrap();
        assert!(until_closed(&mut silent).await.is_some());
        // Headers that never end alike.
        let mut partial = tokio::net::TcpStream::connect(address).await.unwrap();
        partial.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        assert!(until_closed(&mut partial).await.is_some());
    }

    #[tokio::test]
    async fn an_idle_keep_alive_connection_is_closed() {
        let (address, _stop, _server) = listening(Duration::from_millis(300)).await;
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        // The answer, then nothing: the connection stays open for a next request until the
        // timeout, and the routes saw the peer.
        let answer = until_closed(&mut stream).await.expect("closed once idle");
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.ends_with("peer 127.0.0.1"), "{answer}");
    }

    #[tokio::test]
    async fn stopping_closes_idle_connections_and_returns() {
        let (address, stop, server) = listening(Duration::from_secs(60)).await;
        let mut idle = tokio::net::TcpStream::connect(address).await.unwrap();
        idle.write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut start = [0; 12];
        idle.read_exact(&mut start).await.unwrap();
        let _ = stop.send(());
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("the server stops")
            .unwrap()
            .unwrap();
        assert!(until_closed(&mut idle).await.is_some());
    }
}
