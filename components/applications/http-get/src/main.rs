#![no_std]
#![no_main]

//! One launch-supplied HTTP GET using the declared userspace network service.
//! Binary body bytes are streamed as bounded hexadecimal console records, never
//! interpreted as diagnostics. Requests are not replayed after any bytes are sent.

use slime_components::http::{self, Error, ResponseDecoder, Url};
use slime_components::network_io::{Connection, NetworkIo, NetworkNotifications, NetworkReply};
use slime_proto::network_service::{self as net, WireNetworkLaunch};
use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit};

slime_rt::entry!(main);

const RING_BASE: u64 = 0x1b_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
const REQUEST_SECONDS: u64 = 45;
const LAUNCH_SECONDS: u64 = 10;
const MAX_CONNECT_ATTEMPTS: usize = 64;
const MAX_IO_OPERATIONS: usize = 262144;

fn main(_: u32) {
    let control = slime_rt::resolve_binding(b"network-http-service")
        .unwrap_or_else(|_| fail(b"network binding"));
    let rate = frequency().unwrap_or_else(|()| fail(b"clock rate"));
    let mut bytes = [0; http::MAX_URL_BYTES];
    let length =
        receive_url(&mut bytes).unwrap_or_else(|()| setup_failure(control, rate, Error::Transport));
    let text = core::str::from_utf8(&bytes[..length])
        .unwrap_or_else(|_| setup_failure(control, rate, Error::InvalidUrl));
    let url = Url::parse(text).unwrap_or_else(|error| setup_failure(control, rate, error));
    let mut request = [0; http::MAX_URL_BYTES + 256];
    let request_len = url
        .encode_get(&mut request)
        .unwrap_or_else(|error| setup_failure(control, rate, error));
    let provision = slime_rt::resolve_binding(b"network-http-provision")
        .unwrap_or_else(|_| setup_failure(control, rate, Error::Transport));
    let until = deadline(rate, REQUEST_SECONDS)
        .unwrap_or_else(|()| setup_failure(control, rate, Error::Transport));
    let notifications =
        notifications(rate).unwrap_or_else(|()| setup_failure(control, rate, Error::Transport));
    // SAFETY: these disjoint page-aligned mappings are reserved for this sole
    // adapter; the declared control and provisioning endpoints name one service.
    let mut io = unsafe {
        NetworkIo::attach_with_deadline(
            control,
            provision,
            RING_BASE,
            DATA_BASE,
            notifications,
            until,
        )
    }
    .unwrap_or_else(|_| setup_failure(control, rate, Error::Transport));
    let mut parser = ResponseDecoder::new();
    let mut connection = None;
    let result = (|| {
        check_deadline(until)?;
        connection = Some(connect(&mut io, &url, until)?);
        let connection = connection.as_ref().ok_or(Error::Transport)?;
        send_all(&mut io, connection, &request[..request_len], until)?;
        receive_body(&mut io, connection, &mut parser, until)
    })();
    if result.is_err() {
        parser.abort();
    }
    let close_ok = result.is_ok()
        && connection
            .is_some_and(|connection| io.close(connection).is_ok_and(|reply| reply.is_success()));
    let abort = result.is_err() || !close_ok;
    let cleanup_until = deadline(rate, 5).unwrap_or(0);
    let finish_ok = if abort {
        io.abort_with_deadline(cleanup_until).is_ok()
    } else {
        io.finish_with_deadline(cleanup_until).is_ok()
    };
    let cleanup_output = cleanup_record(close_ok, finish_ok, abort).is_ok();

    let mut record = Record::new();
    record.text(b"[http-get] complete success=");
    let success = result.is_ok() && close_ok && finish_ok && cleanup_output;
    record.number(u64::from(success));
    record.text(b" status=");
    record.number(u64::from(parser.status().unwrap_or(0)));
    record.text(b" bytes=");
    record.number(parser.body_bytes() as u64);
    record.text(b" error=");
    record.text(match result {
        Ok(()) if close_ok && finish_ok => b"none",
        Ok(()) => b"cleanup",
        Err(error) => error_name(error),
    });
    record.text(b"\n");
    let reported = record.write().is_ok();
    exit(if success && reported { 0 } else { 1 })
}

fn cleanup_record(close: bool, detach: bool, abort: bool) -> Result<(), Error> {
    let mut record = Record::new();
    record.text(b"[http-get] cleanup close=");
    record.number(u64::from(close));
    record.text(b" detach=");
    record.number(u64::from(detach));
    if abort {
        record.text(b" abort=1");
    }
    record.text(b"\n");
    record.write()
}

