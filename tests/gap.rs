use std::net::UdpSocket;
use std::sync::mpsc;
use std::time::Duration;

use badnet::BadNet;

const GAP: u32 = 5;
const DELAY: Duration = Duration::from_millis(20);
const NUM_PACKETS: usize = 500;
// gap selects every Nth packet as a reorder candidate; reorder 100% ensures
// every candidate is sent immediately, giving a deterministic 1/GAP = 20% rate.
const REORDER_RATE: f64 = 1.0;

#[test]
fn udp_packet_gap() {
    let net = BadNet::builder()
        .seed(42)
        .delay(DELAY)
        .reorder(REORDER_RATE)
        .gap(GAP)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let right_addr = net.right_addr();
    let left_addr = net.left_addr();

    // gap N sends every Nth packet immediately while holding the rest for
    // DELAY, so we expect exactly 1/N of packets to arrive early.
    // We measure this with the same two-phase receive approach as the reorder
    // test: signal the receiver when all packets are in-flight, then drain
    // the socket with a short timeout (< DELAY) to capture immediate packets,
    // followed by a longer timeout for the delayed ones.
    let (tx, rx) = mpsc::channel::<()>();

    let receiver = std::thread::spawn(move || {
        let socket = UdpSocket::bind(format!("{right_addr}:9004")).unwrap();
        let mut buf = [0u8; 8];

        rx.recv().unwrap();

        // Phase 1: immediate packets (already in the socket buffer).
        socket.set_read_timeout(Some(DELAY / 4)).unwrap();
        let mut immediate = 0usize;
        loop {
            match socket.recv(&mut buf) {
                Ok(_) => immediate += 1,
                Err(_) => break,
            }
        }

        // Phase 2: delayed packets.
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

    std::thread::sleep(Duration::from_millis(50));

    let sender = UdpSocket::bind(format!("{left_addr}:0")).unwrap();
    for i in 0u64..NUM_PACKETS as u64 {
        sender
            .send_to(&i.to_be_bytes(), format!("{right_addr}:9004"))
            .unwrap();
    }

    tx.send(()).unwrap();

    let (immediate, delayed) = receiver.join().unwrap();
    let total = immediate + delayed;
    let actual_rate = immediate as f64 / total as f64;
    let expected_rate = 1.0 / GAP as f64;

    println!(
        "sent={NUM_PACKETS}  total_received={total}  immediate={immediate}  delayed={delayed}  \
         gap_rate={:.1}%  (target {:.0}%)",
        actual_rate * 100.0,
        expected_rate * 100.0,
    );

    // gap is deterministic, so allow only ±20% of the target rate.
    assert!(
        actual_rate > expected_rate * 0.8 && actual_rate < expected_rate * 1.2,
        "expected ~{:.0}% immediate (gap {}), got {:.1}%",
        expected_rate * 100.0,
        GAP,
        actual_rate * 100.0,
    );
}
