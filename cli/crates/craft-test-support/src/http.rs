use std::collections::{BTreeMap, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const MAX_HEADER_BYTES: usize = 16_384;
const MAX_BODY_BYTES: usize = 1_048_576;

pub struct ExpectedRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub struct ObservedRequest {
    pub method: String,
    pub target: String,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

pub enum Reply {
    Response {
        status: u16,
        headers: BTreeMap<String, String>,
        body: Vec<u8>,
        delay: Duration,
    },
    DisconnectAfterRead,
}

pub struct Exchange {
    pub request: ExpectedRequest,
    pub reply: Reply,
}

struct Report {
    requests: Vec<ObservedRequest>,
    remaining: usize,
    failures: usize,
}

pub struct HttpRecorder {
    address: SocketAddr,
    count: Arc<AtomicUsize>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<io::Result<Report>>>,
}

impl HttpRecorder {
    pub fn start(exchanges: Vec<Exchange>) -> io::Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let (stop, receiver) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&count);
        let worker = thread::spawn(move || serve(listener, exchanges.into(), receiver, observed));
        Ok(Self {
            address,
            count,
            stop,
            worker: Some(worker),
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn request_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Fails on missing, unexpected or mismatched requests instead of inventing success.
    pub fn verify(mut self) -> io::Result<Vec<ObservedRequest>> {
        let _ = self.stop.send(());
        let worker = self
            .worker
            .take()
            .ok_or_else(|| io::Error::other("Fixture already stopped"))?;
        let report = worker
            .join()
            .map_err(|_| io::Error::other("HTTP fixture worker failed"))??;
        if report.remaining != 0 || report.failures != 0 {
            return Err(io::Error::other(
                "HTTP fixture had missing, unexpected or mismatched requests",
            ));
        }
        Ok(report.requests)
    }
}

impl Drop for HttpRecorder {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> io::Result<ObservedRequest> {
    // Accepted sockets can inherit the listener's nonblocking mode. Blocking reads with
    // a deadline must wait for fragmented headers/bodies instead of rejecting WouldBlock.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err(io::Error::other("Fixture header limit exceeded"));
        }
    }
    let header = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::other("Invalid fixture request header"))?;
    let mut lines = header.split("\r\n");
    let parts: Vec<_> = lines
        .next()
        .ok_or_else(|| io::Error::other("Missing request line"))?
        .split(' ')
        .collect();
    if parts.len() != 3 || !parts[2].starts_with("HTTP/1.") {
        return Err(io::Error::other("Invalid fixture request line"));
    }
    let mut headers = BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("Invalid fixture header"))?;
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_owned())
            .is_some()
        {
            return Err(io::Error::other("Duplicate fixture request header"));
        }
    }
    if headers.contains_key("transfer-encoding") {
        return Err(io::Error::other(
            "Fixture requires an explicit content length",
        ));
    }
    let size = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| io::Error::other("Invalid fixture body length"))?,
        None => 0,
    };
    if size > MAX_BODY_BYTES {
        return Err(io::Error::other("Fixture request body limit exceeded"));
    }
    let mut body = vec![0; size];
    reader.read_exact(&mut body)?;
    Ok(ObservedRequest {
        method: parts[0].into(),
        target: parts[1].into(),
        headers,
        body,
    })
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    write!(
        stream,
        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    )?;
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    stream.write_all(body)
}

fn serve(
    listener: TcpListener,
    mut expected: VecDeque<Exchange>,
    stop: mpsc::Receiver<()>,
    count: Arc<AtomicUsize>,
) -> io::Result<Report> {
    let mut report = Report {
        requests: Vec::new(),
        remaining: 0,
        failures: 0,
    };
    loop {
        if stop.try_recv().is_ok() {
            break;
        }
        let (mut stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            Err(error) => return Err(error),
        };
        count.fetch_add(1, Ordering::SeqCst);
        let request = match read_request(&mut stream) {
            Ok(request) => request,
            Err(_) => {
                report.failures += 1;
                continue;
            }
        };
        let exchange = expected.pop_front();
        let matching = exchange.as_ref().is_some_and(|exchange| {
            let wanted = &exchange.request;
            wanted.method == request.method
                && wanted.target == request.target
                && wanted.body == request.body
                && wanted.headers.iter().all(|(key, value)| {
                    request.headers.get(&key.to_ascii_lowercase()) == Some(value)
                })
        });
        report.requests.push(request);
        if !matching {
            report.failures += 1;
            let _ = respond(
                &mut stream,
                500,
                &BTreeMap::new(),
                b"Unexpected fixture request",
            );
            continue;
        }
        match exchange.map(|exchange| exchange.reply) {
            Some(Reply::DisconnectAfterRead) => {}
            Some(Reply::Response {
                status,
                headers,
                body,
                delay,
            }) => {
                if !delay.is_zero() && stop.recv_timeout(delay).is_ok() {
                    report.failures += 1;
                    break;
                }
                if respond(&mut stream, status, &headers, &body).is_err() {
                    report.failures += 1;
                }
            }
            None => report.failures += 1,
        }
    }
    report.remaining = expected.len();
    Ok(report)
}
