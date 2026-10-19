// muguet - server.rs
// Constantin Clerc (c22dev) - 0.1

use crate::proto::*;
use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

struct Player {
    id: u8,
    name: String,
    last: Instant,
}

pub fn run(port: u16, max: usize) -> std::io::Result<()> {
    let max = max.clamp(2, 32);
    let sock = UdpSocket::bind(("0.0.0.0", port))?;
    sock.set_read_timeout(Some(Duration::from_millis(500)))?;
    println!("[server] listening on UDP port {port} (up to {max} players)");
    for ip in local_ips() {
        println!("[server] players on this network join with {ip}:{port}");
    }

    let mut players: HashMap<SocketAddr, Player> = HashMap::new();
    let mut buf = [0u8; 1500];
    let mut last_sweep = Instant::now();
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some((kind, payload)) = parse(&buf[..n]) {
                    handle(&sock, &mut players, max, from, kind, payload);
                }
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => eprintln!("[server] recv: {e}"),
        }
        if last_sweep.elapsed() >= Duration::from_secs(2) {
            last_sweep = Instant::now();
            let gone: Vec<SocketAddr> = players.iter().filter(|(_, p)| p.last.elapsed() > Duration::from_secs(TIMEOUT_SECS)).map(|(a, _)| *a).collect();
            for addr in gone {
                leave(&sock, &mut players, addr, "timed out");
            }

            for (addr, p) in &players {
                for (to, _) in players.iter().filter(|(a, _)| *a != addr) {
                    let _ = sock.send_to(&name_packet(p), to);
                }
            }
        }
    }
}

fn name_packet(p: &Player) -> Vec<u8> {
    let mut payload = vec![p.id];
    payload.extend(p.name.as_bytes());
    packet(NAME, &payload)
}

fn handle(sock: &UdpSocket, players: &mut HashMap<SocketAddr, Player>, max: usize, from: SocketAddr, kind: u8, payload: &[u8]) {
    match kind {
        HELLO => {
            if let Some(p) = players.get_mut(&from) {
                p.last = Instant::now();
                let _ = sock.send_to(&packet(WELCOME, &[p.id]), from);
                return;
            }
            if players.len() >= max {
                let _ = sock.send_to(&packet(FULL, &[]), from);
                return;
            }
            let id = (1..=255u8).find(|i| !players.values().any(|p| p.id == *i)).unwrap();
            let p = Player { id, name: clean_name(payload), last: Instant::now() };
            println!("[server] {} joined as player {id} from {from} ({} online)", p.name, players.len() + 1);
            let _ = sock.send_to(&packet(WELCOME, &[id]), from);
            for (addr, other) in players.iter() {
                let _ = sock.send_to(&name_packet(other), from);
                let _ = sock.send_to(&name_packet(&p), addr);
            }
            players.insert(from, p);
        }
        kind if relayed(kind) => {
            let Some(p) = players.get_mut(&from) else {
                let _ = sock.send_to(&packet(LEAVE, &[0]), from);
                return;
            };
            p.last = Instant::now();
            let mut out = vec![p.id];
            out.extend_from_slice(payload);
            let pkt = packet(kind, &out);
            for addr in players.keys().filter(|a| **a != from) {
                let _ = sock.send_to(&pkt, addr);
            }
        }
        BYE => leave(sock, players, from, "left"),
        _ => {}
    }
}

fn leave(sock: &UdpSocket, players: &mut HashMap<SocketAddr, Player>, addr: SocketAddr, why: &str) {
    if let Some(p) = players.remove(&addr) {
        println!("[server] {} {why} ({} online)", p.name, players.len());
        for to in players.keys() {
            let _ = sock.send_to(&packet(LEAVE, &[p.id]), to);
        }
    }
}

fn local_ips() -> Vec<String> {
    let run = |cmd: &str, args: &[&str]| std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let mut out = run("/sbin/ifconfig", &[]);
    if out.is_empty() {
        out = run("ip", &["-4", "-o", "addr", "show"]).replace(" inet ", "\ninet ");
    }
    out.lines()
        .filter_map(|l| l.trim().strip_prefix("inet "))
        .filter_map(|l| l.split_whitespace().next())
        .map(|ip| ip.split('/').next().unwrap_or(ip))
        .filter(|ip| !ip.starts_with("127."))
        .map(String::from)
        .collect()
}
