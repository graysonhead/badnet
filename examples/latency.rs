/// Measures round-trip latency on a link with a configured delay.
///
/// Each packet travels left→right (50 ms) then right→left (50 ms),
/// so the expected RTT floor is ~100 ms.
///
/// Run with:
///   cargo run --example latency
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use badnet::BadNet;

const DELAY: Duration = Duration::from_millis(50);
const NUM_PINGS: usize = 20;

fn main() {
    let net = BadNet::builder()
        .delay(DELAY)
        .build()
        .expect("requires CAP_NET_ADMIN — grant via setcap (see library docs)");

    let left = net.left_addr();
    let right = net.right_addr();

    println!("Link:       {} <-> {}", left, right);
    println!("Configured: {}ms one-way delay  (expect ~{}ms RTT)\n",
             DELAY.as_millis(), DELAY.as_millis() * 2);

    let left_sock = UdpSocket::bind(format!("{left}:7100")).unwrap();
    let right_sock = UdpSocket::bind(format!("{right}:7100")).unwrap();
    left_sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    right_sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    let mut rtts: Vec<Duration> = Vec::with_capacity(NUM_PINGS);

    for i in 0u64..NUM_PINGS as u64 {
        let payload = i.to_be_bytes();
        let t0 = Instant::now();

        left_sock.send_to(&payload, format!("{right}:7100")).unwrap();

        let mut buf = [0u8; 8];
        right_sock.recv(&mut buf).unwrap();
        right_sock.send_to(&buf, format!("{left}:7100")).unwrap();
        left_sock.recv(&mut buf).unwrap();

        rtts.push(t0.elapsed());
    }

    rtts.sort();

    let min = rtts.first().unwrap();
    let max = rtts.last().unwrap();
    let median = rtts[rtts.len() / 2];
    let p95 = rtts[(rtts.len() as f64 * 0.95) as usize];
    let mean = rtts.iter().sum::<Duration>() / rtts.len() as u32;

    println!("RTT over {} pings:", NUM_PINGS);
    println!("  min:    {:>6.1} ms", ms(min));
    println!("  mean:   {:>6.1} ms", ms(&mean));
    println!("  median: {:>6.1} ms", ms(&median));
    println!("  p95:    {:>6.1} ms", ms(&p95));
    println!("  max:    {:>6.1} ms", ms(max));

    println!("\nHistogram (each bar = 1 packet):");
    let bucket_ms = 5u128;
    let min_bucket = min.as_millis() / bucket_ms * bucket_ms;
    let max_bucket = max.as_millis() / bucket_ms * bucket_ms;
    let mut bucket = min_bucket;
    while bucket <= max_bucket {
        let count = rtts.iter()
            .filter(|r| r.as_millis() / bucket_ms * bucket_ms == bucket)
            .count();
        println!("  {:>4}ms  {}", bucket, "█".repeat(count));
        bucket += bucket_ms;
    }
}

fn ms(d: &Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
