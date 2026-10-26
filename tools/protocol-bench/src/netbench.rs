// SPDX-License-Identifier: GPL-3.0-only
//! Loopback round-trip latency for the shipped framing over four paths.
//!
//! * `blocking` — both ends sit in a blocking read: the event-driven floor.
//! * `tick10` — both ends run the module's shape: a 10 ms poll cadence, up to
//!   `READ_CHUNK = 1024` per read and `READS_PER_TICK = 8` reads per tick.
//! * `relay` — a two-hop host relay with 8 KiB pumps and `TCP_NODELAY`, the
//!   `tools/m2-rig` mesh relay path.
//! * `relay+tick` — the relay plus the 10 ms tick at both guests.
//! * `hub` — the `tools/m2-rig` M3 hub writer: 128-byte chunks with a 2 ms
//!   sleep between chunks.
//!
//! Every path carries the real record bytes produced by `mesh-transport`.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use crate::stats::{header, ms, row, Series};

use crate::proto::{ping_request, ping_response, write_record, NonBlockingReader, RecordReader};

/// Module tick cadence: `kernel/src/idle.rs` (`INTERVAL = 330_000` at 33 MHz).
const TICK: Duration = Duration::from_millis(10);
/// Module reads per tick: `modules/mcp/src/mesh.rs` `READS_PER_TICK`.
const READS_PER_TICK: usize = 8;
/// Hub writer chunk and sleep: `tools/m2-rig/src/hub.rs` `connect`.
const HUB_CHUNK: usize = 128;
const HUB_SLEEP: Duration = Duration::from_millis(2);

/// Bind a loopback listener and return it with its address.
fn listener() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    (listener, address)
}

/// A blocking echo server: read one record, answer with a response record.
fn serve_blocking(listener: TcpListener, reply_body: usize) {
    let (stream, _) = listener.accept().expect("accept");
    stream.set_nodelay(true).expect("nodelay");
    let mut reader = RecordReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    while reader.read_record_len().is_ok() {
        let tag = {
            let envelope = mesh_transport::decode_record(reader.record()).expect("decode");
            envelope.frame.header.tag.0
        };
        reader.consume();
        let response = ping_response(tag, reply_body.min(16));
        if write_record(&mut writer, &response).is_err() {
            break;
        }
    }
    let _ = writer.shutdown(Shutdown::Both);
}

/// A client that sends `count` records and waits for each answer.
fn drive_blocking(address: SocketAddr, count: usize, body: usize) -> Series {
    let stream = TcpStream::connect(address).expect("connect");
    stream.set_nodelay(true).expect("nodelay");
    let mut reader = RecordReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut samples = Vec::with_capacity(count);
    for index in 0..count {
        let request = ping_request(index as u32 + 1, body);
        let start = Instant::now();
        write_record(&mut writer, &request).expect("request");
        reader.read_record_len().expect("response");
        let body_bytes = reader.body();
        assert_eq!(body_bytes.len(), 16);
        reader.consume();
        samples.push(start.elapsed().as_nanos() as u64);
    }
    Series::new(samples)
}

/// A ticked echo server: one 10 ms cadence, bounded reads, one record per tick.
fn serve_tick(listener: TcpListener) {
    let (stream, _) = listener.accept().expect("accept");
    stream.set_nodelay(true).expect("nodelay");
    stream.set_nonblocking(true).expect("nonblocking");
    let mut reader = NonBlockingReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut next = Instant::now() + TICK;
    loop {
        sleep_until(&mut next);
        match reader.pump(READS_PER_TICK) {
            Ok(true) => {}
            _ => break,
        }
        while reader.ready() {
            let tag = {
                let envelope = mesh_transport::decode_record(reader.record()).expect("decode");
                envelope.frame.header.tag.0
            };
            reader.consume();
            let response = ping_response(tag, 16);
            let mut offset = 0;
            while offset < response.len() {
                match writer.write(&response[offset..]) {
                    Ok(0) => return,
                    Ok(written) => offset += written,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        sleep_until(&mut next);
                    }
                    Err(_) => return,
                }
            }
        }
    }
    let _ = writer.shutdown(Shutdown::Both);
}

