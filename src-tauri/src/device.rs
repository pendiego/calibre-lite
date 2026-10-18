//! calibre "Smart Device App" wireless protocol, server (calibre) side. Client: KOReader.
use crate::library::{self, ascii_filename};
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const BROADCAST_PORTS: [u16; 5] = [54982, 48123, 39001, 44044, 59678];

const OK: u8 = 0;
const SET_CALIBRE_DEVICE_INFO: u8 = 1;
const GET_DEVICE_INFORMATION: u8 = 3;
const GET_BOOK_COUNT: u8 = 6;
const SEND_BOOK: u8 = 8;
const GET_INITIALIZATION_INFO: u8 = 9;
const NOOP: u8 = 12;
const ERROR: u8 = 20;

pub enum Cmd {
    Send(Vec<SendItem>),
}

pub struct SendItem {
    pub uuid: String,
    pub title: String,
    pub lpath: String,
    pub file: PathBuf,
    pub metadata: Value,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub connected: bool,
    pub device_name: Option<String>,
    pub port: u16,
    pub discovery: bool,
    pub on_device: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub current: usize,
    pub total: usize,
    pub title: String,
    pub error: Option<String>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub enum DeviceEvent {
    Status(DeviceStatus),
    Progress(SendProgress),
}

#[derive(Default)]
pub struct Shared {
    pub connected: bool,
    pub device_name: Option<String>,
    pub accepted: Vec<String>,
    pub ext_path_lengths: HashMap<String, usize>,
    pub on_device: HashSet<String>,
    pub port: u16,
    pub discovery: bool,
}

pub struct Handle {
    pub shared: Arc<Mutex<Shared>>,
    pub tx: Sender<Cmd>,
    pub tcp_port: u16,
    pub udp_port: Option<u16>,
}

type Emit = Arc<dyn Fn(DeviceEvent) + Send + Sync>;

pub fn status(sh: &Shared) -> DeviceStatus {
    let mut on_device: Vec<String> = sh.on_device.iter().cloned().collect();
    on_device.sort();
    DeviceStatus { connected: sh.connected, device_name: sh.device_name.clone(), port: sh.port, discovery: sh.discovery, on_device }
}

// ---------- framing: ASCII decimal byte length + JSON `[opcode, {payload}]` ----------

pub fn write_frame(w: &mut impl Write, op: u8, payload: &Value) -> io::Result<()> {
    let body = serde_json::to_string(&json!([op, payload]))?;
    w.write_all(format!("{}{}", body.len(), body).as_bytes())?;
    w.flush()
}

fn invalid(m: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, m.to_string())
}

// No BufReader on purpose: keeps `peek` truthful.
pub fn read_frame(r: &mut impl Read) -> io::Result<(u8, Value)> {
    let mut digits = String::new();
    let mut b = [0u8; 1];
    loop {
        r.read_exact(&mut b)?;
        if b[0] == b'[' {
            break;
        }
        if digits.len() >= 12 || !b[0].is_ascii_digit() {
            return Err(invalid("bad frame length"));
        }
        digits.push(b[0] as char);
    }
    let len: usize = digits.parse().map_err(|_| invalid("bad frame length"))?;
    if len == 0 || len > 64 * 1024 * 1024 {
        return Err(invalid("bad frame length"));
    }
    let mut buf = vec![0u8; len];
    buf[0] = b'[';
    r.read_exact(&mut buf[1..])?;
    let v: Value = serde_json::from_slice(&buf)?;
    let op = v[0].as_u64().ok_or_else(|| invalid("bad opcode"))? as u8;
    Ok((op, v[1].clone()))
}

fn expect_ok(r: &mut impl Read) -> io::Result<Value> {
    match read_frame(r)? {
        (OK, p) => Ok(p),
        (NOOP, _) => Err(io::Error::new(io::ErrorKind::ConnectionAborted, "client left")),
        (op, _) => Err(invalid(&format!("unexpected opcode {op}"))),
    }
}

// ---------- server ----------

fn host_name() -> String {
    let h = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into());
    h.split('.').next().unwrap_or("PC").replace(['(', ')', ';'], "")
}

