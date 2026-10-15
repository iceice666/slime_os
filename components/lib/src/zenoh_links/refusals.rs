//! The refusals of the links exam. Each row builds the bytes a peer could send, injects them into a pair
//! through the link, and feeds the same bytes to a clone of the receiving session. The two must report the
//! same class, and the clone must be open and unchanged: the link closes on a refusal, a session does not.

use super::*;

/// A PUSH with a Put, field by field, so a row can break exactly one.
#[derive(Clone)]
struct Parts {
    header: u8,
    scope: Vec<u8>,
    key: Option<(Vec<u8>, Vec<u8>)>,
    put: u8,
    ext: u8,
    att_len: Vec<u8>,
    att: Vec<u8>,
    size: Vec<u8>,
    payload: Vec<u8>,
}

impl Parts {
    fn full(key: &[u8], size: usize) -> Self {
        Self {
            header: 0x7d,
            scope: std::vec![0],
            key: Some((codec::leb(key.len() as u64), key.to_vec())),
            put: 0x81,
            ext: 0x43,
            att_len: std::vec![33],
            att: codec::attachment(),
            size: codec::leb(size as u64),
            payload: codec::payload(size),
        }
    }

    fn alias(alias: u32, size: usize) -> Self {
        Self {
            header: 0x5d,
            scope: codec::leb(u64::from(alias)),
            key: None,
            ..Self::full(b"", size)
        }
    }

    fn demo(size: usize) -> Self {
        Self::full(codec::KEY.as_bytes(), size)
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = std::vec![self.header];
        out.extend(&self.scope);
        if let Some((length, key)) = &self.key {
            out.extend(length);
            out.extend(key);
        }
        out.extend([self.put, self.ext]);
        out.extend(&self.att_len);
        out.extend(&self.att);
        out.extend(&self.size);
        out.extend(&self.payload);
        out
    }

    fn with(mut self, change: impl FnOnce(&mut Self)) -> Self {
        change(&mut self);
        self
    }
}

fn frame(sn: u32, messages: &[Vec<u8>]) -> Vec<u8> {
    let mut out = std::vec![0x25];
    out.extend(codec::leb(u64::from(sn)));
    for message in messages {
        out.extend(message);
    }
    out
}

fn declare_alias(id: u32, key: &str) -> Vec<u8> {
    let mut out = std::vec![0x1e, 0x20];
    out.extend(codec::leb(u64::from(id)));
    out.push(0);
    out.extend(codec::leb(key.len() as u64));
    out.extend(key.as_bytes());
    out
}

fn subscriber_frame(bounds: Bounds, sn: u32, id: u32, key: &str) -> Vec<u8> {
    let mut out = std::vec![0u8; bounds.batch + 2];
    let mut frame = FrameEncoder::with_bounds(&mut out, sn, bounds).expect("frame");
    frame
        .declare_subscriber(id, Mapping::Sender, key)
        .expect("declare");
    let length = frame.finish().expect("finish");
    out.truncate(length);
    out
}

fn session_class(error: &SessionError) -> String {
    match error {
        SessionError::Decode(refusal) => refusal.name().into(),
        SessionError::Fragment(error) => error.class().into(),
        SessionError::SequenceMismatch => "sequence-mismatch".into(),
        SessionError::UnexpectedMessage => "unexpected-message".into(),
        SessionError::UndeclaredKey => "undeclared-key".into(),
        SessionError::SubscriberLimit => "subscriber-limit".into(),
        SessionError::OutOfMemory => "out-of-memory".into(),
        other => std::format!("{other:?}"),
    }
}

fn class(error: &Error) -> String {
    match error {
        Error::Session(error) => session_class(error),
        Error::Stream(refusal) => refusal.name().into(),
        other => std::format!("{other:?}"),
    }
}