/// A ticked client: send at a tick, poll for the answer at each later tick.
fn drive_tick(address: SocketAddr, count: usize, body: usize) -> Series {
    let stream = TcpStream::connect(address).expect("connect");
    stream.set_nodelay(true).expect("nodelay");
    stream.set_nonblocking(true).expect("nonblocking");
    let mut reader = NonBlockingReader::new(stream.try_clone().expect("clone"));
    let mut writer = stream;
    let mut samples = Vec::with_capacity(count);
    let mut next = Instant::now() + TICK;
    for index in 0..count {
        sleep_until(&mut next);
        let request = ping_request(index as u32 + 1, body);
        let start = Instant::now();
        let mut offset = 0;
        while offset < request.len() {
            match writer.write(&request[offset..]) {
                Ok(0) => return Series::new(samples),
                Ok(written) => offset += written,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    sleep_until(&mut next)
                }
                Err(_) => return Series::new(samples),
            }
        }
        loop {
            sleep_until(&mut next);
            match reader.pump(READS_PER_TICK) {
                Ok(true) => {}
                _ => return Series::new(samples),
            }
            if reader.ready() {
                reader.consume();
                samples.push(start.elapsed().as_nanos() as u64);
                break;
            }
        }
    }
    Series::new(samples)
}

fn sleep_until(next: &mut Instant) {
    let now = Instant::now();
    if *next > now {
        thread::sleep(*next - now);
    }
    *next += TICK;
    let now = Instant::now();
    if *next < now {
        // Fell behind: re-base instead of accumulating a burst of ticks.
        *next = now + TICK;
    }
}

/// Pump bytes from one stream to another until either side closes.
fn pump(mut from: TcpStream, mut to: TcpStream) {
    let mut buffer = [0u8; 8192];
    loop {
        match from.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if to.write_all(&buffer[..count]).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = to.shutdown(Shutdown::Both);
}

/// A two-hop relay: accept the client, dial the server, pump both ways.
fn spawn_relay(listener: TcpListener, target: SocketAddr) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (client, _) = listener.accept().expect("relay accept");
        let server = TcpStream::connect(target).expect("relay connect");
        client.set_nodelay(true).ok();
        server.set_nodelay(true).ok();
        let client_read = client.try_clone().expect("clone");
        let server_read = server.try_clone().expect("clone");
        let up = thread::spawn(move || pump(client_read, server));
        let down = thread::spawn(move || pump(server_read, client));
        up.join().ok();
        down.join().ok();
    })
}

/// A chunked, record-oriented forwarder, matching the M3 hub writer.
fn chunk_pump(from: TcpStream, mut to: TcpStream) {
    let mut reader = RecordReader::new(from.try_clone().expect("clone"));
    while reader.read_record_len().is_ok() {
        let record = reader.record().to_vec();
        reader.consume();
        let mut failed = false;
        for chunk in record.chunks(HUB_CHUNK) {
            if to.write_all(chunk).is_err() {
                failed = true;
                break;
            }
            thread::sleep(HUB_SLEEP);
        }
        if failed {
            break;
        }
    }
    let _ = from.shutdown(Shutdown::Both);
    let _ = to.shutdown(Shutdown::Both);
}

/// Spawn a chunked relay (the M3 hub writer shape).
fn spawn_chunked_relay(listener: TcpListener, target: SocketAddr) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let (client, _) = listener.accept().expect("hub accept");
        let server = TcpStream::connect(target).expect("hub connect");
        client.set_nodelay(true).ok();
        server.set_nodelay(true).ok();
        let client_read = client.try_clone().expect("clone");
        let server_read = server.try_clone().expect("clone");
        let up = thread::spawn(move || chunk_pump(client_read, server));
        let down = thread::spawn(move || chunk_pump(server_read, client));
        up.join().ok();
        down.join().ok();
    })
}

