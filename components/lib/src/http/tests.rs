use super::*;

fn decode_parts(wire: &[u8], width: usize, eof: bool) -> Result<(u16, Vec<u8>), Error> {
    let mut parser = ResponseDecoder::new();
    let mut body = Vec::new();
    for part in wire.chunks(width) {
        parser.feed(part, |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        })?;
    }
    if eof {
        parser.finish_eof()?;
    }
    assert!(parser.is_complete());
    assert_eq!(parser.body_bytes(), body.len());
    Ok((parser.status().unwrap(), body))
}

fn rejects(wire: &[u8]) {
    for width in [1, 2, 7, wire.len()] {
        let mut parser = ResponseDecoder::new();
        let mut error = None;
        for part in wire.chunks(width) {
            if let Err(found) = parser.feed(part, |_| Ok(())) {
                error = Some(found);
                break;
            }
        }
        let error = error
            .or_else(|| parser.finish_eof().err())
            .expect("invalid response accepted");
        assert_eq!(
            parser.feed(b"HTTP/1.1 204 OK\r\n\r\n", |_| Ok(())),
            Err(error)
        );
        assert_eq!(parser.finish_eof(), Err(error));
        assert!(!parser.is_complete());
    }
}

#[test]
fn urls_and_exact_requests() {
    for (input, host, port, path, authority) in [
        ("http://example.com", "example.com", 80, "/", "example.com"),
        ("http://EXAMPLE.com/", "EXAMPLE.com", 80, "/", "EXAMPLE.com"),
        (
            "http://example.com:8080/a?q=x%3Fy",
            "example.com",
            8080,
            "/a?q=x%3Fy",
            "example.com:8080",
        ),
        (
            "http://host-name:65535/abc",
            "host-name",
            65535,
            "/abc",
            "host-name:65535",
        ),
        ("http://127.0.0.1:80/", "127.0.0.1", 80, "/", "127.0.0.1:80"),
    ] {
        let url = Url::parse(input).unwrap();
        assert_eq!(
            (url.host, url.port, url.path, url.authority),
            (host, port, path, authority)
        );
        let mut request = [0u8; 4096];
        let length = url.encode_get(&mut request).unwrap();
        assert_eq!(&request[..length], format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\nAccept-Encoding: identity\r\n\r\n").as_bytes());
        assert_eq!(
            url.encode_get(&mut request[..length - 1]),
            Err(Error::BufferTooSmall)
        );
    }
}

#[test]
fn malformed_and_ambiguous_urls_are_rejected() {
    for input in [
        "",
        "http://",
        "https://example.com/",
        "HTTP://example.com/",
        "//example.com/",
        "http://user@example.com/",
        "http://example.com#frag",
        "http://example.com/?a#frag",
        "http://example.com?q=x",
        "http://[::1]/",
        "http://a..b/",
        "http://a./",
        "http://-a/",
        "http://a-/",
        "http://a_b/",
        "http://é.com/",
        "http://a:0/",
        "http://a:01/",
        "http://a:65536/",
        "http://a:+80/",
        "http://a:80:80/",
        "http://a:/",
        "http://a/\r\nHost: evil",
        "http://a/has space",
        "http://a/\t",
        "http://a/\\evil",
        "http://a/%0d%0aX:evil",
        "http://a/%00",
        "http://a/%7f",
        "http://a/%5C",
        "http://a/%",
        "http://a/%1",
        "http://a/%gg",
        "http://a/é",
        "http://a/\u{7f}",
    ] {
        assert_eq!(Url::parse(input), Err(Error::InvalidUrl), "{input:?}");
    }
    assert!(Url::parse(&format!("http://{}/", "a".repeat(64))).is_err());
    assert!(Url::parse(&format!("http://{}/", vec!["a".repeat(63); 4].join("."))).is_err());
    assert!(Url::parse(&format!("http://a/{}", "a".repeat(MAX_URL_BYTES))).is_err());
    let mut forged = Url::parse("http://example.com/").unwrap();
    forged.path = "/\r\nInjected: yes";
    assert_eq!(forged.encode_get(&mut [0; 4096]), Err(Error::InvalidUrl));
    forged.path = "/";
    forged.port = 81;
    assert_eq!(forged.encode_get(&mut [0; 4096]), Err(Error::InvalidUrl));
}

#[test]
fn all_single_split_positions_work() {
    for wire in [
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 5\r\nX: y\r\n\r\na\0b\xffc".as_slice(),
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;foo=bar\r\nab\r\n3 ; baz = \"x\\\"y\"\r\ncde\r\n0\r\nX-Sum: five\r\n\r\n",
    ] {
        let expected = decode_parts(wire, wire.len(), false).unwrap();
        for split in 0..=wire.len() {
            let mut parser = ResponseDecoder::new();
            let mut body = Vec::new();
            for part in [&wire[..split], &wire[split..]] {
                parser.feed(part, |bytes| {
                    body.extend_from_slice(bytes);
                    Ok(())
                }).unwrap();
            }
            assert_eq!(parser.status(), Some(expected.0));
            assert!(parser.is_complete());
            assert_eq!(body, expected.1, "split {split}");
        }
        for width in 1..=wire.len() {
            assert_eq!(decode_parts(wire, width, false).unwrap(), expected);
        }
    }
}

#[test]
fn body_larger_than_payload_page_is_streamed_in_all_framings() {
    let body: Vec<u8> = (0..10003).map(|index| (index % 256) as u8).collect();
    let mut fixed =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    fixed.extend_from_slice(&body);
    let mut chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for part in body.chunks(4096) {
        chunked.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
        chunked.extend_from_slice(part);
        chunked.extend_from_slice(b"\r\n");
    }
    chunked.extend_from_slice(b"0\r\n\r\n");
    let mut close = b"HTTP/1.0 500 Internal Server Error\r\n\r\n".to_vec();
    close.extend_from_slice(&body);
    for width in [1, 3, 100, 4096, 16384] {
        assert_eq!(
            decode_parts(&fixed, width, false).unwrap(),
            (200, body.clone())
        );
        assert_eq!(
            decode_parts(&chunked, width, false).unwrap(),
            (200, body.clone())
        );
        assert_eq!(
            decode_parts(&close, width, true).unwrap(),
            (500, body.clone())
        );
    }
}

#[test]
fn no_body_statuses_and_zero_lengths() {
    for wire in [
        b"HTTP/1.1 204 No Content\r\n\r\n".as_slice(),
        b"HTTP/1.1 304 Not Modified\r\nContent-Length: 2000000\r\n\r\n",
        b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 205 Reset Content\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 205 Reset Content\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        b"HTTP/1.1 205 Reset Content\r\n\r\n",
        b"HTTP/1.1 301 Moved Permanently\r\nContent-Length: 0\r\nLocation: http://evil/\r\n\r\n",
        b"HTTP/1.1 599 \r\nContent-Length: 0\r\n\r\n",
    ] {
        assert!(decode_parts(wire, 1, true).unwrap().1.is_empty());
    }
}

#[test]
fn malformed_status_and_headers_fail_terminally() {
    for wire in [
        b"HTTP/2 200 OK\r\n\r\n".as_slice(),
        b"HTTP/1.1 099 Invalid\r\n\r\n",
        b"HTTP/1.1 600 Invalid\r\n\r\n",
        b"HTTP/1.1 200\r\n\r\n",
        b"HTTP/1.1 20a OK\r\n\r\n",
        b"HTTP/1.1 200 Bad\0Status\r\n\r\n",
        b"HTTP/1.1 200 OK\n\n",
        b"HTTP/1.1 200 OK\rX\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nFold: a\r\n b\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length : 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\n: value\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nX: a\0b\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nMissingColon\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nConnection: ,close\r\n\r\n",
    ] {
        rejects(wire);
    }
}

#[test]
fn conflicting_or_unsupported_framing_is_rejected() {
    for headers in [
        "Content-Length: 1\r\nContent-Length: 2",
        "Content-Length: 1\r\nContent-Length: 1",
        "Content-Length: 1, 1",
        "Content-Length: +1",
        "Content-Length: -1",
        "Content-Length: ",
        "Content-Length: 999999999999999999999999999999999999999999999999",
        "Content-Length: 0\r\nTransfer-Encoding: chunked",
        "Transfer-Encoding: gzip, chunked",
        "Transfer-Encoding: chunked, chunked",
        "Transfer-Encoding: identity",
        "Transfer-Encoding: chunked\r\nTransfer-Encoding: chunked",
        "Transfer-Encoding: chunked; x=y",
        "Content-Encoding: gzip",
        "Content-Encoding: br",
        "Content-Encoding: identity, gzip",
        "Content-Encoding: identity\r\nContent-Encoding: identity",
        "Upgrade: h2c",
        "Connection: keep-alive, Upgrade",
    ] {
        rejects(format!("HTTP/1.1 200 OK\r\n{headers}\r\n\r\n0\r\n\r\n").as_bytes());
    }
    for wire in [
        b"HTTP/1.1 101 Switching Protocols\r\n\r\n".as_slice(),
        b"HTTP/1.1 100 Continue\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 103 Early Hints\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 204 No Content\r\nTransfer-Encoding: chunked\r\n\r\n",
        b"HTTP/1.1 205 Reset Content\r\nContent-Length: 1\r\n\r\na",
        b"HTTP/1.1 205 Reset Content\r\nTransfer-Encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\n",
        b"HTTP/1.1 205 Reset Content\r\n\r\na",
    ] {
        rejects(wire);
    }
}

#[test]
fn chunk_extensions_and_trailers_are_bounded_and_validated() {
    for chunks in [
        "-1\r\nx\r\n0\r\n\r\n",
        "+1\r\nx\r\n0\r\n\r\n",
        "0x1\r\nx\r\n0\r\n\r\n",
        " 1\r\nx\r\n0\r\n\r\n",
        "1 \r\nx\r\n0\r\n\r\n",
        "1;\r\nx\r\n0\r\n\r\n",
        "1;a=\r\nx\r\n0\r\n\r\n",
        "1;a=\"unterminated\r\nx\r\n0\r\n\r\n",
        "1;a=\"nul\0\"\r\nx\r\n0\r\n\r\n",
        "1\r\nxXX0\r\n\r\n",
        "FFFFFFFFFFFFFFFFFFFFFFFFFFFFF\r\n",
        "0\r\nContent-Length: 0\r\n\r\n",
        "0\r\nTransfer-Encoding: chunked\r\n\r\n",
        "0\r\nContent-Encoding: gzip\r\n\r\n",
        "0\r\nHost: evil\r\n\r\n",
        "0\r\nBad trailer\r\n\r\n",
        "0\r\n Fold: value\r\n\r\n",
    ] {
        rejects(
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunks}").as_bytes(),
        );
    }
    assert_eq!(decode_parts(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\nContent-Encoding: identity\r\n\r\n1;flag;token=value;quoted=\"a\\\"b\"\r\nx\r\n0 ; end = yes\r\nX-Checksum: abc\r\n\r\n", 1, false).unwrap().1, b"x");
}

