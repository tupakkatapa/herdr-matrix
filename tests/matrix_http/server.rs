//! A local HTTP server that answers every request with `reply`, after
//! `delay`, and records what it got. It stops when dropped.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

type Reply = Box<dyn FnMut(&str) -> (u16, String, Vec<u8>) + Send>;

pub struct Server {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    stop: Arc<AtomicBool>,
    addr: std::net::SocketAddr,
}

impl Server {
    pub fn new(reply: impl FnMut(&str) -> (u16, String, Vec<u8>) + Send + 'static) -> Self {
        Self::slow(Duration::ZERO, reply)
    }

    pub fn slow(
        delay: Duration,
        reply: impl FnMut(&str) -> (u16, String, Vec<u8>) + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let reply: Arc<Mutex<Reply>> = Arc::new(Mutex::new(Box::new(reply)));
        {
            let (requests, stop) = (requests.clone(), stop.clone());
            thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(stream) = stream {
                        thread::sleep(delay);
                        answer(stream, &requests, &reply);
                    }
                }
            });
        }
        Self {
            url: format!("http://{addr}"),
            requests,
            stop,
            addr,
        }
    }

    pub fn last(&self) -> Request {
        self.requests
            .lock()
            .expect("lock")
            .pop()
            .expect("a request")
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.close();
    }
}

fn answer(stream: TcpStream, requests: &Mutex<Vec<Request>>, reply: &Mutex<Reply>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).expect("header");
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((k, v)) = header.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_owned());
        }
    }
    let size = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; size];
    reader.read_exact(&mut body).expect("body");
    let (code, kind, out) = (reply.lock().expect("lock"))(&path);
    requests.lock().expect("lock").push(Request {
        method,
        path,
        headers,
        body,
    });
    let mut stream = stream;
    let _ = write!(
        stream,
        "HTTP/1.1 {code} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        out.len()
    );
    let _ = stream.write_all(&out);
}
