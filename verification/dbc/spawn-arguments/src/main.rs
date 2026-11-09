//! Host-sim backend for the spawn/v2 argument-framing oracle.
//!
//! Reads the fixture as `zutai-cli json` prints `contracts/spawn-arguments-dbc/v1/fixture.zt`
//! on stdin, drives `slime_proto::spawn_arguments::SpawnArguments` with each case, and prints
//! what the implementation did as an `Observed` value of that contract's `schema.zt`.
//!
//! The adapter reports raw outcomes only. It never computes an expected outcome, a verdict or a
//! violation: `oracle.zt` is the only judge. It must print one observation per fixture step, in
//! order; the oracle refuses a trace with a missing one.

use std::io::Read;

use slime_proto::spawn::{self, WireSpawnFrame, WireSpawnRequest};
use slime_proto::spawn_arguments::SpawnArguments;

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("read the fixture from stdin");
    let fixture = Json::parse(&input).expect("stdin is the JSON projection of the fixture");
    let mut out = String::from("s ::= import \"./schema.zt\";\nobserved :: s.Observed = {\n");
    out.push_str("  schema = s.observedVersion;\n  cases = {\n");
    for case in fixture.get("cases").list() {
        out.push_str(&observe(case));
    }
    out.push_str("  };\n};\nobserved\n");
    print!("{out}");
}

fn observe(case: &Json) -> String {
    let request = request(case.get("request"));
    let Some(mut assembly) = SpawnArguments::new(&request) else {
        return "    { created = false; complete = false; steps = {;}; };\n".into();
    };
    let mut steps = Vec::new();
    for step in case.get("steps").list() {
        let body = step.get("payload");
        steps.push(match step.get("tag").text() {
            "push" => {
                let outcome = match assembly.push(&frame(body)) {
                    Ok(true) => "#completed",
                    Ok(false) => "#accepted",
                    Err(_) => "#refused",
                };
                format!(
                    "#pushed {{ outcome = {outcome}; complete = {}; }};",
                    assembly.is_complete()
                )
            }
            "read" => {
                let read = assembly.argument(body.get("index").int() as usize);
                format!(
                    "#read {{ present = {}; length = {}; }};",
                    read.is_some(),
                    read.map_or(0, <[u8]>::len)
                )
            }
            other => panic!("unknown step tag {other}"),
        });
    }
    let complete = SpawnArguments::new(&request).is_some_and(|fresh| fresh.is_complete());
    let steps = if steps.is_empty() {
        ";".into()
    } else {
        steps.join(" ")
    };
    format!("    {{ created = true; complete = {complete}; steps = {{ {steps} }}; }};\n")
}

fn request(body: &Json) -> WireSpawnRequest {
    let lengths: Vec<u16> = body
        .get("lengths")
        .list()
        .iter()
        .map(|l| l.int() as u16)
        .collect();
    WireSpawnRequest {
        magic: spawn::SPAWN_MAGIC,
        version: spawn::FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: body.get("argumentCount").int() as u8,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 1,
        command: *b"sysinfo\0\0\0\0\0\0\0\0\0",
        argument_len0: lengths[0],
        argument_len1: lengths[1],
        argument_len2: lengths[2],
        argument_len3: lengths[3],
        environment: [0; 8],
        grant_rights: 0,
        argument_bytes: body.get("argumentBytes").int() as u16,
        supervision_handle: 0,
        reserved: [0; 4],
    }
}

fn frame(body: &Json) -> WireSpawnFrame {
    let length = body.get("length").int() as usize;
    let fill = body.get("fill").int();
    let padding = body.get("padding").int() as u8;
    let mut payload = [padding; spawn::FRAME_PAYLOAD_BYTES];
    for (index, byte) in payload.iter_mut().enumerate().take(length) {
        *byte = ((fill + index as i64) % 256) as u8;
    }
    let mut flags = body.get("extraFlags").int() as u16;
    if body.get("final").boolean() {
        flags |= spawn::FRAME_FLAG_FINAL;
    }
    WireSpawnFrame {
        magic: if body.get("magicOk").boolean() {
            spawn::FRAME_MAGIC
        } else {
            spawn::FRAME_MAGIC ^ 1
        },
        version: if body.get("versionOk").boolean() {
            spawn::FORMAT_VERSION
        } else {
            spawn::FORMAT_VERSION + 1
        },
        sequence: body.get("sequence").int() as u16,
        offset: body.get("offset").int() as u16,
        length: length as u16,
        flags,
        payload,
    }
}