pub fn start(tcp_addr: &str, udp_ports: &[u16], library_name: String, library_uuid: String, emit: Emit) -> io::Result<Handle> {
    let mut addr: SocketAddr = tcp_addr.to_socket_addrs()?.next().ok_or_else(|| invalid("bad address"))?;
    let listener = match TcpListener::bind(addr) {
        Ok(l) => l,
        Err(_) if addr.port() != 0 => {
            addr.set_port(0);
            TcpListener::bind(addr)?
        }
        Err(e) => return Err(e),
    };
    let tcp_port = listener.local_addr()?.port();

    let udp = udp_ports.iter().find_map(|p| UdpSocket::bind(("0.0.0.0", *p)).ok());
    let udp_port = udp.as_ref().and_then(|u| u.local_addr().ok()).map(|a| a.port());

    let shared = Arc::new(Mutex::new(Shared { port: tcp_port, discovery: udp.is_some(), ..Default::default() }));
    let (tx, rx) = mpsc::channel();

    if let Some(sock) = udp {
        let reply = format!("calibre wireless device client (on {});0,{tcp_port}", host_name());
        thread::spawn(move || {
            let mut buf = [0u8; 256];
            while let Ok((_, from)) = sock.recv_from(&mut buf) {
                let _ = sock.send_to(reply.as_bytes(), from);
            }
        });
    }

    let sh = shared.clone();
    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            while rx.try_recv().is_ok() {}
            let _ = serve(stream, &sh, &rx, &emit, &library_name, &library_uuid);
            {
                let mut g = sh.lock().unwrap();
                g.connected = false;
                g.on_device.clear();
                emit(DeviceEvent::Status(status(&g)));
            }
        }
    });
    Ok(Handle { shared, tx, tcp_port, udp_port })
}

fn serve(mut st: TcpStream, sh: &Arc<Mutex<Shared>>, rx: &Receiver<Cmd>, emit: &Emit, lib_name: &str, lib_uuid: &str) -> io::Result<()> {
    st.set_read_timeout(Some(Duration::from_secs(60)))?;
    st.set_nodelay(true)?;

    write_frame(
        &mut st,
        GET_INITIALIZATION_INFO,
        &json!({
            "serverProtocolVersion": 1,
            "validExtensions": library::ADD_EXTENSIONS,
            "passwordChallenge": "",
            "currentLibraryName": lib_name,
            "currentLibraryUUID": lib_uuid,
            "pubdateFormat": "MMM yyyy",
            "timestampFormat": "dd MMM yyyy",
            "lastModifiedFormat": "dd MMM yyyy",
            "calibre_version": [8, 0, 0],
            "canSupportUpdateBooks": true,
            "canSupportLpathChanges": true,
        }),
    )?;
    let info = expect_ok(&mut st)?;
    if info["versionOK"] != true {
        return Err(invalid("client version rejected"));
    }
    let accepted: Vec<String> = info["acceptedExtensions"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).collect()).unwrap_or_default();
    let ext_lens: HashMap<String, usize> = info["extensionPathLengths"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.to_lowercase(), v.as_u64()? as usize))).collect()).unwrap_or_default();
    let device_name = info["deviceName"].as_str().unwrap_or("KOReader").to_string();
    let max_packet = info["maxBookContentPacketLen"].as_u64().filter(|n| *n > 0).unwrap_or(4096) as usize;

    write_frame(&mut st, GET_DEVICE_INFORMATION, &json!({}))?;
    let dev = expect_ok(&mut st)?;
    let store_uuid = dev["device_info"]["device_store_uuid"].as_str().map(String::from).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    write_frame(
        &mut st,
        SET_CALIBRE_DEVICE_INFO,
        &json!({
            "device_store_uuid": store_uuid,
            "device_name": device_name,
            "location_code": "main",
            "last_library_uuid": lib_uuid,
            "calibre_version": "8.0.0",
            "date_last_connected": chrono::Utc::now().to_rfc3339(),
            "prefix": "",
        }),
    )?;
    expect_ok(&mut st)?;

    write_frame(
        &mut st,
        GET_BOOK_COUNT,
        &json!({"canStream": true, "canScan": true, "willUseCachedMetadata": true, "supportsSync": false, "canSupportBookFormatSync": true}),
    )?;
    let count = expect_ok(&mut st)?["count"].as_u64().unwrap_or(0);
    let mut on_device = HashSet::new();
    for _ in 0..count {
        let b = expect_ok(&mut st)?;
        if let Some(u) = b["uuid"].as_str() {
            on_device.insert(u.to_string());
        }
    }
    let mut last_sent = Instant::now();

    {
        let mut g = sh.lock().unwrap();
        *g = Shared { connected: true, device_name: Some(device_name), accepted, ext_path_lengths: ext_lens, on_device, port: g.port, discovery: g.discovery };
        emit(DeviceEvent::Status(status(&g)));
    }

    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(Cmd::Send(items)) => {
                send_batch(&mut st, &items, max_packet, sh, emit)?;
                last_sent = Instant::now();
                emit(DeviceEvent::Status(status(&sh.lock().unwrap())));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if last_sent.elapsed() >= Duration::from_secs(30) {
                    write_frame(&mut st, NOOP, &json!({}))?;
                    expect_ok(&mut st)?;
                    last_sent = Instant::now();
                } else {
                    // anything readable here = EOF or KOReader's parting NOOP
                    st.set_nonblocking(true)?;
                    let mut b = [0u8; 1];
                    let r = st.peek(&mut b);
                    st.set_nonblocking(false)?;
                    match r {
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) => return Err(e),
                        Ok(_) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "client left")),
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

