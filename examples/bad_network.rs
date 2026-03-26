/// Simulates a congested mobile hotspot — loss, corruption, duplication, and
/// reordering all active at once.
///
/// Corruption manifests as loss at the application layer because corrupted
/// packets fail their UDP checksum and are silently dropped by the kernel.
///
/// Run with:
///   cargo run --example bad_network
use std::collections::HashSet;
use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const LOSS_RATE: f64 = 0.10;
const CORRUPT_RATE: f64 = 0.05;
const DUPLICATE_RATE: f64 = 0.08;
const DELAY: Duration = Duration::from_millis(30);
const REORDER_RATE: f64 = 0.15;
const NUM_PACKETS: usize = 500;

fn main() {
    let net = BadNet::builder()
        .loss(LOSS_RATE)
        .corrupt(CORRUPT_RATE)
        .duplicate(DUPLICATE_RATE)
        .delay(DELAY)
        .reorder(REORDER_RATE)
        .build()
        .expect("requires CAP_NET_ADMIN — grant via setcap (see library docs)");

    let left = net.left_addr();
    let right = net.right_addr();

    println!("Link: {} <-> {}", left, right);
    println!("Configured impairments:");
    println!("  loss:      {:.0}%", LOSS_RATE * 100.0);
    println!("  corrupt:   {:.0}%  (shows up as loss — UDP checksum drops corrupt packets)",
             CORRUPT_RATE * 100.0);
    println!("  duplicate: {:.0}%", DUPLICATE_RATE * 100.0);
    println!("  delay:     {}ms + {:.0}% reorder", DELAY.as_millis(), REORDER_RATE * 100.0);
    println!("  sending:   {} packets\n", NUM_PACKETS);

    let receiver = std::thread::spawn(move || {
        let sock = UdpSocket::bind(format!("{right}:7300")).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut buf = [0u8; 8];

        let mut total_received = 0usize;
        let mut seen = HashSet::<u64>::new();
        let mut duplicates = 0usize;
        let mut out_of_order = 0usize;
        let mut max_seq = 0u64;

        loop {
            match sock.recv(&mut buf) {
                Ok(_) => {
                    let seq = u64::from_be_bytes(buf);
                    total_received += 1;
                    if !seen.insert(seq) {
                        duplicates += 1;
                    } else if seq < max_seq {
                        out_of_order += 1;
                    }
                    if seq > max_seq {
                        max_seq = seq;
                    }
                }
                Err(_) => break,
            }
        }

        (total_received, seen.len(), duplicates, out_of_order)
    });

    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender.send_to(&i.to_be_bytes(), format!("{right}:7300")).unwrap();
    }

    let (total_received, unique_received, duplicates, out_of_order) = receiver.join().unwrap();
    let lost = NUM_PACKETS - unique_received;

    println!("Results:");
    println!("  sent:         {:>4}", NUM_PACKETS);
    println!("  received:     {:>4}  ({:.1}% delivery)",
             total_received, total_received as f64 / NUM_PACKETS as f64 * 100.0);
    println!("  unique:       {:>4}  ({:.1}% unique delivery)",
             unique_received, unique_received as f64 / NUM_PACKETS as f64 * 100.0);
    println!("  duplicates:   {:>4}  ({:.1}%)",
             duplicates, duplicates as f64 / NUM_PACKETS as f64 * 100.0);
    println!("  out-of-order: {:>4}  ({:.1}% of received)",
             out_of_order, out_of_order as f64 / total_received as f64 * 100.0);
    println!("  lost+corrupt: {:>4}  ({:.1}%)  configured loss+corrupt = {:.0}%",
             lost,
             lost as f64 / NUM_PACKETS as f64 * 100.0,
             (LOSS_RATE + CORRUPT_RATE) * 100.0);
}
