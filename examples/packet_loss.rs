/// Demonstrates packet loss on a simulated lossy link.
///
/// Run with:
///   cargo run --example packet_loss
use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const LOSS_RATE: f64 = 0.25;
const NUM_PACKETS: usize = 200;

fn main() {
    let net = BadNet::builder()
        .loss(LOSS_RATE)
        .build()
        .expect("requires CAP_NET_ADMIN — grant via setcap (see library docs)");

    let left = net.left_addr();
    let right = net.right_addr();

    println!("Link:       {} <-> {}", left, right);
    println!("Configured: {:.0}% loss", LOSS_RATE * 100.0);
    println!("Sending:    {} packets\n", NUM_PACKETS);

    let receiver = std::thread::spawn(move || {
        let sock = UdpSocket::bind(format!("{right}:7000")).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut buf = [0u8; 8];
        let mut received = 0usize;
        loop {
            match sock.recv(&mut buf) {
                Ok(_) => received += 1,
                Err(_) => break,
            }
        }
        received
    });

    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender.send_to(&i.to_be_bytes(), format!("{right}:7000")).unwrap();
    }

    let received = receiver.join().unwrap();
    let lost = NUM_PACKETS - received;
    let measured = lost as f64 / NUM_PACKETS as f64;

    println!("Sent:       {}", NUM_PACKETS);
    println!("Received:   {}", received);
    println!("Lost:       {}", lost);
    println!("Measured:   {:.1}%  (configured {:.0}%)", measured * 100.0, LOSS_RATE * 100.0);
}