fn send_batch(st: &mut TcpStream, items: &[SendItem], max_packet: usize, sh: &Arc<Mutex<Shared>>, emit: &Emit) -> io::Result<()> {
    let n = items.len();
    for (i, it) in items.iter().enumerate() {
        let progress = |error: Option<String>| {
            emit(DeviceEvent::Progress(SendProgress { current: i + 1, total: n, title: it.title.clone(), error, done: i + 1 == n }))
        };
        let bytes = match fs::read(&it.file) {
            Ok(b) => b,
            Err(e) => {
                progress(Some(format!("não foi possível ler o arquivo: {e}")));
                continue;
            }
        };
        write_frame(
            st,
            SEND_BOOK,
            &json!({
                "lpath": it.lpath, "length": bytes.len(), "metadata": it.metadata,
                "thisBook": i, "totalBooks": n,
                "willStreamBooks": true, "willStreamBinary": true,
                "wantsSendOkToSendbook": true, "canSupportLpathChanges": true,
            }),
        )?;
        match read_frame(st)? {
            (OK, _) => {
                for chunk in bytes.chunks(max_packet) {
                    st.write_all(chunk)?;
                }
                st.flush()?;
                sh.lock().unwrap().on_device.insert(it.uuid.clone());
                progress(None);
            }
            (ERROR, p) => progress(Some(p["message"].as_str().unwrap_or("erro no dispositivo").to_string())),
            (NOOP, _) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "client left")),
            (op, _) => return Err(invalid(&format!("unexpected opcode {op}"))),
        }
    }
    Ok(())
}

