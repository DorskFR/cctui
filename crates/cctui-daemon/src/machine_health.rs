//! Probes the macOS privacy grants that a binary change silently drops.
//!
//! macOS ties folder and Local Network grants to a binary's code signature:
//! a harness auto-update or an ad-hoc-signed self-update drops them and the
//! next worker is denied with no output cctui can see, so the daemon probes
//! the grants itself. The probe runs in this process, whose TCC attribution a
//! forked child inherits; the `claude` binary's own grants are only exercised
//! by a real spawn, which the spawn watchdog classifies.

use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cctui_proto::harness::MachineHealth;
use tokio_util::sync::CancellationToken;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const PERIOD: Duration = Duration::from_mins(10);

static LAST: Mutex<Option<MachineHealth>> = Mutex::new(None);
static SERVER_URL: OnceLock<String> = OnceLock::new();

/// One folder listing or one connect attempt, as the classifier sees it.
pub type Observation<T> = (T, io::Result<()>);

/// The last verdict, for the heartbeat. `None` before the first probe.
#[must_use]
pub fn last() -> Option<MachineHealth> {
    LAST.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Why a spawn on this machine must not start, or `None`.
#[must_use]
pub fn spawn_refusal() -> Option<String> {
    last()?.refusal("this machine")
}

/// Only an explicit refusal is a denial.
///
/// A folder listing refused with `EPERM`/`EACCES` is a dropped grant; a
/// connect refused with `EHOSTUNREACH`/`ENETUNREACH` is a Local Network
/// denial. Anything else (a missing folder, a closed port, a timeout) says
/// nothing about grants and is healthy here.
#[must_use]
pub fn classify(
    folders: &[Observation<PathBuf>],
    network: Option<&Observation<SocketAddr>>,
) -> MachineHealth {
    if let Some((addr, Err(err))) = network
        && matches!(err.kind(), io::ErrorKind::HostUnreachable | io::ErrorKind::NetworkUnreachable)
    {
        return MachineHealth::NetworkDenied { detail: format!("connect to {addr}: {err}") };
    }
    let denied = folders.iter().find_map(|(path, res)| match res {
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            Some(format!("list {}: {err}", path.display()))
        }
        _ => None,
    });
    denied.map_or(MachineHealth::Ok, |detail| MachineHealth::FolderDenied { detail })
}

/// The folders a probe lists. macOS: the TCC-gated user folders that exist.
/// Elsewhere there is no grant to lose, so only the daemon's cwd is checked.
#[must_use]
pub fn probe_folders() -> Vec<PathBuf> {
    if !cfg!(target_os = "macos") {
        return std::env::current_dir().into_iter().collect();
    }
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    ["Desktop", "Documents", "Downloads"]
        .iter()
        .map(|d| home.join(d))
        .filter(|p| p.is_dir())
        .collect()
}

fn list(path: &PathBuf) -> io::Result<()> {
    std::fs::read_dir(path).map(|_| ())
}

async fn resolve(server_url: &str) -> Option<SocketAddr> {
    let url = reqwest::Url::parse(server_url).ok()?;
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_owned();
    let port = url.port_or_known_default()?;
    tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host.as_str(), port)))
        .await
        .ok()?
        .ok()?
        .next()
}

async fn connect(addr: SocketAddr) -> io::Result<()> {
    tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await.map_or_else(
        |_| Err(io::Error::new(io::ErrorKind::TimedOut, "connect timed out")),
        |res| res.map(|_| ()),
    )
}

/// macOS only: elsewhere an unreachable host is a network fault, not a grant.
async fn probe_network(server_url: Option<&str>) -> Option<Observation<SocketAddr>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let addr = resolve(server_url?).await?;
    Some((addr, connect(addr).await))
}