impl<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
> Pair<F, K, S, B, P, Q>
{
    /// Puts one batch body on the stream `b` reads, behind its stream length.
    fn inject(&self, body: &[u8]) {
        let mut pipe = self.a_to_b.borrow_mut();
        pipe.bytes.extend((body.len() as u16).to_le_bytes());
        pipe.bytes.extend(body.iter());
    }

    /// Pumps `b` until it has read what was injected.
    fn pump_b(&mut self, now: u64) -> Result<(), Error> {
        for _ in 0..64 {
            let report = self.b.pump_with(self.hb, now, &mut self.heap_b)?;
            if report.batches == 0 && !self.b.send_pending() {
                return Ok(());
            }
        }
        Ok(())
    }

    fn next_peer_sn(&self) -> u32 {
        self.b.session().expected_rx_sn()
    }

    /// Declares the peer's alias `id` for `key`, as a stock peer does.
    fn peer_aliases(&mut self, id: u32, key: &str) {
        let sn = self.next_peer_sn();
        self.inject(&frame(sn, &[declare_alias(id, key)]));
        self.pump_b(5).expect("the alias is declared");
    }

    fn drain(&mut self) {
        while self.b.next_notice().is_some() {}
    }

    /// Injects `steps`, then pumps at `pump_at`, and returns the class the link refused them with. The
    /// same steps go to a clone of the session, which must refuse them alike and stay open and unchanged.
    fn refused(&mut self, steps: &[(Vec<u8>, u64)], pump_at: Option<u64>, expect_open: bool) -> String {
        let before = self.b.session().clone();
        let start_sn = before.expected_rx_sn();
        let mut failure = None;
        for (body, at) in steps {
            self.inject(body);
            if let Err(error) = self.pump_b(*at) {
                failure = Some(error);
                break;
            }
        }
        if failure.is_none()
            && let Some(at) = pump_at
            && let Err(error) = self.pump_b(at)
        {
            failure = Some(error);
        }
        let failure = failure.expect("the link refused the run");
        let got = class(&failure);

        let mut session = before.clone();
        let mut out = std::vec![0u8; F];
        let limit = session.params().message_limit;
        let mut cell = std::vec![0u8; if limit == 0 { 0 } else { self.heap_b.len().min(limit) }];
        let mut refusal = None;
        for (body, at) in steps {
            if let Err(error) = session.on_batch_with(body, *at, &mut out, &mut cell) {
                refusal = Some(error);
                break;
            }
        }
        if refusal.is_none()
            && let Some(at) = pump_at
            && let Err(error) = session.tick(at, &mut out)
        {
            refusal = Some(error);
        }
        let Some(error) = refusal else {
            panic!("the session accepted what the link refused as {got}");
        };
        {
            assert_eq!(session_class(&error), got, "the session and the link refuse alike");
            if expect_open {
                assert_eq!(session.state(), before.state(), "the session keeps its state");
                assert!(session.is_open(), "the session stays open");
                assert_eq!(session.expected_rx_sn(), start_sn, "the sequence number does not move");
                assert!(!session.reassembling(), "the run is released");
                assert_eq!(session.subscribers(), before.subscribers(), "the tables are unchanged");
            }
        }
        got
    }
}

const TIME: u64 = 10;
const TIMEOUT: u64 = 5000;

fn bodies(message: &[u8], first_sn: u32) -> Vec<(Vec<u8>, u64)> {
    codec::split(message, 2048, first_sn)
        .into_iter()
        .map(|body| (body, TIME))
        .collect()
}

/// What the receiving session is set up with before the bytes arrive.
#[derive(Clone, Copy)]
struct Setup {
    /// The key the receiver subscribed to.
    sub: &'static str,
    /// The key the peer's alias 1 stands for, if it declared one.
    alias: Option<&'static str>,
    heap: usize,
}

const PLAIN: Setup = Setup { sub: codec::KEY, alias: None, heap: 149_004 };

fn heap_row(setup: Setup, steps: impl FnOnce(u32) -> Vec<(Vec<u8>, u64)>, pump_at: Option<u64>) -> String {
    let mut pair: HeapPair = pair_of(&HEAP, setup.heap);
    pair.open();
    pair.declare(1, setup.sub);
    if let Some(key) = setup.alias {
        pair.peer_aliases(1, key);
    }
    let first = pair.next_peer_sn();
    pair.refused(&steps(first), pump_at, true)
}

fn message_row(setup: Setup, message: Vec<u8>) -> String {
    heap_row(setup, |sn| bodies(&message, sn), None)
}