/// calibre's `_create_upload_path` with the default template `{title} - {authors} ({id})`.
pub fn send_item(b: &library::SendBook, ext_path_len: usize) -> SendItem {
    let maxlen = 250usize.saturating_sub(40 + ext_path_len);
    let raw = format!("{} - {} ({})", b.title_sort, b.authors.join(" & "), b.id).replace(['/', '\\'], "_");
    let mut name = ascii_filename(&raw);
    if name.starts_with(['.', '_']) {
        name = format!("x{}", &name[1..]);
    }
    let mut name = name.trim_end_matches('.').to_string();
    let limit = maxlen.saturating_sub(b.id.to_string().len() + 1);
    let chars: Vec<char> = name.chars().collect();
    if chars.len() > limit {
        let l = (chars.len() - (chars.len() - limit)) / 2;
        name = chars[..l].iter().chain(&chars[chars.len() - l..]).collect();
    }
    let lpath = ascii_filename(&format!("{name}.{}", b.ext));
    let metadata = json!({
        "title": b.title, "title_sort": b.title_sort, "authors": b.authors, "author_sort": b.author_sort,
        "tags": b.tags, "series": b.series, "series_index": b.series_index, "uuid": b.uuid,
        "lpath": lpath, "size": b.size, "last_modified": b.last_modified,
        "thumbnail": null, "_series_sort_": "",
    });
    SendItem { uuid: b.uuid.clone(), title: b.title.clone(), lpath, file: b.file.clone(), metadata }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;

    fn wait_for(ev: &Arc<Mutex<Vec<DeviceEvent>>>, f: impl Fn(&DeviceEvent) -> bool) -> DeviceEvent {
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            if let Some(e) = ev.lock().unwrap().iter().find(|e| f(e)) {
                return e.clone();
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("event not seen");
    }

    #[test]
    fn koreader_session() {
        let events: Arc<Mutex<Vec<DeviceEvent>>> = Default::default();
        let ev = events.clone();
        let h = start("127.0.0.1:0", &[0], "Lib".into(), "lib-uuid".into(), Arc::new(move |e| ev.lock().unwrap().push(e))).unwrap();

        let u = UdpSocket::bind("127.0.0.1:0").unwrap();
        u.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        u.send_to(b"hello", ("127.0.0.1", h.udp_port.unwrap())).unwrap();
        let mut buf = [0u8; 200];
        let n = u.recv(&mut buf).unwrap();
        let msg = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(msg.starts_with("calibre wireless device client (on ") && msg.ends_with(&format!(");0,{}", h.tcp_port)), "{msg}");

        let mut c = TcpStream::connect(("127.0.0.1", h.tcp_port)).unwrap();
        let (op, p) = read_frame(&mut c).unwrap();
        assert_eq!(op, GET_INITIALIZATION_INFO);
        assert!(p["calibre_version"].is_array());
        write_frame(&mut c, OK, &json!({"versionOK": true, "acceptedExtensions": ["epub", "pdf"], "extensionPathLengths": {"epub": 4, "pdf": 3}, "maxBookContentPacketLen": 4096, "deviceName": "KOReader (test)"})).unwrap();
        assert_eq!(read_frame(&mut c).unwrap().0, GET_DEVICE_INFORMATION);
        write_frame(&mut c, OK, &json!({"device_info": {"device_store_uuid": null}})).unwrap();
        assert_eq!(read_frame(&mut c).unwrap().0, SET_CALIBRE_DEVICE_INFO);
        write_frame(&mut c, OK, &json!({})).unwrap();
        assert_eq!(read_frame(&mut c).unwrap().0, GET_BOOK_COUNT);
        write_frame(&mut c, OK, &json!({"count": 1})).unwrap();
        write_frame(&mut c, OK, &json!({"priKey": 1, "uuid": "u-1", "lpath": "x.epub", "last_modified": "2020-01-01T00:00:00+00:00"})).unwrap();

        let DeviceEvent::Status(s) = wait_for(&events, |e| matches!(e, DeviceEvent::Status(s) if s.connected)) else { unreachable!() };
        assert_eq!(s.on_device, vec!["u-1"]);
        assert_eq!(s.device_name.as_deref(), Some("KOReader (test)"));

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("b.epub");
        let data: Vec<u8> = (0..10_000u32).map(|i| i as u8).collect();
        fs::write(&file, &data).unwrap();
        let item = |u: &str| SendItem { uuid: u.into(), title: "T".into(), lpath: "t.epub".into(), file: file.clone(), metadata: json!({}) };

        h.tx.send(Cmd::Send(vec![item("u-2")])).unwrap();
        let (op, p) = read_frame(&mut c).unwrap();
        assert_eq!((op, p["lpath"].as_str(), p["length"].as_u64()), (SEND_BOOK, Some("t.epub"), Some(10_000)));
        write_frame(&mut c, OK, &json!({})).unwrap();
        let mut got = vec![0u8; 10_000];
        c.read_exact(&mut got).unwrap();
        assert_eq!(got, data);
        let DeviceEvent::Progress(p) = wait_for(&events, |e| matches!(e, DeviceEvent::Progress(_))) else { unreachable!() };
        assert_eq!((p.current, p.total, p.done, p.error), (1, 1, true, None));
        assert!(h.shared.lock().unwrap().on_device.contains("u-2"));

        events.lock().unwrap().clear();
        h.tx.send(Cmd::Send(vec![item("u-3")])).unwrap();
        assert_eq!(read_frame(&mut c).unwrap().0, SEND_BOOK);
        write_frame(&mut c, ERROR, &json!({"message": "no space"})).unwrap();
        let DeviceEvent::Progress(p) = wait_for(&events, |e| matches!(e, DeviceEvent::Progress(_))) else { unreachable!() };
        assert_eq!(p.error.as_deref(), Some("no space"));
        c.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        assert!(c.read(&mut [0u8; 1]).is_err(), "bytes were sent after error");
        assert!(!h.shared.lock().unwrap().on_device.contains("u-3"));

        drop(c);
        wait_for(&events, |e| matches!(e, DeviceEvent::Status(s) if !s.connected));
    }
}
