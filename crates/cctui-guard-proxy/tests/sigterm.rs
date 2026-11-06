use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Proxy {
    child: Child,
    addr: String,
    _dir: tempfile::TempDir,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_addr() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().to_string()
}

fn start_echo() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            thread::spawn(move || {
                let mut buf = [0u8; 256];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 || s.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

fn start_proxy(allowed: &str, drain_secs: u64) -> Proxy {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("policy.json");
    std::fs::write(&policy, format!(r#"{{"allowed_hosts": ["{allowed}"], "default": "deny"}}"#))
        .unwrap();
    let addr = free_addr();
    let child = Command::new(env!("CARGO_BIN_EXE_cctui-guard-proxy"))
        .args(["--mode", "forward", "--listen", &addr, "--health-listen", &free_addr()])
        .arg("--policy")
        .arg(&policy)
        .arg("--inject-config")
        .arg(dir.path().join("absent.json"))
        .arg("--private-allow-file")
        .arg(dir.path().join("absent-private-allow"))
        .args(["--shutdown-drain-secs", &drain_secs.to_string()])
        .env("GUARD_PROXY_PRIVATE_ALLOW", allowed)
        .env_remove("GUARD_PROXY_ALLOW_PRIVATE_IPS")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(&addr).is_err() {
        assert!(Instant::now() < deadline, "proxy never started listening on {addr}");
        thread::sleep(Duration::from_millis(50));
    }
    Proxy { child, addr, _dir: dir }
}

fn sigterm(child: &Child) {
    let ok = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(ok.success());
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "proxy did not exit within {within:?}");
        thread::sleep(Duration::from_millis(25));
    }
}

fn open_tunnel(proxy: &str, target: &str) -> TcpStream {
    let mut conn = TcpStream::connect(proxy).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    conn.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .unwrap();
    let mut buf = [0u8; 128];
    let n = conn.read(&mut buf).unwrap();
    let resp = String::from_utf8_lossy(&buf[..n]);
    assert!(resp.starts_with("HTTP/1.1 200"), "CONNECT refused: {resp}");
    conn
}

fn roundtrip(conn: &mut TcpStream) {
    conn.write_all(b"ping").unwrap();
    let mut buf = [0u8; 4];
    conn.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"ping");
}

#[test]
fn sigterm_drains_an_in_flight_tunnel_then_exits_zero() {
    let echo = start_echo();
    let mut proxy = start_proxy(&echo, 30);
    let mut conn = open_tunnel(&proxy.addr, &echo);
    roundtrip(&mut conn);

    sigterm(&proxy.child);
    thread::sleep(Duration::from_millis(300));
    assert!(proxy.child.try_wait().unwrap().is_none(), "exited with a tunnel in flight");
    roundtrip(&mut conn);
    assert!(TcpStream::connect(&proxy.addr).is_err(), "still accepting after SIGTERM");

    drop(conn);
    let status = wait_exit(&mut proxy.child, Duration::from_secs(5));
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn sigterm_exits_zero_at_the_drain_deadline() {
    let echo = start_echo();
    let mut proxy = start_proxy(&echo, 1);
    let mut conn = open_tunnel(&proxy.addr, &echo);
    roundtrip(&mut conn);

    sigterm(&proxy.child);
    let status = wait_exit(&mut proxy.child, Duration::from_secs(5));
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn sigterm_when_idle_exits_zero_promptly() {
    let mut proxy = start_proxy("example.com:443", 30);
    sigterm(&proxy.child);
    let status = wait_exit(&mut proxy.child, Duration::from_secs(3));
    assert_eq!(status.code(), Some(0), "{status:?}");
}