fn fragment(more: bool, sn: u32, data: &[u8]) -> Vec<u8> {
    let mut out = std::vec![0x26 | if more { 0x40 } else { 0 }];
    out.extend(codec::leb(u64::from(sn)));
    out.extend(data);
    out
}

fn fixed_row(body: &[u8]) -> String {
    let mut pair: Pair<2048, 4, 5, 129, 1024, 8> = pair_of(&LARGE, 0);
    pair.open();
    pair.declare(1, codec::KEY);
    pair.refused(&[(body.to_vec(), TIME)], None, true)
}

/// Declares `count` subscribers or aliases and returns the class of the one after them.
fn limit_row<
    const F: usize,
    const K: usize,
    const S: usize,
    const B: usize,
    const P: usize,
    const Q: usize,
>(
    mut pair: Pair<F, K, S, B, P, Q>,
    count: usize,
    aliases: bool,
) -> String {
    pair.open();
    let make = |pair: &Pair<F, K, S, B, P, Q>, id: u32| {
        let sn = pair.next_peer_sn();
        if aliases {
            frame(sn, &[declare_alias(id, codec::KEY)])
        } else {
            subscriber_frame(pair.bounds, sn, id, codec::KEY)
        }
    };
    for index in 0..count {
        let body = make(&pair, index as u32 + 1);
        pair.inject(&body);
        pair.pump_b(TIME).expect("within the limit");
        pair.drain();
    }
    let body = make(&pair, count as u32 + 1);
    pair.refused(&[(body, TIME)], None, true)
}

fn counts(mode: Mode, keys: usize, subscribers: usize, samples: usize, payload: u64) -> Limits {
    let mut limits = Limits {
        name: "counts",
        mode,
        batch_bytes: 2048,
        payload_bytes: payload,
        key_bytes: 129,
        attachment_bytes: 33,
        max_keys: keys,
        max_subscribers: subscribers,
        outstanding_samples: samples,
        stack_bytes: 0,
        reassembly_timeout_ms: if mode == Mode::Heap { 5000 } else { 0 },
        heap_bytes: 0,
    };
    limits.stack_bytes = limits.required_stack();
    if mode == Mode::Heap {
        limits.heap_bytes = limits.heap_needed();
    }
    limits.check().expect("the contract admits it");
    limits
}