/// The subset of JSON `zutai-cli json` prints for a first-order value: objects, arrays,
/// strings without escapes beyond `\"` and `\\`, integers, booleans.
enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    Text(String),
    Int(i64),
    Bool(bool),
}

impl Json {
    fn parse(text: &str) -> Option<Json> {
        let mut cursor = text.as_bytes();
        let value = Self::value(&mut cursor)?;
        skip(&mut cursor);
        cursor.is_empty().then_some(value)
    }

    fn value(cursor: &mut &[u8]) -> Option<Json> {
        skip(cursor);
        match cursor.first()? {
            b'{' => {
                *cursor = &cursor[1..];
                let mut fields = Vec::new();
                loop {
                    skip(cursor);
                    if take(cursor, b'}') {
                        return Some(Json::Object(fields));
                    }
                    let Json::Text(key) = Self::value(cursor)? else {
                        return None;
                    };
                    skip(cursor);
                    take(cursor, b':').then_some(())?;
                    fields.push((key, Self::value(cursor)?));
                    skip(cursor);
                    take(cursor, b',');
                }
            }
            b'[' => {
                *cursor = &cursor[1..];
                let mut items = Vec::new();
                loop {
                    skip(cursor);
                    if take(cursor, b']') {
                        return Some(Json::Array(items));
                    }
                    items.push(Self::value(cursor)?);
                    skip(cursor);
                    take(cursor, b',');
                }
            }
            b'"' => {
                *cursor = &cursor[1..];
                let mut text = Vec::new();
                loop {
                    let (&byte, rest) = cursor.split_first()?;
                    *cursor = rest;
                    match byte {
                        b'"' => return String::from_utf8(text).ok().map(Json::Text),
                        b'\\' => {
                            let (&escaped, rest) = cursor.split_first()?;
                            *cursor = rest;
                            text.push(escaped);
                        }
                        _ => text.push(byte),
                    }
                }
            }
            b't' if cursor.starts_with(b"true") => {
                *cursor = &cursor[4..];
                Some(Json::Bool(true))
            }
            b'f' if cursor.starts_with(b"false") => {
                *cursor = &cursor[5..];
                Some(Json::Bool(false))
            }
            _ => {
                let end = cursor
                    .iter()
                    .position(|b| !(b.is_ascii_digit() || *b == b'-'))
                    .unwrap_or(cursor.len());
                let number = std::str::from_utf8(&cursor[..end]).ok()?.parse().ok()?;
                *cursor = &cursor[end..];
                Some(Json::Int(number))
            }
        }
    }

    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => fields
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or_else(|| panic!("missing field {key}")),
            _ => panic!("field {key} of a non-object"),
        }
    }

    fn list(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            _ => panic!("expected a list"),
        }
    }

    fn text(&self) -> &str {
        match self {
            Json::Text(text) => text,
            _ => panic!("expected text"),
        }
    }

    fn int(&self) -> i64 {
        match self {
            Json::Int(value) => *value,
            _ => panic!("expected an integer"),
        }
    }

    fn boolean(&self) -> bool {
        match self {
            Json::Bool(value) => *value,
            _ => panic!("expected a boolean"),
        }
    }
}

fn skip(cursor: &mut &[u8]) {
    while cursor.first().is_some_and(u8::is_ascii_whitespace) {
        *cursor = &cursor[1..];
    }
}

fn take(cursor: &mut &[u8], byte: u8) -> bool {
    if cursor.first() == Some(&byte) {
        *cursor = &cursor[1..];
        true
    } else {
        false
    }
}
