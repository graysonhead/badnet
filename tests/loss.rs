use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const LOSS_RATE: f64 = 0.3;
const NUM_PACKETS: usize = 500;

#[test]
fn udp_packet_loss() {
    let net = BadNet::builder()
        .seed(42)
        .loss(LOSS_RATE)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let right_addr = net.right_addr();
    let left_addr = net.left_addr();

    // Receiver: count packets until 500 ms of silence.
    let receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{right_addr}:9000")).unwrap();
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

    // Give receiver a moment to bind.
    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left_addr}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender
            .send_to(&i.to_be_bytes(), format!("{right_addr}:9000"))
            .unwrap();
    }

    let received = receiver.join().unwrap();
    let actual_loss = 1.0 - (received as f64 / NUM_PACKETS as f64);

    println!(
        "sent={NUM_PACKETS}  received={received}  loss={:.1}%  (target {:.0}%)",
        actual_loss * 100.0,
        LOSS_RATE * 100.0,
    );

    // Allow ±50% of the target rate (15 %–45 % for a 30 % target).
    assert!(
        actual_loss > LOSS_RATE * 0.5 && actual_loss < LOSS_RATE * 1.5,
        "expected ~{:.0}% loss, got {:.1}%",
        LOSS_RATE * 100.0,
        actual_loss * 100.0,
    );
}