#[test]
fn every_early_eof_of_framed_response_fails() {
    for wire in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabcde".as_slice(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nabcde\r\n0\r\nX: y\r\n\r\n",
    ] {
        for end in 0..wire.len() {
            let mut parser = ResponseDecoder::new();
            parser.feed(&wire[..end], |_| Ok(())).unwrap();
            assert_eq!(parser.finish_eof(), Err(Error::UnexpectedEof), "end={end}");
            assert_eq!(
                parser.feed(&wire[end..], |_| Ok(())),
                Err(Error::UnexpectedEof)
            );
        }
    }
}

#[test]
fn close_requires_orderly_eof_and_reset_is_never_success() {
    for wire in [
        b"HTTP/1.1 200 OK\r\n\r\nabc".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc",
    ] {
        let mut parser = ResponseDecoder::new();
        assert_eq!(parser.feed(wire, |_| Ok(())), Ok(Progress::NeedMore));
        assert_eq!(parser.abort(), Error::Transport);
        assert_eq!(parser.finish_eof(), Err(Error::Transport));
        assert_eq!(parser.feed(b"", |_| Ok(())), Err(Error::Transport));
    }
    let mut parser = ResponseDecoder::new();
    parser
        .feed(b"HTTP/1.1 200 OK\r\n\r\nabc", |_| Ok(()))
        .unwrap();
    assert_eq!(parser.finish_eof(), Ok(Progress::Complete));
}

