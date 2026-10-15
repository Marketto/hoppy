//! Minimal node over hoppy-core for `tools/interop.sh` legs that the CLI does not cover
//! yet (callback, spec §6). Loopback/unicast only (no multicast).
//!
//!   interop_node receive DIR TCP_PORT UDP_PORT      auto-accepts; answers callbacks
//!   interop_node send PEER_UDP [--no-direct] FILE... discovers the peer via PEER_UDP

use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use hoppy_core::transfer::serve;
use hoppy_core::{
    DeviceInfo, Discovery, DiscoveryConfig, Error, Offer, ReceiveHandler, accept_callbacks,
    send_to_peer, send_via_callback,
};

struct AcceptAll;

impl ReceiveHandler for AcceptAll {
    fn decide(&self, _offer: &Offer, _from: SocketAddr) -> bool {
        true
    }
    fn finished(&self, from: SocketAddr, result: &Result<Vec<PathBuf>, Error>) {
        println!("finished from {from}: {result:?}");
    }
}

fn me() -> DeviceInfo {
    DeviceInfo {
        id: "rust-interop-node".into(),
        name: "InteropRustNode".into(),
        platform: "linux".into(),
    }
}

fn config(transfer_port: u16, bind_port: u16, seeds: Vec<SocketAddr>) -> DiscoveryConfig {
    let mut cfg = DiscoveryConfig::new(me(), transfer_port);
    cfg.bind_port = bind_port;
    cfg.use_multicast = false;
    cfg.seeds = seeds;
    cfg
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!("usage: receive DIR TCP_PORT UDP_PORT | send PEER_UDP [--no-direct] FILE...");
        ExitCode::from(2)
    };
    match args.first().map(String::as_str) {
        Some("receive") if args.len() == 4 => {
            let dir = PathBuf::from(&args[1]);
            let (Ok(tcp), Ok(udp)) = (args[2].parse::<u16>(), args[3].parse::<u16>()) else {
                return usage();
            };
            let listener = TcpListener::bind(("0.0.0.0", tcp)).expect("bind TCP");
            let port = listener.local_addr().expect("local addr").port();
            let discovery = Discovery::start(config(port, udp, Vec::new())).expect("discovery");
            let handler: Arc<dyn ReceiveHandler> = Arc::new(AcceptAll);
            accept_callbacks(&discovery, dir.clone(), handler.clone());
            println!("receiving on tcp {port}, udp {}", discovery.local_port());
            serve(listener, dir, handler)
        }
        Some("send") if args.len() >= 3 => {
            let Ok(peer_udp) = args[1].parse::<SocketAddr>() else {
                return usage();
            };
            let direct = args[2] != "--no-direct";
            let files: Vec<PathBuf> = args[if direct { 2 } else { 3 }..]
                .iter()
                .map(PathBuf::from)
                .collect();
            let discovery = Discovery::start(config(0, 0, vec![peer_udp])).expect("discovery");
            let Some(peer) = discovery
                .wait_for_peers(1, Duration::from_secs(5))
                .into_iter()
                .next()
            else {
                eprintln!("no peer answered at {peer_udp}");
                return ExitCode::FAILURE;
            };
            let result = if direct {
                send_to_peer(&peer, &me(), Some(&discovery), &files, &mut |_| {})
            } else {
                send_via_callback(&peer, &me(), &discovery, &files, &mut |_| {})
            };
            println!("{result:?}");
            if matches!(result, Ok(hoppy_core::SendOutcome::Delivered)) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => usage(),
    }
}
