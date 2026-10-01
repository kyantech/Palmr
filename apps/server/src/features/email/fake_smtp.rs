use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use base64ct::Encoding;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const CERTIFICATE: &[u8] = include_bytes!("testdata/smtp_test_cert.der");
const PRIVATE_KEY: &[u8] = include_bytes!("testdata/smtp_test_key.der");
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Plain,
    StartTls,
    Implicit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    Unavailable,
    Accepts { username: String, password: String },
    RejectsEveryone,
}

#[derive(Debug, Clone)]
pub struct Behavior {
    pub wire: Wire,
    pub advertise_starttls: bool,
    pub auth: Auth,
    pub data_reply: &'static str,
}

impl Behavior {
    pub fn new(wire: Wire) -> Self {
        Self {
            wire,
            advertise_starttls: true,
            auth: Auth::Unavailable,
            data_reply: "250 2.0.0 queued",
        }
    }

    #[must_use]
    pub fn with_auth(mut self, username: &str, password: &str) -> Self {
        self.auth = Auth::Accepts {
            username: username.to_owned(),
            password: password.to_owned(),
        };
        self
    }
}

#[derive(Debug, Default, Clone)]
pub struct Record {
    pub connections: usize,
    pub commands: Vec<String>,
    pub auth_attempts: Vec<(String, String)>,
    pub messages: Vec<String>,
    pub encrypted_commands: usize,
    pub plaintext_commands: usize,
    pub tls_failures: usize,
}

pub struct FakeSmtp {
    address: SocketAddr,
    record: Arc<Mutex<Record>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FakeSmtp {
    pub fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let record = Arc::new(Mutex::new(Record::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let tls = Arc::new(server_config());
        let accept = {
            let record = Arc::clone(&record);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let behavior = behavior.clone();
                    let record = Arc::clone(&record);
                    let tls = Arc::clone(&tls);
                    std::thread::spawn(move || serve(stream, &behavior, &tls, &record));
                }
            })
        };
        Self {
            address,
            record,
            stop,
            accept: Some(accept),
        }
    }

    pub fn port(&self) -> u16 {
        self.address.port()
    }

    pub fn record(&self) -> Record {
        lock(&self.record).clone()
    }
}

impl Drop for FakeSmtp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        drop(TcpStream::connect(self.address));
        if let Some(accept) = self.accept.take() {
            drop(accept.join());
        }
    }
}

fn lock(record: &Mutex<Record>) -> MutexGuard<'_, Record> {
    record.lock().unwrap_or_else(PoisonError::into_inner)
}

fn server_config() -> ServerConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(CERTIFICATE.to_vec())],
            PrivatePkcs8KeyDer::from(PRIVATE_KEY.to_vec()).into(),
        )
        .unwrap()
}

enum Stream {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ServerConnection, TcpStream>>),
}

impl Stream {
    fn encrypted(&self) -> bool {
        matches!(self, Self::Tls(_))
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buf),
            Self::Tls(stream) => stream.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buf),
            Self::Tls(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn encrypt(stream: TcpStream, tls: &Arc<ServerConfig>) -> Stream {
    let connection = ServerConnection::new(Arc::clone(tls)).unwrap();
    Stream::Tls(Box::new(StreamOwned::new(connection, stream)))
}

fn serve(stream: TcpStream, behavior: &Behavior, tls: &Arc<ServerConfig>, record: &Mutex<Record>) {
    drop(stream.set_read_timeout(Some(IO_TIMEOUT)));
    drop(stream.set_write_timeout(Some(IO_TIMEOUT)));
    lock(record).connections += 1;
    let wire = match behavior.wire {
        Wire::Implicit => encrypt(stream, tls),
        Wire::Plain | Wire::StartTls => Stream::Plain(stream),
    };
    let mut session = BufReader::new(wire);
    if reply(&mut session, "220 localhost ESMTP fake").is_err() {
        lock(record).tls_failures += 1;
        return;
    }
    let mut encrypted_seen = false;
    loop {
        let mut line = String::new();
        match session.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(_) => {
                if session.get_ref().encrypted() || behavior.wire == Wire::Implicit {
                    lock(record).tls_failures += 1;
                }
                return;
            }
        }
        let command = line.trim_end().to_owned();
        let verb = command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_uppercase();
        {
            let mut state = lock(record);
            if session.get_ref().encrypted() {
                state.encrypted_commands += 1;
                encrypted_seen = true;
            } else {
                state.plaintext_commands += 1;
            }
            if verb != "AUTH" {
                state.commands.push(verb.clone());
            }
        }
        let outcome = match verb.as_str() {
            "EHLO" | "HELO" => {
                let greeting = ehlo(behavior, session.get_ref().encrypted());
                reply(&mut session, &greeting)
            }
            "STARTTLS" if behavior.wire == Wire::StartTls && behavior.advertise_starttls => {
                if reply(&mut session, "220 ready to start TLS").is_err() {
                    return;
                }
                let Stream::Plain(tcp) = session.into_inner() else {
                    return;
                };
                session = BufReader::new(encrypt(tcp, tls));
                continue;
            }
            "STARTTLS" => reply(&mut session, "502 5.5.1 STARTTLS not available"),
            "AUTH" => authenticate(&mut session, behavior, &command, record),
            "MAIL" | "RCPT" | "RSET" | "NOOP" => reply(&mut session, "250 2.1.0 ok"),
            "DATA" => receive(&mut session, behavior, record),
            "QUIT" => {
                drop(reply(&mut session, "221 2.0.0 bye"));
                if let Stream::Plain(stream) = session.get_ref() {
                    drop(stream.shutdown(Shutdown::Both));
                }
                return;
            }
            _ => reply(&mut session, "502 5.5.2 command not recognized"),
        };
        if outcome.is_err() {
            if encrypted_seen || behavior.wire == Wire::Implicit {
                lock(record).tls_failures += 1;
            }
            return;
        }
    }
}