/// Record and return the verdict; logs only when it changes.
pub async fn probe(folders: Vec<PathBuf>, server_url: Option<&str>) -> MachineHealth {
    let listed = tokio::task::spawn_blocking(move || {
        folders
            .into_iter()
            .map(|p| {
                let res = list(&p);
                (p, res)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let network = probe_network(server_url).await;
    let verdict = classify(&listed, network.as_ref());
    let previous = LAST.lock().unwrap_or_else(PoisonError::into_inner).replace(verdict.clone());
    if previous.as_ref() != Some(&verdict) {
        match &verdict {
            MachineHealth::Ok => tracing::info!("machine health probe ok"),
            other => {
                tracing::warn!(detail = ?other.detail(), "machine health probe found a denial");
            }
        }
        crate::harness_update::bump_revision();
    }
    verdict
}

/// Re-run the probe with the daemon's configured server.
pub async fn refresh() -> MachineHealth {
    probe(probe_folders(), SERVER_URL.get().map(String::as_str)).await
}

/// Probe now, then every `PERIOD`.
pub fn spawn_loop(shutdown: CancellationToken, server_url: String) {
    let _ = SERVER_URL.set(server_url);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PERIOD);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            refresh().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(kind: io::ErrorKind) -> io::Result<()> {
        Err(io::Error::from(kind))
    }

    fn addr() -> SocketAddr {
        "192.168.1.10:8080".parse().unwrap()
    }

    #[test]
    fn only_a_permission_error_on_a_folder_is_a_dropped_grant() {
        let docs = PathBuf::from("/Users/me/Documents");
        let denied = classify(&[(docs.clone(), err(io::ErrorKind::PermissionDenied))], None);
        match denied {
            MachineHealth::FolderDenied { detail } => {
                assert!(detail.starts_with("list /Users/me/Documents:"), "{detail}");
            }
            other => panic!("expected FolderDenied, got {other:?}"),
        }
        assert!(classify(&[(docs.clone(), err(io::ErrorKind::NotFound))], None).is_ok());
        assert!(classify(&[(docs, Ok(()))], None).is_ok());
        assert!(classify(&[], None).is_ok());
    }

    #[test]
    fn only_an_unreachable_host_is_a_local_network_denial() {
        for kind in [io::ErrorKind::HostUnreachable, io::ErrorKind::NetworkUnreachable] {
            match classify(&[], Some(&(addr(), err(kind)))) {
                MachineHealth::NetworkDenied { detail } => {
                    assert!(detail.starts_with("connect to 192.168.1.10:8080:"), "{detail}");
                }
                other => panic!("expected NetworkDenied, got {other:?}"),
            }
        }
        for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut] {
            assert!(classify(&[], Some(&(addr(), err(kind)))).is_ok(), "{kind:?}");
        }
        assert!(classify(&[], Some(&(addr(), Ok(())))).is_ok());
    }

    /// The network denial names the capability every worker needs first.
    #[test]
    fn a_network_denial_outranks_a_folder_denial() {
        let both = classify(
            &[(PathBuf::from("/x"), err(io::ErrorKind::PermissionDenied))],
            Some(&(addr(), err(io::ErrorKind::HostUnreachable))),
        );
        assert!(matches!(both, MachineHealth::NetworkDenied { .. }), "{both:?}");
    }

    /// The one test touching the shared verdict: healthy refuses nothing, a
    /// denial refuses with the fix.
    #[tokio::test]
    async fn the_recorded_verdict_drives_the_spawn_refusal() {
        let dir = tempfile::tempdir().expect("tempdir");
        let verdict = probe(vec![dir.path().to_path_buf()], None).await;
        assert!(verdict.is_ok());
        assert_eq!(last(), Some(MachineHealth::Ok));
        assert!(spawn_refusal().is_none());

        let denied = MachineHealth::NetworkDenied { detail: "connect to x: EHOSTUNREACH".into() };
        *LAST.lock().unwrap() = Some(denied);
        let refusal = spawn_refusal().expect("refused");
        assert!(refusal.contains("cctui-daemon doctor --grant"), "{refusal}");
        assert!(refusal.contains("EHOSTUNREACH"), "{refusal}");
        *LAST.lock().unwrap() = None;
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn off_macos_the_network_is_not_probed() {
        assert!(probe_network(Some("http://192.0.2.1:9")).await.is_none());
    }

    #[test]
    fn probe_folders_are_directories_that_exist() {
        for p in probe_folders() {
            assert!(p.is_dir(), "{}", p.display());
        }
    }
}
