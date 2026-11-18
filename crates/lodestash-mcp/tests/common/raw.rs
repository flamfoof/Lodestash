//! A loopback listener that is not the app: it records what arrives and answers the way a test
//! asks. Shared by the integration tests and the crate's unit tests (std only).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the listener does with a connection once it has read what was sent.
#[derive(Clone, Copy)]
pub enum Behaviour {
    /// Close it without a word — an app that died with the request in hand.
    Drop,
    /// Keep it open and say nothing for this long — an app that is busy.
    Hold(Duration),
    /// Answer `200` with this JSON body.
    Answer(&'static str),
}

/// A listener on a port of its own. Every accepted connection is recorded with the bytes it
/// carried; a connection that carried none is a bare connect.
pub struct RawListener {
    pub port: u16,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RawListener {
    pub fn start(behaviour: Behaviour) -> RawListener {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().map_while(Result::ok) {
                let record = record.clone();
                std::thread::spawn(move || serve(stream, behaviour, &record));
            }
        });
        RawListener { port, seen }
    }

    /// Every connection so far, as the bytes it carried.
    pub fn connections(&self) -> Vec<Vec<u8>> {
        self.seen.lock().unwrap().clone()
    }

    /// The connections that carried an HTTP request, as text.
    pub fn requests(&self) -> Vec<String> {
        self.connections()
            .iter()
            .filter(|bytes| !bytes.is_empty())
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect()
    }

    /// Wait until a request has arrived, at most `within`.
    pub fn wait_for_request(&self, within: Duration) -> bool {
        let until = Instant::now() + within;
        while Instant::now() < until {
            if !self.requests().is_empty() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

fn serve(mut stream: TcpStream, behaviour: Behaviour, record: &Mutex<Vec<Vec<u8>>>) {
    let request = read_request(&mut stream);
    // Recorded before the behaviour runs, so a test can see a request the listener is holding.
    record.lock().unwrap().push(request);
    match behaviour {
        Behaviour::Drop => {}
        Behaviour::Hold(long) => std::thread::sleep(long),
        Behaviour::Answer(body) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.flush();
        }
    }
}

/// One HTTP request — headers, and the body its `Content-Length` announces — or whatever arrived
/// before the peer went quiet.
fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = find(&bytes, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let body = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + body {
                return bytes;
            }
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return bytes,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
