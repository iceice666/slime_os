#![no_std]

#[path = "../../../components/services/virtio-net-driver/src/reset.rs"]
pub mod link_reset;
#[path = "../../../components/services/network-service/src/loopback.rs"]
pub mod loopback;
#[path = "../../../components/services/network-service/src/tcp.rs"]
pub mod tcp;

#[path = "../../../components/services/network-service/src/dns.rs"]
pub mod dns;
#[path = "../../../components/services/network-service/src/resolver.rs"]
pub mod resolver;
