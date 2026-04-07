use std::net::UdpSocket;
use std::time::Duration;
use badnet::BadNet;

const GE_P: f64 = 0.1;
const GE_R: f64 = 0.5;
const GE_H: f64 = 0.0;
const GE_K: f64 = 1.0;
const EXPECTED_LOSS: f64 = GE_P / (GE_P + GE_R); // ≈ 0.167
const NUM_PACKETS: usize = 500;

#[test]
fn gilbert_elliot_aggregate_loss() {
    let net = BadNet::builder()
        .seed(42)
        .loss_ge(GE_P, GE_R, GE_H, GE_K)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap");

    let right_addr = net.right_addr();
    let left_addr = net.left_addr();

    let receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{right_addr}:9100")).unwrap();
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
        sender.send_to(&i.to_be_bytes(), format!("{right_addr}:9100")).unwrap();
    }

    let received = receiver.join().unwrap();
    let actual_loss = 1.0 - (received as f64 / NUM_PACKETS as f64);

    println!(
        "sent={NUM_PACKETS}  received={received}  loss={:.1}%  (expected ~{:.1}%)",
        actual_loss * 100.0,
        EXPECTED_LOSS * 100.0,
    );

    assert!(
        actual_loss > EXPECTED_LOSS * 0.5 && actual_loss < EXPECTED_LOSS * 1.5,
        "expected ~{:.1}% GE loss, got {:.1}%",
        EXPECTED_LOSS * 100.0,
        actual_loss * 100.0,
    );
}
