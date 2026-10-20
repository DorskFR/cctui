//! Scripted HTTP server for the client tests: answers from a routing function
//! and records every request it was sent.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub body: String,
}

pub type Log = Arc<Mutex<Vec<Seen>>>;

pub struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Reply {
    pub fn json(body: &str) -> Self {
        Self { status: 200, content_type: "application/json", body: body.to_owned() }
    }

    pub fn html() -> Self {
        Self {
            status: 200,
            content_type: "text/html",
            body: "<!doctype html><html></html>".to_owned(),
        }
    }

    pub const fn status(status: u16) -> Self {
        Self { status, content_type: "text/plain", body: String::new() }
    }
}

pub async fn serve(route: fn(&str, &str) -> Reply) -> (String, Log) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Log::default();
    let seen = Arc::clone(&log);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move { answer(stream, route, &seen).await });
        }
    });
    (base, log)
}

async fn answer(stream: tokio::net::TcpStream, route: fn(&str, &str) -> Reply, seen: &Log) {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body).await;
    let mut words = request_line.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let path = words.next().unwrap_or_default().to_owned();
    let reply = route(&method, &path);
    seen.lock().unwrap().push(Seen { method, path, body: String::from_utf8_lossy(&body).into() });
    let length = if reply.status == 204 {
        String::new()
    } else {
        format!("content-length: {}\r\n", reply.body.len())
    };
    let head = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\n{length}connection: close\r\n\r\n",
        reply.status, reply.content_type
    );
    let _ = write.write_all(head.as_bytes()).await;
    let _ = write.write_all(reply.body.as_bytes()).await;
}
