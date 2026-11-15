use std::collections::BTreeMap;
use std::io::{self, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone)]
pub(super) enum Reply {
    Raw(Vec<u8>),
    Disconnect,
    Stall,
    Sequence(Vec<Reply>),
}

impl Reply {
    pub(super) fn json(status: u16, body: &[u8], headers: &[(&str, &str)]) -> Self {
        let mut bytes = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: Application/Vnd.Craft+JSON; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n", body.len()).into_bytes();
        for (name, value) in headers {
            bytes.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"\r\n");
        bytes.extend_from_slice(body);
        Self::Raw(bytes)
    }
}

pub(super) struct Observed {
    pub(super) method: String,
    pub(super) target: String,
    pub(super) headers: BTreeMap<String, String>,
    pub(super) body: Vec<u8>,
}

pub(super) struct TlsServer {
    address: SocketAddr,
    root: Vec<u8>,
    accepted: Arc<AtomicUsize>,
    observed: Arc<Mutex<Vec<Observed>>>,
    received: Arc<AtomicUsize>,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<io::Result<()>>>,
}

impl TlsServer {
    pub(super) fn start(reply: Reply) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_on(0, reply)
    }
    /// Binds a chosen local port so a reply can name the server's own origin.
    pub(super) fn start_on(port: u16, reply: Reply) -> Result<Self, Box<dyn std::error::Error>> {
        use rcgen::{
            BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
            KeyUsagePurpose,
        };
        let root_key = KeyPair::generate()?;
        let mut root_params = CertificateParams::new(Vec::<String>::new())?;
        root_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "Craft fixture root");
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let root_cert = root_params.self_signed(&root_key)?;
        let root = root_cert.pem().into_bytes();
        let issuer = Issuer::new(root_params, root_key);
        let key = KeyPair::generate()?;
        let mut leaf = CertificateParams::new(vec!["localhost".into()])?;
        leaf.distinguished_name
            .push(rcgen::DnType::CommonName, "localhost");
        leaf.use_authority_key_identifier_extension = true;
        leaf.extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        leaf.key_usages.push(KeyUsagePurpose::DigitalSignature);
        let certificate = leaf.signed_by(&key, &issuer)?;
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![certificate.der().clone(), root_cert.der().clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
            )?;
        let config = Arc::new(config);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let connections = accepted.clone();
        let observed = Arc::new(Mutex::new(Vec::new()));
        let requests = observed.clone();
        let received = Arc::new(AtomicUsize::new(0));
        let request_count = received.clone();
        let (stop, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || -> io::Result<()> {
            loop {
                if receiver.try_recv().is_ok() {
                    return Ok(());
                }
                match listener.accept() {
                    Ok((socket, _)) => {
                        connections.fetch_add(1, Ordering::SeqCst);
                        // Windows accepts inherit the listener's nonblocking mode.
                        socket.set_nonblocking(false)?;
                        socket.set_read_timeout(Some(Duration::from_millis(500)))?;
                        socket.set_write_timeout(Some(Duration::from_millis(500)))?;
                        let connection = rustls::ServerConnection::new(config.clone())
                            .map_err(io::Error::other)?;
                        let mut stream = rustls::StreamOwned::new(connection, socket);
                        // Failed certificate/hostname handshakes intentionally produce no HTTP request.
                        let Ok(request) = read_request(&mut stream) else {
                            continue;
                        };
                        requests
                            .lock()
                            .map_err(|_| io::Error::other("Fixture lock poisoned"))?
                            .push(request);
                        let index = request_count.fetch_add(1, Ordering::SeqCst);
                        let selected = if let Reply::Sequence(replies) = &reply {
                            replies.get(index).ok_or_else(|| {
                                io::Error::other("Unexpected request after fixture reply sequence")
                            })?
                        } else {
                            &reply
                        };
                        match selected {
                            Reply::Raw(bytes) => {
                                let _ = stream.write_all(bytes).and_then(|()| stream.flush());
                            }
                            Reply::Disconnect => {}
                            Reply::Stall => {
                                let _ = receiver.recv_timeout(Duration::from_secs(2));
                                return Ok(());
                            }
                            Reply::Sequence(_) => {
                                return Err(io::Error::other(
                                    "Nested fixture reply sequences are unsupported",
                                ));
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => return Err(error),
                }
            }
        });
        Ok(Self {
            address,
            root,
            accepted,
            observed,
            received,
            stop,
            worker: Some(worker),
        })
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }
    pub(super) fn root(&self) -> Vec<u8> {
        self.root.clone()
    }
    pub(super) fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
    pub(super) fn request_count(&self) -> usize {
        self.received.load(Ordering::SeqCst)
    }
    pub(super) fn request_counter(&self) -> Arc<AtomicUsize> {
        self.received.clone()
    }
    pub(super) fn finish(mut self) -> Result<Vec<Observed>, Box<dyn std::error::Error>> {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| "TLS fixture panicked")??;
        }
        Ok(std::mem::take(
            &mut *self.observed.lock().map_err(|_| "Fixture lock poisoned")?,
        ))
    }
}

impl Drop for TlsServer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn read_request(stream: impl Read) -> io::Result<Observed> {
    let mut reader = BufReader::new(stream);
    let mut headers = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte)?;
        headers.push(byte[0]);
        if headers.len() > 32_768 {
            return Err(io::Error::other("Fixture headers too large"));
        }
        if headers.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&headers).map_err(io::Error::other)?;
    let mut lines = text.split("\r\n");
    let mut first = lines
        .next()
        .ok_or_else(|| io::Error::other("Missing request line"))?
        .split_whitespace();
    let method = first
        .next()
        .ok_or_else(|| io::Error::other("Missing method"))?
        .into();
    let target = first
        .next()
        .ok_or_else(|| io::Error::other("Missing target"))?
        .into();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 1_048_576 {
        return Err(io::Error::other("Fixture body too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Observed {
        method,
        target,
        headers,
        body,
    })
}