#[test]
fn output_failure_is_terminal_and_extra_bytes_are_rejected() {
    let mut parser = ResponseDecoder::new();
    assert_eq!(
        parser.feed(b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx", |_| Err(
            Error::Output
        )),
        Err(Error::Output)
    );
    assert_eq!(parser.finish_eof(), Err(Error::Output));
    assert_eq!(
        parser.feed(b"x", |_| panic!("callback after failure")),
        Err(Error::Output)
    );
    rejects(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\nx");
    rejects(b"HTTP/1.1 204 No Content\r\n\r\nx");
    rejects(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\nx");
}

#[test]
fn all_resource_limits_are_enforced() {
    let cases = [
        (
            Limits {
                header_bytes: 18,
                ..Limits::default()
            },
            "HTTP/1.1 204 OK\r\n\r\n",
        ),
        (
            Limits {
                body_bytes: 2,
                ..Limits::default()
            },
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc",
        ),
        (
            Limits {
                body_bytes: 2,
                ..Limits::default()
            },
            "HTTP/1.1 200 OK\r\n\r\nabc",
        ),
        (
            Limits {
                body_bytes: 2,
                ..Limits::default()
            },
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ),
        (
            Limits {
                trailer_bytes: 7,
                ..Limits::default()
            },
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX: y\r\n\r\n",
        ),
        (
            Limits {
                chunk_metadata_bytes: 7,
                ..Limits::default()
            },
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n0\r\n\r\n",
        ),
        (
            Limits {
                informational_responses: 1,
                ..Limits::default()
            },
            "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Hints\r\n\r\nHTTP/1.1 204 OK\r\n\r\n",
        ),
    ];
    for (limits, wire) in cases {
        for width in [1, wire.len()] {
            let mut parser = ResponseDecoder::with_limits(limits);
            let result = wire
                .as_bytes()
                .chunks(width)
                .try_for_each(|part| parser.feed(part, |_| Ok(())).map(|_| ()));
            assert_eq!(result, Err(Error::Limit), "{limits:?} {wire:?}");
            assert_eq!(parser.finish_eof(), Err(Error::Limit));
        }
    }
    rejects(
        format!(
            "HTTP/1.1 200 OK\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_LINE_BYTES)
        )
        .as_bytes(),
    );
    rejects(
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1;a={}\r\nx\r\n0\r\n\r\n",
            "a".repeat(MAX_LINE_BYTES)
        )
        .as_bytes(),
    );
    rejects(
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX: {}\r\n\r\n",
            "a".repeat(MAX_LINE_BYTES)
        )
        .as_bytes(),
    );
}

