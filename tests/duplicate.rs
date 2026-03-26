use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const DUPLICATE_RATE: f64 = 0.3;
const NUM_PACKETS: usize = 500;

#[test]
fn udp_packet_duplicate() {
    let net = BadNet::builder()
        .seed(42)
        .duplicate(DUPLICATE_RATE)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let right_addr = net.right_addr();
    let left_addr = net.left_addr();

    let receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{right_addr}:9002")).unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let mut buf = [0u8; 8];
        let mut count = 0usize;
        loop {
            match socket.recv(&mut buf) {
                Ok(_) => count += 1,
                Err(_) => break,
            }
        }
        count
    });

    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left_addr}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender
            .send_to(&i.to_be_bytes(), format!("{right_addr}:9002"))
            .unwrap();
    }

    let received = receiver.join().unwrap();
    let actual_dup_rate = (received as f64 - NUM_PACKETS as f64) / NUM_PACKETS as f64;

    println!(
        "sent={NUM_PACKETS}  received={received}  duplicate_rate={:.1}%  (target {:.0}%)",
        actual_dup_rate * 100.0,
        DUPLICATE_RATE * 100.0,
    );

    // Allow ±50% of the target rate (15%–45% for a 30% target).
    assert!(
        actual_dup_rate > DUPLICATE_RATE * 0.5 && actual_dup_rate < DUPLICATE_RATE * 1.5,
        "expected ~{:.0}% duplication, got {:.1}%",
        DUPLICATE_RATE * 100.0,
        actual_dup_rate * 100.0,
    );
}