fn setup_failure(control: u32, rate: u64, error: Error) -> ! {
    let detached = NetworkIo::abort_session(control, deadline(rate, 5).unwrap_or(0)).is_ok();
    let _ = cleanup_record(false, detached, true);
    let mut record = Record::new();
    record.text(b"[http-get] complete success=0 status=0 bytes=0 error=");
    record.text(error_name(error));
    record.text(b"\n");
    let _ = record.write();
    exit(1)
}

fn receive_url(out: &mut [u8; http::MAX_URL_BYTES]) -> Result<usize, ()> {
    let peer = slime_rt::resolve_binding(b"http-launch-client").map_err(|_| ())?;
    let until = deadline(frequency()?, LAUNCH_SECONDS)?;
    let mut length = 0;
    let mut total = None;
    let mut bytes = [0; MAX_MSG];
    let mut caps = [0; MAX_CAPS_PER_MSG];
    loop {
        check_deadline(until).map_err(|_| ())?;
        let count = slime_rt::recv(peer, &mut bytes, &mut caps);
        if count == ERR_WOULDBLOCK {
            slime_rt::yield_now();
            continue;
        }
        if count != net::LAUNCH_BYTES as i64 || caps.iter().any(|cap| *cap != 0) {
            return Err(());
        }
        let frame = WireNetworkLaunch::decode(&bytes[..count as usize])
            .filter(slime_proto::valid_network_launch)
            .filter(|frame| frame.kind == net::LAUNCH_URL)
            .ok_or(())?;
        let frame_total = usize::from(frame.total_len);
        let frame_len = usize::from(frame.length);
        if total.is_some_and(|total| total != frame_total)
            || usize::from(frame.offset) != length
            || frame_total == 0
            || frame_total > out.len()
            || frame_len == 0
            || frame_len > frame.payload.len()
            || length
                .checked_add(frame_len)
                .is_none_or(|end| end > frame_total)
        {
            return Err(());
        }
        total = Some(frame_total);
        out[length..length + frame_len].copy_from_slice(&frame.payload[..frame_len]);
        length += frame_len;
        if length == frame_total {
            return Ok(length);
        }
    }
}

fn notifications(rate: u64) -> Result<Option<NetworkNotifications>, ()> {
    let signal = slime_rt::resolve_binding(b"notification:network-http-request+signal");
    let wait = slime_rt::resolve_binding(b"notification:network-http-completion+wait");
    match (signal, wait) {
        (Ok(request_signal), Ok(completion_wait)) => Ok(Some(NetworkNotifications {
            request_signal,
            completion_wait,
            timeout_ticks: rate.checked_mul(20).ok_or(())?,
        })),
        (Err(slime_rt::ERR_INVALID_ARG), Err(slime_rt::ERR_INVALID_ARG)) => Ok(None),
        _ => Err(()),
    }
}

fn connect(io: &mut NetworkIo<'_>, url: &Url<'_>, until: u64) -> Result<Connection, Error> {
    let numeric = ipv4(url.host)?;
    for _ in 0..MAX_CONNECT_ATTEMPTS {
        check_deadline(until)?;
        let mut reply = match numeric {
            Some(address) => io.connect_ipv4(address, url.port),
            None => io.connect_hostname(url.host.as_bytes(), url.port),
        }
        .map_err(|_| Error::Transport)?;
        // Preserve an acquired handle for cleanup even if its synchronous
        // connect completed after the deadline; send_all checks the deadline.
        if let Some(connection) = reply.take_connection() {
            return Ok(connection);
        }
        if !would_block(&reply) {
            return Err(Error::Transport);
        }
        slime_rt::yield_now();
    }
    Err(Error::Limit)
}

fn ipv4(host: &str) -> Result<Option<[u8; 4]>, Error> {
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Ok(None);
    }
    let mut address = [0; 4];
    let mut labels = host.split('.');
    for octet in &mut address {
        let label = labels.next().ok_or(Error::InvalidUrl)?;
        if label.is_empty() || (label.len() > 1 && label.starts_with('0')) {
            return Err(Error::InvalidUrl);
        }
        *octet = label.parse().map_err(|_| Error::InvalidUrl)?;
    }
    if labels.next().is_some() {
        return Err(Error::InvalidUrl);
    }
    Ok(Some(address))
}

