"""Bytes a stock eclipse-zenoh 1.0.0 peer put on a TCP link, captured once.

These are the batch bodies (without the 2-byte stream length) that a real Zenoh peer sent
to, and accepted from, an endpoint running this repository's Profile 0 encoder. They are
the independent bytes the stock-peer exam judges against: this repository did not write
them, and no Slime code produced the stock side of any batch.

Provenance, all on one x86_64 Linux host, one run each:

* peer: `eclipse-zenoh==1.0.0` Python wheel
  `eclipse_zenoh-1.0.0-cp38-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl`,
  sha256 `1763912951ecd2a3706f668faf0541245387096b37a4fefb73af7848dadd46ff`, `mode=peer`,
  multicast scouting and gossip disabled, one TCP endpoint, loopback.
* our side: the Profile 0 encoder at `65244f8d` (`INIT_ACK`/`OPEN_ACK` as listener, `INIT_SYN`/
  `OPEN_SYN` and `Put` frames as connector), answering and publishing the demo key and the
  four demo samples with their 33-byte attachments. Our side's batches are not fixtures.
* `connects`: the stock peer connected to us, declared a subscriber and a publisher on the
  demo key, and published the four samples with attachments. We listened.
* `listens`: the stock peer listened with a subscriber on the demo key. We connected, waited
  for its subscriber declaration, and published the four samples.
* `refuse`: batches a stock peer sent that a bounded profile must keep refusing: a client and
  a router `INIT_SYN`, a queryable declaration, a `get` request, a wildcard key declaration,
  and a wildcard declared as a suffix of a declared alias.

Fields that vary between runs are the Zenoh IDs, the frame sequence numbers, the cookie and
the initial sequence number. The batches replay as bytes, so a run is deterministic.

What this does not cover: `rmw_zenoh` itself, liveliness tokens (the 1.0.0 Python binding has
no liveliness API, so none was observed), a router actually forwarding, and any peer other
than this one. See docs/decisions/zenoh-stock-peer.md.
"""

CONNECTS = (
    ("connects-01-init-syn", "c109f1c80817ffa75448d9eec5b0cf34c6059e0ac8ff01"),
    ("connects-02-open-syn", "420ae9f6ef561d736c696d652d73746f636b2d636170747572652d636f6f6b69652d3031"),
    ("connects-03-oam-declare-final", "25e9f6ef56df012108250203000210c80817ffa75448d9eec5b0cf34c6059e020003010008b2b2b2b2b2b2b2b202003e001a"),
    ("connects-04-declare-keyexpr-declare-subscriber", "25eaf6ef569e21082001008101302f736c696d655f64656d6f2f636f756e7465722f736c696d655f64656d6f5f6d7367733a3a6d73673a3a6464735f3a3a436f756e7465725f2f5249485330315f613832666435666663623936643061313937613561643336383064316334653662613433613936323932386563643539326662353635656238313239353935629e2108420101"),
    ("connects-05-push-1", "25ebf6ef565d018143210000000000000000e80300000000000010e0e1e2e3e4e5e6e7e8e9eaebecedeeef0c00010000000000000a000000"),
    ("connects-06-push-2", "25ecf6ef565d018143210100000000000000e90300000000000010e0e1e2e3e4e5e6e7e8e9eaebecedeeef0c000100000100000014000000"),
    ("connects-07-push-3", "25edf6ef565d018143210200000000000000ea0300000000000010e0e1e2e3e4e5e6e7e8e9eaebecedeeef0c00010000020000001e000000"),
    ("connects-08-push-4", "25eef6ef565d018143210300000000000000eb0300000000000010e0e1e2e3e4e5e6e7e8e9eaebecedeeef0c000100000300000028000000"),
    ("connects-09-keep-alive-1", "04"),
    ("connects-10-keep-alive-2", "04"),
    ("connects-11-close", "0300"),
)

LISTENS = (
    ("listens-01-init-ack", "6109f1ea220ec0a35d4de9b4618ecd68ccbbaa0a00022120d424560bc9b22a8dbed0918e284fe09529b5461465b69d0178a79fe7124f1763"),
    ("listens-02-open-ack", "620ab8be9957"),
    ("listens-03-oam", "25b8be9957df012108250203000210ea220ec0a35d4de9b4618ecd68ccbbaa020003010008a1a1a1a1a1a1a1a10200"),
    ("listens-04-declare-keyexpr", "25b9be99579e21082001008101302f736c696d655f64656d6f2f636f756e7465722f736c696d655f64656d6f5f6d7367733a3a6d73673a3a6464735f3a3a436f756e7465725f2f5249485330315f61383266643566666362393664306131393761356164333638306431633465366261343361393632393238656364353932666235363565623831323935393562"),
    ("listens-05-declare-subscriber-declare-final", "25babe99579e21084201013e001a"),
    ("listens-06-keep-alive-1", "04"),
    ("listens-07-keep-alive-2", "04"),
    ("listens-08-close", "0300"),
)