fn report(title: &str, series: &Series, records: usize, bytes: usize) {
    let median = series.pct(50.0).max(1);
    let rate = 1_000_000_000.0 / median as f64;
    row(&[
        title.to_string(),
        records.to_string(),
        bytes.to_string(),
        format!("{:.3}", ms(series.pct(50.0))),
        format!("{:.3}", ms(series.pct(90.0))),
        format!("{:.3}", ms(series.pct(99.0))),
        format!("{:.3}", ms(series.min())),
        format!("{rate:.0}"),
    ]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Path {
    Blocking,
    Tick,
    Relay,
    RelayTick,
    Hub,
}

fn scenario(path: Path, count: usize, body: usize) -> Series {
    let (server_listener, server_address) = listener();
    let ticked = matches!(path, Path::Tick | Path::RelayTick);
    let server = if ticked {
        thread::spawn(move || serve_tick(server_listener))
    } else {
        thread::spawn(move || serve_blocking(server_listener, 16))
    };

    let client_address = match path {
        Path::Blocking | Path::Tick => server_address,
        Path::Relay | Path::RelayTick => {
            let (relay_listener, relay_address) = listener();
            spawn_relay(relay_listener, server_address);
            relay_address
        }
        Path::Hub => {
            let (relay_listener, relay_address) = listener();
            spawn_chunked_relay(relay_listener, server_address);
            relay_address
        }
    };

    // The client dials immediately; the listener is already bound, so the
    // accept side runs as soon as the relay or the server reaches it.
    let series = if ticked {
        drive_tick(client_address, count, body)
    } else {
        drive_blocking(client_address, count, body)
    };
    server.join().ok();
    series
}

/// Run the network section.
pub fn run() {
    println!("\n## Loopback round-trip latency (measured on this host)\n");
    println!(
        "Each row is one request record and one response record carrying the real \
         `MeshEnvelope`.  `req/s` is the serial (one in flight) rate implied by p50.\n"
    );
    println!("{}\n", crate::stats::host_line());
    header(&[
        "path", "n", "req B", "p50 ms", "p90 ms", "p99 ms", "min ms", "req/s",
    ]);

    const BLOCKING_N: usize = 2_000;
    const TICK_N: usize = 300;
    const HUB_N: usize = 60;

    for body in [16usize, 332] {
        let bytes = 4 + 128 + 128 + 64 + body;
        report(
            "blocking (event-driven)",
            &scenario(Path::Blocking, BLOCKING_N, body),
            BLOCKING_N,
            bytes,
        );
        report(
            "two-hop host relay",
            &scenario(Path::Relay, BLOCKING_N, body),
            BLOCKING_N,
            bytes,
        );
        report(
            "10 ms tick, both ends",
            &scenario(Path::Tick, TICK_N, body),
            TICK_N,
            bytes,
        );
        report(
            "relay + 10 ms tick",
            &scenario(Path::RelayTick, TICK_N, body),
            TICK_N,
            bytes,
        );
    }

    for body in [16usize, 332, 2176] {
        let bytes = 4 + 128 + 128 + 64 + body;
        report(
            &format!("M3 hub writer ({body} B body)"),
            &scenario(Path::Hub, HUB_N, body),
            HUB_N,
            bytes,
        );
        let chunks = bytes.div_ceil(HUB_CHUNK);
        println!(
            "  - {bytes} B record = {chunks} chunks = {} ms of writer sleep per direction",
            chunks * 2
        );
    }

    println!();
    println!("Notes:");
    println!(
        "- `blocking` is the floor this host can reach with a real TCP socket and the real codec."
    );
    println!("- `10 ms tick` reproduces `kernel/src/idle.rs` (10 ms timer) and `modules/mcp/src/mesh.rs`");
    println!("  (1024-byte read chunk, 8 reads per tick, one flush per tick) on both ends.");
    println!(
        "- The relay is `tools/m2-rig/src/relay.rs`: two TCP connections, 8 KiB pumps, nodelay."
    );
    println!("- The hub row is `tools/m2-rig/src/hub.rs` `connect`: every record is written in");
    println!("  128-byte chunks with a 2 ms sleep after each chunk, in both directions.");
    println!(
        "- The guest adds its own CPU time on an emulated R4000; none of these rows include it."
    );
}
