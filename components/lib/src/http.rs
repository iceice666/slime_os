//! Bounded HTTP/1.x GET framing, independent of transport and allocation.
//!
//! Only an orderly transport EOF may finish a close-delimited body. Callers must
//! use `abort` for reset, timeout, or service loss, and enforce their own total
//! request deadline and partial-write budget. Body callbacks receive binary data;
//! a later framing failure does not retract bytes already streamed.

#[cfg(test)]
#[path = "http/tests.rs"]
mod tests;

pub const MAX_URL_BYTES: usize = 2048;
pub const MAX_LINE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    InvalidUrl,
    BufferTooSmall,
    Malformed,
    Unsupported,
    Limit,
    UnexpectedEof,
    Transport,
    Output,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Url<'a> {
    pub host: &'a str,
    pub port: u16,
    pub authority: &'a str,
    pub path: &'a str,
}

impl<'a> Url<'a> {
    pub fn parse(input: &'a str) -> Result<Self, Error> {
        if input.len() > MAX_URL_BYTES {
            return Err(Error::InvalidUrl);
        }
        let rest = input.strip_prefix("http://").ok_or(Error::InvalidUrl)?;
        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let (host, port) = parse_authority(authority)?;
        validate_path(path)?;
        Ok(Self {
            host,
            port,
            authority,
            path,
        })
    }

    /// Encode one request; the caller must transmit every returned byte, even
    /// when the transport accepts only a prefix in each write.
    pub fn encode_get(&self, output: &mut [u8]) -> Result<usize, Error> {
        let (host, port) = parse_authority(self.authority)?;
        validate_path(self.path)?;
        if host != self.host
            || port != self.port
            || 7 + self.authority.len() + self.path.len() > MAX_URL_BYTES
        {
            return Err(Error::InvalidUrl);
        }
        let pieces = [
            "GET ",
            self.path,
            " HTTP/1.1\r\nHost: ",
            self.authority,
            "\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n",
        ];
        let length = pieces.iter().map(|part| part.len()).sum();
        if output.len() < length {
            return Err(Error::BufferTooSmall);
        }
        let mut offset = 0;
        for part in pieces {
            output[offset..offset + part.len()].copy_from_slice(part.as_bytes());
            offset += part.len();
        }
        Ok(length)
    }
}

fn parse_authority(authority: &str) -> Result<(&str, u16), Error> {
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => {
            if port.is_empty() || port.starts_with('0') || !port.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(Error::InvalidUrl);
            }
            let port = port.parse::<u16>().map_err(|_| Error::InvalidUrl)?;
            (host, port)
        }
        None => (authority, 80),
    };
    if host.is_empty() || host.len() > 253 {
        return Err(Error::InvalidUrl);
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label.as_bytes()[0].is_ascii_alphanumeric()
            || !label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::InvalidUrl);
        }
    }
    Ok((host, port))
}

fn validate_path(path: &str) -> Result<(), Error> {
    if !path.starts_with('/') || path.len() > MAX_URL_BYTES {
        return Err(Error::InvalidUrl);
    }
    let bytes = path.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let high = bytes.get(index + 1).and_then(|b| hex(*b));
            let low = bytes.get(index + 2).and_then(|b| hex(*b));
            let decoded = high.zip(low).map(|(high, low)| high * 16 + low);
            if decoded.is_none_or(|b| b <= 0x20 || b == 0x7f || b == b'\\') {
                return Err(Error::InvalidUrl);
            }
            index += 3;
        } else if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/?".contains(&byte) {
            index += 1;
        } else {
            return Err(Error::InvalidUrl);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Cumulative status/header bytes, including informational responses and CRLF.
    pub header_bytes: usize,
    pub trailer_bytes: usize,
    pub body_bytes: usize,
    pub informational_responses: usize,
    /// Chunk-size/extension lines and chunk-data delimiters, including CRLF.
    pub chunk_metadata_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            header_bytes: 8192,
            trailer_bytes: 2048,
            body_bytes: 1024 * 1024,
            informational_responses: 4,
            chunk_metadata_bytes: 65536,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    NeedMore,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Status,
    Headers,
    Fixed(usize),
    Close,
    ChunkSize,
    ChunkData(usize),
    ChunkCrlf(u8),
    Trailers,
    Complete,
    Failed(Error),
}