REFUSE = (
    ("refuse-client", "c109f2d374f022dce5bb49582e20fdeb66f66e0ac8ff01"),
    ("refuse-router", "c109f071a611afd4989829c8c12c53be70316e0ac8ff01"),
    ("refuse-queryable", "25e8ebfe5d9e21082001000c736c696d652f64656d6f2f719e2108440101"),
    ("refuse-request", "25e9d19c12fc01000c736c696d652f64656d6f2f71a10d26f4032303"),
    ("refuse-wildcard", "25c9f4bb679e21082001000a736c696d652f64656d6f9e2108620101032f2a2a"),
)

# The stock peer's keep-alive is the single byte 0x04. On a 10 s lease it sent one every 2.5 s
# and on a 2 s lease every 0.5 s, so every lease/4. A peer that heard nothing from us closed the
# connection one lease after our last transmission: 2.002 s after it on a 2 s lease and 10.002 s
# on a 10 s lease. Sending its own keep-alives every 0.7 s kept a 2 s-lease session open for the
# whole 9 s run.
KEEP_ALIVE_BODY = "04"

# The cookie our side issued in the `connects` capture. The stock peer echoed it in its
# OPEN_SYN, so a listener replaying the capture must be configured with it. 29 bytes.
COOKIE = "slime-stock-capture-cookie-01"

# The exam, by name. A Rust test prints `[zenoh-exam] stock ok <name>` for each of OK and
# `[zenoh-exam] stock refused <name> <class>` for each of REFUSED; the checker owns both lists.
OK_SCENARIOS = (
    "stock-connects-open-and-deliver",
    "stock-listens-open-and-publish",
    "alias-resolves-to-declared-key",
    "keep-alive-renews-lease",
    "keep-alive-sent-lease-2000",
    "keep-alive-sent-lease-10000",
    "silence-past-lease-closes",
)
REFUSED_SCENARIOS = (
    ("refuse-client", "unsupported-whatami"),
    ("refuse-router", "unsupported-whatami"),
    ("refuse-queryable", "unsupported-message"),
    ("refuse-request", "unsupported-message"),
    ("refuse-wildcard", "wildcard-keyexpr"),
    ("alias-undeclared", "invalid-keyexpr"),
    ("alias-second", "over-bound"),
    ("alias-redeclared-to-other-key", "invalid-keyexpr"),
    ("extension-other-than-qos", "unsupported-extension"),
    ("transport-qos-marked-mandatory", "unsupported-extension"),
    ("network-qos-marked-mandatory", "unsupported-extension"),
    ("qos-extension-on-open", "unsupported-extension"),
    ("qos-extension-on-init-ack", "unsupported-extension"),
)


def replay_names() -> tuple[str, ...]:
    """`replay:<fixture>` for every batch the stock peer sent while connecting or listening."""
    return tuple(f"replay:{name}" for group in ("connects", "listens") for name, _ in batches(group))


def batches(group: str) -> tuple[tuple[str, str], ...]:
    return {"connects": CONNECTS, "listens": LISTENS, "refuse": REFUSE}[group]


def fixture_zti() -> str:
    """The captured batches as one `contracts/zenoh-stock-fixtures/v1` record, in capture order.

    This is the hand-off to the Rust exam. The record is validated against the contract's
    schema before the tests see it, so a reader never parses a bespoke text format.
    """

    def rows(group: str) -> str:
        return "".join(f'    {{ name = "{name}"; hex = "{hexed}"; }};\n' for name, hexed in batches(group))

    return (
        "{\n"
        "  formatVersion = 1;\n"
        f'  cookieHex = "{COOKIE.encode().hex()}";\n'
        f"  connects = [\n{rows('connects')}  ];\n"
        f"  listens = [\n{rows('listens')}  ];\n"
        f"  refuse = [\n{rows('refuse')}  ];\n"
        "}\n"
    )