fn ehlo(behavior: &Behavior, encrypted: bool) -> String {
    let mut lines = vec!["250-localhost".to_owned()];
    if behavior.wire == Wire::StartTls && behavior.advertise_starttls && !encrypted {
        lines.push("250-STARTTLS".to_owned());
    }
    if behavior.auth != Auth::Unavailable {
        lines.push("250-AUTH PLAIN LOGIN".to_owned());
    }
    lines.push("250 8BITMIME".to_owned());
    lines.join("\r\n")
}

fn reply(session: &mut BufReader<Stream>, text: &str) -> std::io::Result<()> {
    let stream = session.get_mut();
    stream.write_all(text.as_bytes())?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

fn authenticate(
    session: &mut BufReader<Stream>,
    behavior: &Behavior,
    command: &str,
    record: &Mutex<Record>,
) -> std::io::Result<()> {
    let Auth::Accepts { username, password } = &behavior.auth else {
        return reply(session, "535 5.7.8 authentication failed");
    };
    let mut parts = command.split_whitespace().skip(1);
    let mechanism = parts.next().unwrap_or_default().to_ascii_uppercase();
    let (supplied_user, supplied_password) = match mechanism.as_str() {
        "PLAIN" => {
            let initial = match parts.next() {
                Some(initial) => initial.to_owned(),
                None => {
                    reply(session, "334 ")?;
                    read_line(session)?
                }
            };
            let decoded = base64ct::Base64::decode_vec(&initial).unwrap_or_default();
            let mut fields = decoded.split(|byte| *byte == 0).skip(1);
            (
                String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned(),
                String::from_utf8_lossy(fields.next().unwrap_or_default()).into_owned(),
            )
        }
        "LOGIN" => {
            reply(session, "334 VXNlcm5hbWU6")?;
            let user = decode(&read_line(session)?);
            reply(session, "334 UGFzc3dvcmQ6")?;
            (user, decode(&read_line(session)?))
        }
        _ => return reply(session, "504 5.5.4 unrecognized mechanism"),
    };
    let accepted = supplied_user == *username && supplied_password == *password;
    lock(record)
        .auth_attempts
        .push((supplied_user, supplied_password));
    if accepted {
        reply(session, "235 2.7.0 authenticated")
    } else {
        reply(session, "535 5.7.8 authentication failed")
    }
}

fn decode(text: &str) -> String {
    String::from_utf8_lossy(&base64ct::Base64::decode_vec(text).unwrap_or_default()).into_owned()
}

fn read_line(session: &mut BufReader<Stream>) -> std::io::Result<String> {
    let mut line = String::new();
    session.read_line(&mut line)?;
    Ok(line.trim_end().to_owned())
}

fn receive(
    session: &mut BufReader<Stream>,
    behavior: &Behavior,
    record: &Mutex<Record>,
) -> std::io::Result<()> {
    reply(session, "354 end with <CRLF>.<CRLF>")?;
    let mut message = String::new();
    loop {
        let mut line = String::new();
        if session.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if line == ".\r\n" {
            break;
        }
        message.push_str(&line);
    }
    if behavior.data_reply.starts_with('2') {
        lock(record).messages.push(message);
    }
    reply(session, behavior.data_reply)
}
