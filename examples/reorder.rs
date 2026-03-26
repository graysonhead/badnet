/// Demonstrates packet reordering: sequence-numbered packets are sent in order
/// but arrive scrambled.
///
/// Run with:
///   cargo run --example reorder
use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const DELAY: Duration = Duration::from_millis(20);
const REORDER_RATE: f64 = 0.40;
const NUM_PACKETS: usize = 100;

fn main() {
    let net = BadNet::builder()
        .delay(DELAY)
        .reorder(REORDER_RATE)
        .build()
        .expect("requires CAP_NET_ADMIN — grant via setcap (see library docs)");

    let left = net.left_addr();
    let right = net.right_addr();

    println!("Link:       {} <-> {}", left, right);
    println!("Configured: {}ms delay, {:.0}% reorder\n",
             DELAY.as_millis(), REORDER_RATE * 100.0);

    let receiver = std::thread::spawn(move || {
        let sock = UdpSocket::bind(format!("{right}:7200")).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut buf = [0u8; 8];
        let mut seqs: Vec<u64> = Vec::new();
        loop {
            match sock.recv(&mut buf) {
                Ok(_) => seqs.push(u64::from_be_bytes(buf)),
                Err(_) => break,
            }
        }
        seqs
    });

    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender.send_to(&i.to_be_bytes(), format!("{right}:7200")).unwrap();
    }

    let seqs = receiver.join().unwrap();
    let received = seqs.len();

    // Count arrivals where the sequence number went backwards.
    let mut max_seq = 0u64;
    let mut out_of_order = 0usize;
    let mut examples: Vec<(u64, u64)> = Vec::new(); // (expected_next, actual)
    for &seq in &seqs {
        if seq < max_seq {
            out_of_order += 1;
            if examples.len() < 5 {
                examples.push((max_seq, seq));
            }
        } else {
            max_seq = seq;
        }
    }

    println!("Sent:         {}", NUM_PACKETS);
    println!("Received:     {}", received);
    println!("Out-of-order: {}  ({:.1}%)", out_of_order, out_of_order as f64 / received as f64 * 100.0);

    if !examples.is_empty() {
        println!("\nExamples of reordered arrivals (running-max → actual):");
        for (expected, actual) in &examples {
            println!("  seq {} arrived after seq {}", actual, expected);
        }
    }
}