#[test]
fn cumulative_limits_cannot_be_reset_by_informational_or_chunk_boundaries() {
    let mut parser = ResponseDecoder::with_limits(Limits {
        header_bytes: 45,
        ..Limits::default()
    });
    assert_eq!(
        parser.feed(
            b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nX: a\r\n\r\n",
            |_| Ok(())
        ),
        Err(Error::Limit)
    );
    let mut parser = ResponseDecoder::with_limits(Limits {
        body_bytes: 1,
        ..Limits::default()
    });
    assert_eq!(
        parser.feed(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\nx\r\n1\r\ny\r\n0\r\n\r\n",
            |_| Ok(())
        ),
        Err(Error::Limit)
    );
}

fn terminal_outcome(wire: &[u8], split: usize) -> (Result<Progress, Error>, Option<u16>, Vec<u8>) {
    let mut parser = ResponseDecoder::new();
    let mut body = Vec::new();
    let mut result = Ok(Progress::NeedMore);
    for part in [&wire[..split], &wire[split..]] {
        if result.is_err() {
            break;
        }
        result = parser.feed(part, |bytes| {
            body.extend_from_slice(bytes);
            Ok(())
        });
    }
    if result.is_ok() {
        result = parser.finish_eof();
    }
    if let Err(error) = result {
        assert!(!parser.is_complete());
        assert_eq!(
            parser.feed(b"", |_| panic!("callback after error")),
            Err(error)
        );
        assert_eq!(parser.abort(), error);
        assert_eq!(parser.finish_eof(), Err(error));
    } else {
        assert!(parser.is_complete());
        assert_eq!(parser.body_bytes(), body.len());
        assert!(parser.status().is_some());
    }
    (result, parser.status(), body)
}

