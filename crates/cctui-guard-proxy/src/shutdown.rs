//! The proxy runs as PID 1, which ignores any signal it has no handler for:
//! without these handlers the kubelet SIGKILLs it at the end of the grace period.

use std::future::Future;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinSet;

/// Handlers are installed on call, so a signal during startup is not lost.
pub fn termination() -> anyhow::Result<impl Future<Output = ()> + Send + 'static> {
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM received"),
            _ = int.recv() => tracing::info!("SIGINT received"),
        }
    })
}

pub async fn serve_until<S, F, Fut>(
    listener: TcpListener,
    shutdown: S,
    drain: Duration,
    mut handle: F,
) -> std::io::Result<()>
where
    S: Future<Output = ()>,
    F: FnMut(TcpStream) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut conns = JoinSet::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
            accepted = listener.accept() => {
                let (stream, _peer) = accepted?;
                conns.spawn(handle(stream));
            }
        }
    }
    drop(listener);

    tracing::info!("shutting down: draining {} connection(s) for up to {drain:?}", conns.len());
    let drained =
        tokio::time::timeout(drain, async { while conns.join_next().await.is_some() {} }).await;
    if drained.is_err() {
        tracing::warn!("drain deadline reached: aborting {} connection(s)", conns.len());
        conns.shutdown().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::oneshot;
    use tokio::time::Instant;

    async fn echo(mut s: TcpStream) {
        let mut buf = [0u8; 64];
        while let Ok(n) = s.read(&mut buf).await {
            if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    }

    async fn start(
        drain: Duration,
    ) -> (std::net::SocketAddr, oneshot::Sender<()>, tokio::task::JoinHandle<std::io::Result<()>>)
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = oneshot::channel::<()>();
        let server = tokio::spawn(serve_until(
            listener,
            async move {
                let _ = rx.await;
            },
            drain,
            echo,
        ));
        (addr, tx, server)
    }

    async fn roundtrip(conn: &mut TcpStream) {
        conn.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn shutdown_waits_for_in_flight_connection_then_returns_ok() {
        let (addr, tx, server) = start(Duration::from_secs(30)).await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        roundtrip(&mut conn).await;

        tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!server.is_finished(), "returned while a connection was in flight");
        assert!(TcpStream::connect(addr).await.is_err(), "still accepting after shutdown");
        roundtrip(&mut conn).await;

        drop(conn);
        let res = tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn drain_is_bounded_by_the_deadline() {
        let (addr, tx, server) = start(Duration::from_millis(200)).await;
        let mut conn = TcpStream::connect(addr).await.unwrap();
        roundtrip(&mut conn).await;

        let started = Instant::now();
        tx.send(()).unwrap();
        let res = tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
        assert!(res.is_ok());
        assert!(started.elapsed() >= Duration::from_millis(200));
        let mut buf = [0u8; 1];
        assert_eq!(conn.read(&mut buf).await.unwrap_or(0), 0, "straggler not aborted");
    }
}
