//! A scripted HTTP server for the answers the real router never gives: a `503`, a body with an escape sequence in it, a form with a
//! member of a later release, a server that never answers. One thread per connection, `Connection: close`, nothing it does is
//! checked beyond the request line and the two headers a test asks about.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What the stub does with a request.
pub enum Answer {
    /// Answer with this status, these headers and this body.
    Say(u16, Vec<(String, String)>, String),
    /// Accept the request and never answer.
    Hang,
}

/// One request the stub saw.
#[derive(Clone, Debug)]
pub struct Seen {
    /// `GET /v1/agents/chat`.
    pub line: String,
    /// The `Authorization` header, if any.
    pub authorization: Option<String>,
}

pub struct Stub {
    pub url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    /// `handler(request line, how many times this request line has been seen, counting this one)`.
    pub fn start(handler: impl Fn(&str, usize) -> Answer + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let log = seen.clone();
        let held: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = handler.clone();
                let log = log.clone();
                let held = held.clone();
                std::thread::spawn(move || serve(stream, &*handler, &log, &held));
            }
        });
        Self { url, seen }
    }

    /// A stub that answers every request with the same JSON problem.
    pub fn answering(status: u16, body: &str) -> Self {
        let body = body.to_owned();
        Self::start(move |_, _| Answer::Say(status, json_headers(), body.clone()))
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn lines(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.line).collect()
    }

    pub fn count(&self, line_starts_with: &str) -> usize {
        self.lines()
            .iter()
            .filter(|l| l.starts_with(line_starts_with))
            .count()
    }
}

pub fn json_headers() -> Vec<(String, String)> {
    vec![("Content-Type".to_owned(), "application/json".to_owned())]
}

/// A problem of the API: `urn:vymalo:agents:problem:<slug>`.
pub fn problem(status: u16, slug: &str, detail: &str, more: &str) -> String {
    format!(
        r#"{{"type":"urn:vymalo:agents:problem:{slug}","title":"t","status":{status},"detail":{}{more}}}"#,
        serde_json::to_string(detail).unwrap()
    )
}

fn serve(
    mut stream: TcpStream,
    handler: &(dyn Fn(&str, usize) -> Answer + Send + Sync),
    log: &Arc<Mutex<Vec<Seen>>>,
    held: &Arc<Mutex<Vec<TcpStream>>>,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.lines();
    let request = lines.next().unwrap_or("").to_owned();
    let line = request
        .rsplit_once(' ')
        .map_or(request.clone(), |(l, _)| l.to_owned());
    let mut authorization = None;
    let mut length = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "authorization" => authorization = Some(v.trim().to_owned()),
                "content-length" => length = v.trim().parse().unwrap_or(0),
                _ => {}
            }
        }
    }
    while buffer.len() < head_end + length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
    let nth = {
        let mut log = log.lock().unwrap();
        log.push(Seen {
            line: line.clone(),
            authorization,
        });
        log.iter().filter(|s| s.line == line).count()
    };
    match handler(&line, nth) {
        Answer::Hang => {
            held.lock().unwrap().push(stream);
        }
        Answer::Say(status, headers, body) => {
            let mut out = format!(
                "HTTP/1.1 {status} X\r\nConnection: close\r\nContent-Length: {}\r\n",
                body.len()
            );
            for (k, v) in headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&body);
            let _ = stream.write_all(out.as_bytes());
            let _ = stream.flush();
        }
    }
}