#[test]
fn deterministic_arbitrary_bytes_and_response_mutations_are_fragmentation_invariant() {
    let seeds: &[&[u8]] = &[
        b"",
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;a=\"x\\\"y\"\r\nabc\r\n0\r\nX: y\r\n\r\n",
        b"HTTP/1.0 500 Error\r\n\r\nabc",
        b"HTTP/1.1 103 Hints\r\nLink: </x>\r\n\r\nHTTP/1.1 304 Not Modified\r\nContent-Length: 3\r\n\r\n",
    ];
    let mut random = 0x7192_a4d5u32;
    for case in 0..1024 {
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            random
        };
        let seed = seeds[case % seeds.len()];
        let mut wire = seed.to_vec();
        match case % 4 {
            0 => {
                let length = (next() % 192) as usize;
                wire.extend((0..length).map(|_| next() as u8));
            }
            1 if !wire.is_empty() => {
                let index = next() as usize % wire.len();
                wire[index] = next() as u8;
            }
            2 => wire.truncate(next() as usize % (wire.len() + 1)),
            _ => {
                let index = next() as usize % (wire.len() + 1);
                wire.insert(index, next() as u8);
            }
        }
        let expected = terminal_outcome(&wire, wire.len());
        for split in [
            0,
            1.min(wire.len()),
            wire.len() / 2,
            wire.len().saturating_sub(1),
        ] {
            assert_eq!(
                terminal_outcome(&wire, split),
                expected,
                "case={case} split={split}"
            );
        }
    }
}

#[test]
fn arbitrary_utf8_urls_never_encode_unvalidated_request_delimiters() {
    let alphabet = b"http:/@0123456789example.-_%\\\r\n\t ?#[]:";
    let mut random = 0x9317_54d1u32;
    for case in 0..1024 {
        let mut text = if case % 2 == 0 {
            "http://example.com/".to_owned()
        } else {
            String::new()
        };
        for _ in 0..case % 80 {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            text.push(char::from(alphabet[random as usize % alphabet.len()]));
        }
        if let Ok(url) = Url::parse(&text) {
            let mut bytes = [0; MAX_URL_BYTES + 256];
            let length = url.encode_get(&mut bytes).unwrap();
            let request = core::str::from_utf8(&bytes[..length]).unwrap();
            assert_eq!(request.matches("\r\n").count(), 5);
            assert!(request.starts_with("GET /"));
            assert_eq!(url, Url::parse(&text).unwrap());
        }
    }
}

#[test]
fn exact_line_header_body_and_trailer_limits_accept_the_boundary() {
    let wire = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc";
    let header_length = wire.len() - 3;
    let mut parser = ResponseDecoder::with_limits(Limits {
        header_bytes: header_length,
        body_bytes: 3,
        ..Limits::default()
    });
    assert_eq!(parser.feed(wire, |_| Ok(())), Ok(Progress::Complete));
    let line = format!("X:{}\r\n", "a".repeat(MAX_LINE_BYTES - 3));
    let wire = format!(
        "HTTP/1.1 200 OK\r\n{line}Transfer-Encoding: chunked\r\n\r\n1;flag\r\nx\r\n0\r\nX:y\r\n\r\n"
    );
    let mut parser = ResponseDecoder::with_limits(Limits {
        trailer_bytes: 7,
        chunk_metadata_bytes: 13,
        body_bytes: 1,
        ..Limits::default()
    });
    assert_eq!(
        parser.feed(wire.as_bytes(), |_| Ok(())),
        Ok(Progress::Complete)
    );
}

#[test]
fn informational_only_and_no_body_metadata_do_not_bypass_framing_validation() {
    for status in [100, 103, 204, 304] {
        rejects(format!("HTTP/1.1 {status} Status\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n").as_bytes());
        rejects(format!("HTTP/1.1 {status} Status\r\nContent-Encoding: gzip\r\n\r\n").as_bytes());
        rejects(
            format!("HTTP/1.1 {status} Status\r\nTransfer-Encoding: gzip, chunked\r\n\r\n")
                .as_bytes(),
        );
    }
    rejects(b"HTTP/1.1 103 Early Hints\r\n\r\n");
    let wire = b"HTTP/1.1 100 Continue\r\n\r\n".repeat(5);
    assert_eq!(terminal_outcome(&wire, wire.len()).0, Err(Error::Limit));
}