pub fn run(name: &str) -> String {
    let demo = Parts::demo(5000);
    let key = codec::KEY.as_bytes();
    let invalid_key = |text: &[u8]| message_row(PLAIN, Parts::full(text, 5000).bytes());
    let aliased = |key| Setup { alias: Some(key), ..PLAIN };
    match name {
        // A fixed link refuses every fragment as unsupported-message, whatever its form.
        "fixed-refuses-fragment" => fixed_row(&fragment(true, 1000, &[1, 2, 3])),
        "fixed-refuses-best-effort-fragment" => fixed_row(&[0x06, 0xe8, 0x07, 1, 2, 3]),
        "fixed-refuses-fragment-with-extension" => fixed_row(&[0xa6, 0xe8, 0x07, 1, 2, 3]),
        "fixed-refuses-empty-fragment" => fixed_row(&[0x26, 0xe8, 0x07]),
        "fixed-refuses-fragment-sequence-noncanonical" => fixed_row(&[0x26, 0x80, 0x00, 0xaa]),
        "fixed-refuses-fragment-sequence-over-32-bits" => {
            fixed_row(&[0x26, 0xff, 0xff, 0xff, 0xff, 0x10, 0xaa])
        }
        "fixed-refuses-fragment-sequence-truncated" => fixed_row(&[0x26, 0x80]),

        // A fragment is the reliable form and nothing else.
        "fragment-best-effort" => heap_row(PLAIN, |sn| std::vec![(fragment(false, sn, &[1]).with_flag(0x20, false), TIME)], None),
        "fragment-extension" => heap_row(PLAIN, |sn| std::vec![(fragment(false, sn, &[1]).with_flag(0x80, true), TIME)], None),
        "empty-fragment" => heap_row(PLAIN, |sn| std::vec![(fragment(false, sn, &[]), TIME)], None),
        "fragment-before-open" => {
            let mut pair: HeapPair = pair_of(&HEAP, HEAP.heap_bytes as usize);
            pair.refused(&[(fragment(true, 1000, &[1, 2, 3]), TIME)], None, false)
        }
        "fragment-sequence-gap" => heap_row(
            PLAIN,
            |sn| std::vec![(fragment(true, sn, &[1]), TIME), (fragment(true, sn + 2, &[2]), TIME)],
            None,
        ),
        "fragment-sequence-truncated" => heap_row(PLAIN, |_| std::vec![(std::vec![0x26, 0x80], TIME)], None),
        "fragment-sequence-duplicate" => heap_row(
            PLAIN,
            |sn| std::vec![(fragment(true, sn, &[1]), TIME), (fragment(true, sn, &[2]), TIME)],
            None,
        ),
        "fragment-sequence-backward" => heap_row(
            PLAIN,
            |sn| {
                std::vec![
                    (fragment(true, sn, &[1]), TIME),
                    (fragment(true, sn + 1, &[2]), TIME),
                    (fragment(true, sn, &[3]), TIME),
                ]
            },
            None,
        ),
        "fragment-interior-marked-final" => heap_row(
            PLAIN,
            |sn| {
                let mut run = bodies(&demo.bytes(), sn);
                run[0].0[0] &= !0x40;
                run
            },
            None,
        ),
        "fragment-final-marked-more" => heap_row(
            PLAIN,
            |sn| {
                let mut run = bodies(&demo.bytes(), sn);
                for (body, _) in &mut run {
                    body[0] |= 0x40;
                }
                run
            },
            Some(TIME + TIMEOUT + 1),
        ),
        "fragment-stale-first-sequence" => heap_row(PLAIN, |sn| bodies(&demo.bytes(), sn - 1), None),
        "fragment-skipped-first-sequence" => heap_row(PLAIN, |sn| bodies(&demo.bytes(), sn + 1), None),
        "frame-during-reassembly" => heap_row(
            PLAIN,
            |sn| {
                let whole = frame(sn + 1, &[Parts::demo(12).bytes()]);
                std::vec![(fragment(true, sn, &[1, 2, 3]), TIME), (whole, TIME)]
            },
            None,
        ),
        "fragments-of-two-messages-in-one-run" => {
            let mut two = Parts::demo(100).bytes();
            two.extend(Parts::demo(100).bytes());
            message_row(PLAIN, two)
        }
        "best-effort-fragment-during-reassembly" => heap_row(
            PLAIN,
            |sn| {
                std::vec![
                    (fragment(true, sn, &[1]), TIME),
                    (fragment(true, sn + 1, &[2]).with_flag(0x20, false), TIME),
                ]
            },
            None,
        ),

        // The Put inside a fragmented message is held to what a single-batch Put is held to.
        "fragmented-put-without-attachment" => message_row(PLAIN, demo.clone().with(|p| p.put = 0x01).bytes()),
        "fragmented-put-with-short-attachment" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.att_len = std::vec![32];
                p.att.truncate(32);
            }).bytes(),
        ),
        "fragmented-put-gid-length-15" => message_row(PLAIN, demo.clone().with(|p| p.att[16] = 15).bytes()),
        "fragmented-put-gid-length-17" => message_row(PLAIN, demo.clone().with(|p| p.att[16] = 17).bytes()),
        "fragmented-put-attachment-34-bytes" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.att_len = std::vec![34];
                p.att.push(0);
            }).bytes(),
        ),
        "fragmented-put-alias-scope-zero" => message_row(aliased(codec::KEY), Parts::alias(0, 5000).bytes()),
        "fragmented-put-alias-scope-65536" => message_row(
            aliased(codec::KEY),
            Parts::alias(1, 5000).with(|p| p.scope = std::vec![0x80, 0x80, 0x04]).bytes(),
        ),
        "message-over-declared-heap" => message_row(PLAIN, Parts::demo(16_385).bytes()),
        "payload-over-declared-with-enough-storage" => message_row(
            Setup { sub: "a/b", heap: 2 * HEAP.heap_bytes as usize, ..PLAIN },
            Parts::full(b"a/b", 16_385).bytes(),
        ),
        "demo-refuses-payload-13-bytes" => {
            let mut pair: Pair<514, 1, 2, 129, 12, 8> = pair_params(Params::DEMO, 0);
            pair.open();
            pair.declare(1, codec::KEY);
            let attachment = encode::attachment(1, 1001, &codec::GID).expect("attachment");
            assert!(
                matches!(
                    pair.a.publish(pair.ha, codec::KEY, &attachment, &[0u8; 13]),
                    Err(Error::Session(SessionError::Encode(encode::EncodeError::PayloadTooLong)))
                ),
                "the sender refuses it too"
            );
            let sn = pair.next_peer_sn();
            pair.refused(&[(frame(sn, &[Parts::demo(13).bytes()]), TIME)], None, true)
        }
        "large-refuses-payload-1025-bytes" => {
            let mut pair: Pair<2048, 4, 5, 129, 1024, 8> = pair_of(&LARGE, 0);
            pair.open();
            pair.declare(1, codec::KEY);
            let sn = pair.next_peer_sn();
            pair.refused(&[(frame(sn, &[Parts::demo(1025).bytes()]), TIME)], None, true)
        }

        // The key inside a reassembled Put is held to what a whole-frame key is held to.
        "fragmented-put-key-130-bytes" => invalid_key(&[b'k'; 130]),
        "fragmented-put-key-wildcard" => invalid_key(b"a/*/b"),
        "fragmented-put-key-leading-slash" => invalid_key(b"/bad"),
        "fragmented-put-key-double-slash" => invalid_key(b"a//b"),
        "fragmented-put-key-invalid-utf8" => invalid_key(&[0xff, b'a', b'b', b'c']),
        "fragmented-put-key-not-subscribed" => invalid_key(b"slime_demo/other"),
        "fragmented-put-full-key-nonzero-scope" => message_row(PLAIN, demo.clone().with(|p| p.scope = std::vec![1]).bytes()),
        "fragmented-put-empty-key" => message_row(PLAIN, demo.clone().with(|p| p.key = Some((std::vec![0], Vec::new()))).bytes()),
        "fragmented-put-key-trailing-slash" => invalid_key(b"bad/"),
        "fragmented-put-key-question-mark" => invalid_key(b"a?b"),
        "fragmented-put-key-hash" => invalid_key(b"a#b"),
        "fragmented-put-key-dollar" => invalid_key(b"a/b$*"),
        "fragmented-put-key-truncated" => message_row(PLAIN, demo.bytes()[..40].to_vec()),
        "fragmented-push-delete-body" => message_row(PLAIN, demo.clone().with(|p| p.put = 0x02).bytes()),
        "fragmented-push-ends-before-body" => message_row(PLAIN, demo.bytes()[..4 + key.len()].to_vec()),
        "fragmented-put-scope-unterminated" => message_row(PLAIN, std::vec![0x7d, 0x80]),
        "fragmented-put-key-length-unterminated" => message_row(PLAIN, std::vec![0x7d, 0x00, 0x80]),
        "fragmented-put-attachment-length-unterminated" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.att_len = std::vec![0x80];
                p.att.clear();
                p.size.clear();
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-put-payload-length-unterminated" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.size = std::vec![0x80];
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-push-extension-flag" => message_row(PLAIN, demo.clone().with(|p| p.header = 0xfd).bytes()),
        "fragmented-put-timestamp-flag" => message_row(PLAIN, demo.clone().with(|p| p.put = 0xa1).bytes()),
        "fragmented-put-encoding-flag" => message_row(PLAIN, demo.clone().with(|p| p.put = 0xc1).bytes()),
        "fragmented-put-unknown-extension" => message_row(PLAIN, demo.clone().with(|p| p.ext = 0x41).bytes()),
        "fragmented-put-extension-chained" => message_row(PLAIN, demo.clone().with(|p| p.ext = 0xc3).bytes()),
        "fragmented-put-alias-for-another-key" => {
            message_row(aliased("slime_demo/other"), Parts::alias(1, 5000).bytes())
        }
        "fragmented-put-receiver-mapped-alias" => message_row(
            aliased(codec::KEY),
            Parts::alias(1, 5000).with(|p| p.header = 0x1d).bytes(),
        ),

        // Every variable-length integer is read at its width, canonically, and its top bit too.
        "fragmented-put-key-length-over-16-bits" => message_row(
            PLAIN,
            demo.clone().with(|p| p.key = Some((std::vec![0x80, 0x80, 0x04], Vec::new()))).bytes(),
        ),
        "fragmented-put-attachment-length-over-32-bits" => message_row(
            PLAIN,
            demo.clone().with(|p| p.att_len = std::vec![0x80, 0x80, 0x80, 0x80, 0x10]).bytes(),
        ),
        "fragmented-put-scope-256" => message_row(
            aliased(codec::KEY),
            Parts::alias(1, 5000).with(|p| p.scope = std::vec![0x80, 0x02]).bytes(),
        ),
        "fragmented-put-key-length-256" => message_row(
            PLAIN,
            demo.clone().with(|p| p.key = Some((std::vec![0x80, 0x02], Vec::new()))).bytes(),
        ),
        "fragmented-put-attachment-length-65536" => message_row(
            PLAIN,
            demo.clone().with(|p| p.att_len = std::vec![0x80, 0x80, 0x04]).bytes(),
        ),
        "fragmented-put-payload-length-65536" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.size = std::vec![0x80, 0x80, 0x04];
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-put-payload-truncated" => {
            let whole = demo.bytes();
            message_row(PLAIN, whole[..whole.len() - 1].to_vec())
        }
        "fragmented-put-scope-32768" => message_row(
            aliased(codec::KEY),
            Parts::alias(1, 5000).with(|p| p.scope = std::vec![0x80, 0x80, 0x02]).bytes(),
        ),
        "fragmented-put-key-length-32768" => message_row(
            PLAIN,
            demo.clone().with(|p| p.key = Some((std::vec![0x80, 0x80, 0x02], Vec::new()))).bytes(),
        ),
        "fragmented-put-attachment-length-2147483648" => message_row(
            PLAIN,
            demo.clone().with(|p| p.att_len = std::vec![0x80, 0x80, 0x80, 0x80, 0x08]).bytes(),
        ),
        "fragmented-put-payload-length-2147483648" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.size = std::vec![0x80, 0x80, 0x80, 0x80, 0x08];
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-put-attachment-truncated" => message_row(PLAIN, demo.bytes()[..4 + key.len() + 3 + 10].to_vec()),
        "reassembly-one-ms-after-the-timeout" => heap_row(
            PLAIN,
            |sn| {
                let mut run = bodies(&demo.bytes(), sn);
                let last = run.len() - 1;
                run[0].1 = TIME;
                run[last].1 = TIME + TIMEOUT + 1;
                for step in &mut run[1..last] {
                    step.1 = TIME + TIMEOUT - 1;
                }
                run
            },
            None,
        ),
        "fragment-sequence-noncanonical" => heap_row(PLAIN, |_| std::vec![(std::vec![0x26, 0x80, 0x00, 0xaa], TIME)], None),
        "fragment-sequence-over-32-bits" => heap_row(
            PLAIN,
            |_| std::vec![(std::vec![0x26, 0xff, 0xff, 0xff, 0xff, 0x10, 0xaa], TIME)],
            None,
        ),
        "fragmented-put-scope-noncanonical" => message_row(PLAIN, demo.clone().with(|p| p.scope = std::vec![0x80, 0x00]).bytes()),
        "fragmented-put-key-length-noncanonical" => message_row(
            PLAIN,
            demo.clone().with(|p| p.key = Some((std::vec![0x81, 0x00], Vec::new()))).bytes(),
        ),
        "fragmented-put-attachment-length-noncanonical" => message_row(
            PLAIN,
            demo.clone().with(|p| p.att_len = std::vec![0xa1, 0x00]).bytes(),
        ),
        "fragmented-put-payload-length-noncanonical" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.size = std::vec![0x81, 0x00];
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-put-payload-length-over-32-bits" => message_row(
            PLAIN,
            demo.clone().with(|p| {
                p.size = std::vec![0x80, 0x80, 0x80, 0x80, 0x10];
                p.payload.clear();
            }).bytes(),
        ),
        "fragmented-put-undeclared-alias" => message_row(aliased(codec::KEY), Parts::alias(2, 5000).bytes()),
        "fragment-over-the-announced-batch" => heap_row(
            PLAIN,
            |sn| {
                let mut body = fragment(true, sn, &[]);
                body.resize(2047, 7);
                std::vec![(body, TIME)]
            },
            None,
        ),
        "reassembly-timeout" => heap_row(
            PLAIN,
            |sn| bodies(&demo.bytes(), sn).into_iter().take(2).collect(),
            Some(TIME + TIMEOUT + 1),
        ),
        "storage-refused" => message_row(Setup { heap: 4000, ..PLAIN }, demo.bytes()),

        // The tables hold what the configuration declares and refuse the next.
        "fifth-subscriber" => limit_row::<2048, 4, 5, 129, 0, 8>(pair_of(&HEAP, 0), 4, false),
        "fifth-alias" => limit_row::<2048, 4, 5, 129, 0, 8>(pair_of(&HEAP, 0), 4, true),
        "tenth-subscriber-in-nine" => limit_row::<2048, 9, 10, 129, 0, 18>(
            pair_of(&counts(Mode::Heap, 9, 9, 9, 1024), 0),
            9,
            false,
        ),
        "tenth-alias-in-nine" => limit_row::<2048, 9, 10, 129, 0, 18>(
            pair_of(&counts(Mode::Heap, 9, 9, 9, 1024), 0),
            9,
            true,
        ),
        "tenth-subscriber-in-nine-fixed" => limit_row::<2048, 9, 10, 129, 1024, 18>(
            pair_of(&counts(Mode::Fixed, 9, 9, 9, 1024), 0),
            9,
            false,
        ),
        "tenth-alias-in-nine-fixed" => limit_row::<2048, 9, 10, 129, 1024, 18>(
            pair_of(&counts(Mode::Fixed, 9, 9, 9, 1024), 0),
            9,
            true,
        ),
        "257th-subscriber-in-256-heap" => limit_row::<2048, 256, 257, 129, 0, 512>(
            pair_of(&counts(Mode::Heap, 256, 256, 256, 12), 0),
            256,
            false,
        ),
        "257th-alias-in-256-heap" => limit_row::<2048, 256, 257, 129, 0, 512>(
            pair_of(&counts(Mode::Heap, 256, 256, 256, 12), 0),
            256,
            true,
        ),
        "257th-subscriber-in-256-fixed" => limit_row::<2048, 256, 257, 129, 12, 512>(
            pair_of(&counts(Mode::Fixed, 256, 256, 256, 12), 0),
            256,
            false,
        ),
        "257th-alias-in-256-fixed" => limit_row::<2048, 256, 257, 129, 12, 512>(
            pair_of(&counts(Mode::Fixed, 256, 256, 256, 12), 0),
            256,
            true,
        ),

        // A key is counted in encoded bytes: five characters of ten bytes against a declared nine.
        "fragmented-put-multibyte-key-over-declared" => {
            let key = "\u{e9}/\u{65e5}\u{672c}";
            let mut limits = HEAP;
            limits.name = "multibyte";
            limits.batch_bytes = 512;
            limits.payload_bytes = 1500;
            limits.key_bytes = key.len();
            limits.max_keys = 1;
            limits.max_subscribers = 1;
            limits.outstanding_samples = 1;
            limits.stack_bytes = limits.required_stack();
            limits.heap_bytes = limits.heap_needed();
            limits.check().expect("the contract admits it");
            let mut pair: Pair<512, 1, 2, 9, 0, 2> = pair_of(&limits, limits.heap_bytes as usize);
            pair.open();
            pair.declare(1, key);
            let over = std::format!("{key}x");
            assert_eq!((over.chars().count(), over.len()), (5, 10));
            let sn = pair.next_peer_sn();
            let message = Parts::full(over.as_bytes(), 1500).bytes();
            let steps: Vec<(Vec<u8>, u64)> = codec::split(&message, 512, sn)
                .into_iter()
                .map(|body| (body, TIME))
                .collect();
            pair.refused(&steps, None, true)
        }
        other => panic!("no scenario for the refusal {other}"),
    }
}

trait Flagged {
    fn with_flag(self, bit: u8, set: bool) -> Self;
}

impl Flagged for Vec<u8> {
    fn with_flag(mut self, bit: u8, set: bool) -> Self {
        if set {
            self[0] |= bit;
        } else {
            self[0] &= !bit;
        }
        self
    }
}
