use std::net::UdpSocket;
use std::time::Duration;

use badnet::BadNet;

const LOSS_RATE: f64 = 0.5;
const NUM_PACKETS: usize = 500;

#[test]
fn loss_does_not_affect_unrelated_connection() {
    let lossy = BadNet::builder()
        .seed(42)
        .loss(LOSS_RATE)
        .build()
        .expect("failed to create lossy BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let clean = BadNet::builder()
        .build()
        .expect("failed to create clean BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let lossy_right = lossy.right_addr();
    let lossy_left = lossy.left_addr();
    let clean_right = clean.right_addr();
    let clean_left = clean.left_addr();

    let lossy_receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{lossy_right}:9010")).unwrap();
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

    let clean_receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{clean_right}:9010")).unwrap();
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

    // Send through both connections concurrently.
    let lossy_sender = UdpSocket::bind(format!("{lossy_left}:0")).unwrap();
    let clean_sender = UdpSocket::bind(format!("{clean_left}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        lossy_sender.send_to(&i.to_be_bytes(), format!("{lossy_right}:9010")).unwrap();
        clean_sender.send_to(&i.to_be_bytes(), format!("{clean_right}:9010")).unwrap();
    }

    let lossy_received = lossy_receiver.join().unwrap();
    let clean_received = clean_receiver.join().unwrap();

    let lossy_loss = 1.0 - lossy_received as f64 / NUM_PACKETS as f64;
    let clean_loss = 1.0 - clean_received as f64 / NUM_PACKETS as f64;

    println!(
        "lossy: sent={NUM_PACKETS} received={lossy_received} loss={:.1}% (target {:.0}%)",
        lossy_loss * 100.0,
        LOSS_RATE * 100.0,
    );
    println!(
        "clean: sent={NUM_PACKETS} received={clean_received} loss={:.1}% (target 0%)",
        clean_loss * 100.0,
    );

    // Lossy connection should exhibit the configured loss rate.
    assert!(
        lossy_loss > LOSS_RATE * 0.5 && lossy_loss < LOSS_RATE * 1.5,
        "lossy connection: expected ~{:.0}% loss, got {:.1}%",
        LOSS_RATE * 100.0,
        lossy_loss * 100.0,
    );

    // Clean connection must not be affected by the other connection's loss rule.
    assert!(
        clean_loss < 0.05,
        "clean connection suffered {:.1}% loss — isolation failure",
        clean_loss * 100.0,
    );
}