pub struct ResponseDecoder {
    limits: Limits,
    state: State,
    line: [u8; MAX_LINE_BYTES],
    line_len: usize,
    headers: usize,
    trailers: usize,
    metadata: usize,
    informational: usize,
    body: usize,
    current_status: u16,
    final_status: Option<u16>,
    content_length: Option<usize>,
    chunked: bool,
    content_encoding: bool,
}

impl Default for ResponseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseDecoder {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            state: State::Status,
            line: [0; MAX_LINE_BYTES],
            line_len: 0,
            headers: 0,
            trailers: 0,
            metadata: 0,
            informational: 0,
            body: 0,
            current_status: 0,
            final_status: None,
            content_length: None,
            chunked: false,
            content_encoding: false,
        }
    }

    pub const fn status(&self) -> Option<u16> {
        self.final_status
    }

    pub const fn body_bytes(&self) -> usize {
        self.body
    }

    pub fn is_complete(&self) -> bool {
        self.state == State::Complete
    }

    pub fn feed(
        &mut self,
        input: &[u8],
        mut output: impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<Progress, Error> {
        let result = self.feed_inner(input, &mut output);
        if let Err(error) = result {
            self.state = State::Failed(error);
        }
        result
    }

    fn feed_inner(
        &mut self,
        mut input: &[u8],
        output: &mut impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<Progress, Error> {
        if let State::Failed(error) = self.state {
            return Err(error);
        }
        while !input.is_empty() {
            match self.state {
                State::Fixed(remaining) | State::ChunkData(remaining) => {
                    let length = remaining.min(input.len());
                    self.emit(&input[..length], output)?;
                    input = &input[length..];
                    self.state = match self.state {
                        State::Fixed(_) if length == remaining => State::Complete,
                        State::Fixed(_) => State::Fixed(remaining - length),
                        _ if length == remaining => State::ChunkCrlf(0),
                        _ => State::ChunkData(remaining - length),
                    };
                }
                State::Close => {
                    self.emit(input, output)?;
                    input = &[];
                }
                State::ChunkCrlf(index) => {
                    add_bounded(&mut self.metadata, 1, self.limits.chunk_metadata_bytes)?;
                    if input[0] != b"\r\n"[usize::from(index)] {
                        return Err(Error::Malformed);
                    }
                    input = &input[1..];
                    self.state = if index == 0 {
                        State::ChunkCrlf(1)
                    } else {
                        State::ChunkSize
                    };
                }
                State::Status | State::Headers | State::ChunkSize | State::Trailers => {
                    match self.state {
                        State::Status | State::Headers => {
                            add_bounded(&mut self.headers, 1, self.limits.header_bytes)?;
                        }
                        State::Trailers => {
                            add_bounded(&mut self.trailers, 1, self.limits.trailer_bytes)?;
                        }
                        _ => add_bounded(&mut self.metadata, 1, self.limits.chunk_metadata_bytes)?,
                    }
                    let byte = input[0];
                    input = &input[1..];
                    if byte == b'\n' {
                        if self.line_len == 0 || self.line[self.line_len - 1] != b'\r' {
                            return Err(Error::Malformed);
                        }
                        self.line_len -= 1;
                        self.finish_line()?;
                        self.line_len = 0;
                    } else {
                        if self.line_len > 0 && self.line[self.line_len - 1] == b'\r' {
                            return Err(Error::Malformed);
                        }
                        if self.line_len == self.line.len() {
                            return Err(Error::Limit);
                        }
                        self.line[self.line_len] = byte;
                        self.line_len += 1;
                    }
                }
                State::Complete => return Err(Error::Malformed),
                State::Failed(error) => return Err(error),
            }
        }
        Ok(self.progress())
    }

    fn emit(
        &mut self,
        bytes: &[u8],
        output: &mut impl FnMut(&[u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        if self.current_status == 205 && !bytes.is_empty() {
            return Err(Error::Malformed);
        }
        add_bounded(&mut self.body, bytes.len(), self.limits.body_bytes)?;
        output(bytes)
    }

    fn finish_line(&mut self) -> Result<(), Error> {
        let line = &self.line[..self.line_len];
        match self.state {
            State::Status => {
                if line.len() < 13
                    || (!line.starts_with(b"HTTP/1.1 ") && !line.starts_with(b"HTTP/1.0 "))
                    || !line[9..12].iter().all(u8::is_ascii_digit)
                    || line[12] != b' '
                    || !line[13..].iter().all(|b| valid_value(*b))
                {
                    return Err(Error::Malformed);
                }
                self.current_status = u16::from(line[9] - b'0') * 100
                    + u16::from(line[10] - b'0') * 10
                    + u16::from(line[11] - b'0');
                if !(100..=599).contains(&self.current_status) {
                    return Err(Error::Malformed);
                }
                if self.current_status == 101 {
                    return Err(Error::Unsupported);
                }
                self.state = State::Headers;
            }
            State::Headers if line.is_empty() => self.finish_headers()?,
            State::Headers => {
                let (name, value) = header(line)?;
                if name.eq_ignore_ascii_case(b"content-length") {
                    if self.content_length.is_some() {
                        return Err(Error::Malformed);
                    }
                    self.content_length = Some(decimal(value)?);
                } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
                    if self.chunked {
                        return Err(Error::Malformed);
                    }
                    if !value.eq_ignore_ascii_case(b"chunked") {
                        return Err(Error::Unsupported);
                    }
                    self.chunked = true;
                } else if name.eq_ignore_ascii_case(b"content-encoding") {
                    if self.content_encoding || !value.eq_ignore_ascii_case(b"identity") {
                        return Err(Error::Unsupported);
                    }
                    self.content_encoding = true;
                } else if name.eq_ignore_ascii_case(b"upgrade") {
                    return Err(Error::Unsupported);
                } else if name.eq_ignore_ascii_case(b"connection") {
                    for token in value.split(|b| *b == b',') {
                        let token = trim(token);
                        if token.is_empty() || !token.iter().all(|b| is_token(*b)) {
                            return Err(Error::Malformed);
                        }
                        if token.eq_ignore_ascii_case(b"upgrade") {
                            return Err(Error::Unsupported);
                        }
                    }
                }
            }
            State::ChunkSize => {
                let length = chunk_size(line)?;
                if length > self.limits.body_bytes.saturating_sub(self.body) {
                    return Err(Error::Limit);
                }
                if self.current_status == 205 && length != 0 {
                    return Err(Error::Malformed);
                }
                self.state = if length == 0 {
                    State::Trailers
                } else {
                    State::ChunkData(length)
                };
            }
            State::Trailers if line.is_empty() => self.state = State::Complete,
            State::Trailers => {
                let (name, _) = header(line)?;
                // Trailer fields cannot change framing, representation decoding,
                // routing, or connection semantics after body bytes were emitted.
                if [
                    b"content-length".as_slice(),
                    b"transfer-encoding",
                    b"content-encoding",
                    b"content-type",
                    b"content-range",
                    b"host",
                    b"connection",
                    b"trailer",
                    b"upgrade",
                ]
                .iter()
                .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
                {
                    return Err(Error::Malformed);
                }
            }
            _ => return Err(Error::Malformed),
        }
        Ok(())
    }

    fn finish_headers(&mut self) -> Result<(), Error> {
        if self.chunked && self.content_length.is_some() {
            return Err(Error::Malformed);
        }
        if self.current_status < 200 {
            if self.chunked || self.content_length.is_some() {
                return Err(Error::Malformed);
            }
            add_bounded(
                &mut self.informational,
                1,
                self.limits.informational_responses,
            )?;
            self.content_encoding = false;
            self.state = State::Status;
            return Ok(());
        }
        self.final_status = Some(self.current_status);
        if self.current_status == 204 {
            if self.chunked || self.content_length.is_some() {
                return Err(Error::Malformed);
            }
            self.state = State::Complete;
        } else if self.current_status == 304 {
            self.state = State::Complete;
        } else if self.chunked {
            self.state = State::ChunkSize;
        } else if let Some(length) = self.content_length {
            if self.current_status == 205 && length != 0 {
                return Err(Error::Malformed);
            }
            if length > self.limits.body_bytes {
                return Err(Error::Limit);
            }
            self.state = if length == 0 {
                State::Complete
            } else {
                State::Fixed(length)
            };
        } else {
            self.state = State::Close;
        }
        Ok(())
    }

    /// This is exclusively for an orderly TCP FIN, never a reset or timeout.
    pub fn finish_eof(&mut self) -> Result<Progress, Error> {
        match self.state {
            State::Close | State::Complete => {
                self.state = State::Complete;
                Ok(Progress::Complete)
            }
            State::Failed(error) => Err(error),
            _ => {
                self.state = State::Failed(Error::UnexpectedEof);
                Err(Error::UnexpectedEof)
            }
        }
    }

    /// Record a transport failure without treating it as close-delimited EOF.
    pub fn abort(&mut self) -> Error {
        let error = match self.state {
            State::Failed(error) => error,
            _ => Error::Transport,
        };
        self.state = State::Failed(error);
        error
    }

    fn progress(&self) -> Progress {
        if self.is_complete() {
            Progress::Complete
        } else {
            Progress::NeedMore
        }
    }
}

fn add_bounded(value: &mut usize, increment: usize, limit: usize) -> Result<(), Error> {
    *value = value
        .checked_add(increment)
        .filter(|v| *v <= limit)
        .ok_or(Error::Limit)?;
    Ok(())
}

fn valid_value(byte: u8) -> bool {
    byte == b'\t' || (byte >= 0x20 && byte != 0x7f)
}

fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

fn trim(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
        value = &value[1..];
    }
    while value.last().is_some_and(|b| matches!(b, b' ' | b'\t')) {
        value = &value[..value.len() - 1];
    }
    value
}

fn header(line: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let colon = line
        .iter()
        .position(|b| *b == b':')
        .ok_or(Error::Malformed)?;
    let name = &line[..colon];
    let value = &line[colon + 1..];
    if name.is_empty()
        || !name.iter().all(|b| is_token(*b))
        || !value.iter().all(|b| valid_value(*b))
    {
        return Err(Error::Malformed);
    }
    Ok((name, trim(value)))
}

fn decimal(bytes: &[u8]) -> Result<usize, Error> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(Error::Malformed);
    }
    bytes.iter().try_fold(0usize, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|v| v.checked_add(usize::from(byte - b'0')))
            .ok_or(Error::Limit)
    })
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn chunk_size(line: &[u8]) -> Result<usize, Error> {
    let digits = line.iter().take_while(|b| b.is_ascii_hexdigit()).count();
    if digits == 0 {
        return Err(Error::Malformed);
    }
    let length = line[..digits].iter().try_fold(0usize, |value, byte| {
        value
            .checked_mul(16)
            .and_then(|v| v.checked_add(usize::from(hex(*byte)?)))
            .ok_or(Error::Limit)
    })?;
    let mut rest = &line[digits..];
    while !rest.is_empty() {
        rest = trim_start(rest);
        if rest.first() != Some(&b';') {
            return Err(Error::Malformed);
        }
        rest = trim_start(&rest[1..]);
        let name = rest.iter().take_while(|b| is_token(**b)).count();
        if name == 0 {
            return Err(Error::Malformed);
        }
        rest = &rest[name..];
        let after_space = trim_start(rest);
        if after_space.first() != Some(&b'=') {
            continue;
        }
        rest = trim_start(&after_space[1..]);
        if rest.first() == Some(&b'"') {
            rest = &rest[1..];
            loop {
                let byte = *rest.first().ok_or(Error::Malformed)?;
                rest = &rest[1..];
                if byte == b'"' {
                    break;
                }
                if byte == b'\\' {
                    let escaped = *rest.first().ok_or(Error::Malformed)?;
                    if !valid_value(escaped) {
                        return Err(Error::Malformed);
                    }
                    rest = &rest[1..];
                } else if !valid_value(byte) {
                    return Err(Error::Malformed);
                }
            }
        } else {
            let value = rest.iter().take_while(|b| is_token(**b)).count();
            if value == 0 {
                return Err(Error::Malformed);
            }
            rest = &rest[value..];
        }
    }
    Ok(length)
}

fn trim_start(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
        bytes = &bytes[1..];
    }
    bytes
}
