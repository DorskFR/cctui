//! Boot-time upstream calls. Kubernetes does not promise a working pod network
//! when the first container starts, so the first call waits for the upstream
//! to accept a TCP connection, and secret fetches retry transient failures.

use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::Instant;

use crate::secrets::{Credential, RefResolver, SecretError, SecretRef};

pub const READY_DEADLINE: Duration = Duration::from_secs(30);
const READY_POLL: Duration = Duration::from_millis(200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const FETCH_ATTEMPTS: u32 = 5;

pub enum Readiness {
    Ready(Duration),
    TimedOut { waited: Duration, last: String },
}

pub async fn wait_reachable(host: &str, port: u16, deadline: Duration) -> Readiness {
    let start = Instant::now();
    loop {
        let last =
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port))).await {
                Ok(Ok(_)) => return Readiness::Ready(start.elapsed()),
                Ok(Err(e)) => e.to_string(),
                Err(_) => format!("connect timed out after {CONNECT_TIMEOUT:?}"),
            };
        if start.elapsed() + READY_POLL >= deadline {
            return Readiness::TimedOut { waited: start.elapsed(), last };
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

pub async fn resolve_with_retry(
    resolver: &dyn RefResolver,
    r: &SecretRef,
    on_retry: impl Fn(u32, &SecretError),
) -> Result<Credential, SecretError> {
    let mut attempt = 0;
    loop {
        match resolver.resolve(r).await {
            Err(e) if e.is_transient() && attempt + 1 < FETCH_ATTEMPTS => {
                on_retry(attempt, &e);
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                attempt += 1;
            }
            res => return res,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[tokio::test]
    async fn a_listening_upstream_is_ready_at_once() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let Readiness::Ready(waited) = wait_reachable("127.0.0.1", port, READY_DEADLINE).await
        else {
            panic!("must be ready");
        };
        assert!(waited < READY_POLL, "{waited:?}");
    }

    #[tokio::test]
    async fn waits_through_refused_connections_then_proceeds() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(700)).await;
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
            loop {
                let _ = listener.accept().await;
            }
        });
        let Readiness::Ready(waited) = wait_reachable("127.0.0.1", port, READY_DEADLINE).await
        else {
            panic!("must become ready");
        };
        assert!(waited >= Duration::from_millis(600), "{waited:?}");
        late.abort();
    }

    #[tokio::test]
    async fn gives_up_at_the_deadline_with_the_last_error() {
        let Readiness::TimedOut { waited, last } =
            wait_reachable("127.0.0.1", 9, Duration::from_millis(500)).await
        else {
            panic!("port 9 must stay unreachable");
        };
        assert!(waited < Duration::from_secs(2), "{waited:?}");
        assert!(!last.is_empty());
    }

    struct Scripted(Mutex<Vec<Result<Credential, SecretError>>>);

    #[async_trait::async_trait]
    impl RefResolver for Scripted {
        async fn resolve(&self, _: &SecretRef) -> Result<Credential, SecretError> {
            self.0.lock().unwrap().remove(0)
        }
    }

    fn gpg_ref() -> SecretRef {
        SecretRef::parse("vault:kv/data/w#GPG_KEY").unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_retries_a_transient_failure() {
        let resolver = Scripted(Mutex::new(vec![
            Err(SecretError::transient("vault login request: error sending request")),
            Err(SecretError::transient("vault read failed: HTTP 503")),
            Ok(Credential::new("armored-key".to_owned())),
        ]));
        let retries = Mutex::new(0);
        let cred = resolve_with_retry(&resolver, &gpg_ref(), |_, _| *retries.lock().unwrap() += 1)
            .await
            .unwrap();
        assert_eq!(cred.expose(), "armored-key");
        assert_eq!(*retries.lock().unwrap(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_stops_on_a_permanent_failure() {
        let resolver = Scripted(Mutex::new(vec![
            Err(SecretError::Backend(anyhow::anyhow!("vault login failed: HTTP 403"))),
            Ok(Credential::new("never".to_owned())),
        ]));
        let err = resolve_with_retry(&resolver, &gpg_ref(), |_, e| panic!("retried {e}"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("403"), "{err}");

        let resolver = Scripted(Mutex::new(vec![
            Err(SecretError::NotFound { what: "GPG_KEY".to_owned() }),
            Ok(Credential::new("never".to_owned())),
        ]));
        let err = resolve_with_retry(&resolver, &gpg_ref(), |_, e| panic!("retried {e}"))
            .await
            .unwrap_err();
        assert!(matches!(err, SecretError::NotFound { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn fetch_gives_up_after_bounded_transient_retries() {
        let resolver = Scripted(Mutex::new(
            (0..FETCH_ATTEMPTS).map(|_| Err(SecretError::transient("down"))).collect(),
        ));
        let retries = Mutex::new(0);
        let err = resolve_with_retry(&resolver, &gpg_ref(), |_, _| *retries.lock().unwrap() += 1)
            .await
            .unwrap_err();
        assert!(err.is_transient());
        assert_eq!(*retries.lock().unwrap(), FETCH_ATTEMPTS - 1);
    }
}