fn send_all(
    io: &mut NetworkIo<'_>,
    connection: &Connection,
    bytes: &[u8],
    until: u64,
) -> Result<(), Error> {
    let mut sent = 0;
    for _ in 0..MAX_IO_OPERATIONS {
        check_deadline(until)?;
        if sent == bytes.len() {
            return Ok(());
        }
        let end = (sent + net::DATA_BYTES).min(bytes.len());
        let reply = io
            .send(connection, &bytes[sent..end])
            .map_err(|_| Error::Transport)?;
        check_deadline(until)?;
        if reply.is_success() {
            let count = usize::try_from(reply.transferred).map_err(|_| Error::Malformed)?;
            if count > end - sent || reply.flags != 0 {
                return Err(Error::Malformed);
            }
            sent += count;
            if count == 0 {
                slime_rt::yield_now();
            }
        } else if would_block(&reply) {
            slime_rt::yield_now();
        } else {
            return Err(Error::Transport);
        }
    }
    Err(Error::Limit)
}

fn receive_body(
    io: &mut NetworkIo<'_>,
    connection: &Connection,
    parser: &mut ResponseDecoder,
    until: u64,
) -> Result<(), Error> {
    let mut bytes = [0; net::DATA_BYTES];
    for _ in 0..MAX_IO_OPERATIONS {
        check_deadline(until)?;
        let reply = io
            .recv(connection, &mut bytes)
            .map_err(|_| Error::Transport)?;
        check_deadline(until)?;
        if !reply.is_success() {
            if would_block(&reply) {
                slime_rt::yield_now();
                continue;
            }
            return Err(Error::Transport);
        }
        let count = usize::try_from(reply.transferred).map_err(|_| Error::Malformed)?;
        if count > bytes.len() || reply.flags & !net::FLAG_END_OF_STREAM != 0 {
            return Err(Error::Malformed);
        }
        parser.feed(&bytes[..count], emit_body)?;
        check_deadline(until)?;
        if reply.flags & net::FLAG_END_OF_STREAM != 0 {
            parser.finish_eof()?;
        }
        if parser.is_complete() {
            return Ok(());
        }
        if count == 0 {
            slime_rt::yield_now();
        }
    }
    Err(Error::Limit)
}

fn would_block(reply: &NetworkReply) -> bool {
    reply.queue_status == slime_proto::io_queue::STATUS_OK
        && reply.status_detail == net::STATUS_WOULD_BLOCK
        && reply.transferred == 0
        && reply.flags == 0
}

fn emit_body(bytes: &[u8]) -> Result<(), Error> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for part in bytes.chunks(64) {
        let mut record = Record::new();
        record.text(b"[http-get] body hex=");
        for byte in part {
            record.text(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 15)]]);
        }
        record.text(b"\n");
        record.write()?;
    }
    Ok(())
}

fn frequency() -> Result<u64, ()> {
    slime_rt::monotonic_frequency()
        .map_err(|_| ())
        .and_then(|rate| if rate == 0 { Err(()) } else { Ok(rate) })
}

fn deadline(rate: u64, seconds: u64) -> Result<u64, ()> {
    slime_rt::monotonic_read()
        .map_err(|_| ())?
        .checked_add(rate.checked_mul(seconds).ok_or(())?)
        .ok_or(())
}

fn check_deadline(until: u64) -> Result<(), Error> {
    if slime_rt::monotonic_read().map_err(|_| Error::Transport)? >= until {
        Err(Error::Transport)
    } else {
        Ok(())
    }
}

fn error_name(error: Error) -> &'static [u8] {
    match error {
        Error::InvalidUrl => b"url",
        Error::BufferTooSmall => b"buffer",
        Error::Malformed => b"malformed",
        Error::Unsupported => b"unsupported",
        Error::Limit => b"limit",
        Error::UnexpectedEof => b"early-eof",
        Error::Transport => b"transport",
        Error::Output => b"output",
    }
}

struct Record {
    bytes: [u8; 192],
    length: usize,
    overflow: bool,
}

impl Record {
    fn new() -> Self {
        Self {
            bytes: [0; 192],
            length: 0,
            overflow: false,
        }
    }

    fn text(&mut self, text: &[u8]) {
        if text.len() > self.bytes.len() - self.length {
            self.overflow = true;
            return;
        }
        self.bytes[self.length..self.length + text.len()].copy_from_slice(text);
        self.length += text.len();
    }

    fn number(&mut self, mut value: u64) {
        let mut digits = [0; 20];
        let mut index = digits.len();
        loop {
            index -= 1;
            digits[index] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.text(&digits[index..]);
    }

    fn write(&self) -> Result<(), Error> {
        if self.overflow || debug_write(&self.bytes[..self.length]) != self.length as i64 {
            Err(Error::Output)
        } else {
            Ok(())
        }
    }
}

fn fail(reason: &[u8]) -> ! {
    let mut record = Record::new();
    record.text(b"[http-get] fail: ");
    record.text(reason);
    record.text(b"\n");
    let _ = record.write();
    exit(1)
}
