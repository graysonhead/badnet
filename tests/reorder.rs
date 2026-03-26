use std::net::UdpSocket;
use std::sync::mpsc;
use std::time::Duration;

use badnet::BadNet;

const REORDER_RATE: f64 = 0.3;
const DELAY: Duration = Duration::from_millis(20);
const NUM_PACKETS: usize = 500;

#[test]
fn udp_packet_reorder() {
    let net = BadNet::builder()
        .seed(42)
        .delay(DELAY)
        .reorder(REORDER_RATE)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let right_addr = net.right_addr();
    let left_addr = net.left_addr();

    // tc-netem reorder sends REORDER_RATE% of packets immediately while holding
    // the rest for DELAY.  We measure the reorder rate by splitting receives
    // into two phases around the delay boundary:
    //
    //   Phase 1 (timeout < DELAY): drains immediate packets, which have already
    //   landed in the socket buffer since loopback latency ≈ 0.
    //
    //   Phase 2 (timeout > DELAY): collects the held (delayed) packets.
    //
    // actual_reorder_rate = phase1_count / total
    let (tx, rx) = mpsc::channel::<()>();

    let receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{right_addr}:9003")).unwrap();
        let mut buf = [0u8; 8];

        // Wait until the sender has finished so all immediate packets are
        // already sitting in the socket buffer.
        rx.recv().unwrap();

        // Phase 1: immediate packets (timeout well under DELAY).
        socket.set_read_timeout(Some(DELAY / 4)).unwrap();
        let mut immediate = 0usize;
        loop {
            match socket.recv(&mut buf) {
                Ok(_) => immediate += 1,
                Err(_) => break,
            }
        }

        // Phase 2: delayed packets (timeout well over DELAY).
        socket.set_read_timeout(Some(DELAY * 5)).unwrap();
        let mut delayed = 0usize;
        loop {
            match socket.recv(&mut buf) {
                Ok(_) => delayed += 1,
                Err(_) => break,
            }
        }

        (immediate, delayed)
    });

    // Give receiver time to bind.
    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left_addr}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender
            .send_to(&i.to_be_bytes(), format!("{right_addr}:9003"))
            .unwrap();
    }

    // Signal receiver that all packets are in-flight.
    tx.send(()).unwrap();

    let (immediate, delayed) = receiver.join().unwrap();
    let total = immediate + delayed;
    let actual_reorder_rate = immediate as f64 / total as f64;

    println!(
        "sent={NUM_PACKETS}  total_received={total}  immediate={immediate}  delayed={delayed}  \
         reorder_rate={:.1}%  (target {:.0}%)",
        actual_reorder_rate * 100.0,
        REORDER_RATE * 100.0,
    );

    // Allow ±50% of the target rate (15%–45% for a 30% target).
    assert!(
        actual_reorder_rate > REORDER_RATE * 0.5 && actual_reorder_rate < REORDER_RATE * 1.5,
        "expected ~{:.0}% reorder, got {:.1}%",
        REORDER_RATE * 100.0,
        actual_reorder_rate * 100.0,
    );
}
