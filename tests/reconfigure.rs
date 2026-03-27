use std::net::UdpSocket;
use std::time::Duration;

use badnet::{BadNet, BadNetConfig};

const PORT: u16 = 9300;
const BURST: usize = 200;

fn build_with_loss(rate: f64) -> BadNet {
    BadNet::builder()
        .seed(42)
        .loss(rate)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)")
}

/// Count packets that arrive within `timeout` after a burst of `n` datagrams.
fn send_and_count(
    tx: &UdpSocket,
    rx: &UdpSocket,
    dest: &str,
    n: usize,
    timeout: Duration,
) -> usize {
    rx.set_read_timeout(Some(timeout)).unwrap();
    for i in 0u32..n as u32 {
        tx.send_to(&i.to_le_bytes(), dest).unwrap();
    }
    let mut buf = [0u8; 8];
    let mut count = 0usize;
    loop {
        match rx.recv(&mut buf) {
            Ok(_) => count += 1,
            Err(_) => break,
        }
    }
    count
}

/// `BadNet::config()` should reflect the parameters passed to the builder.
#[test]
fn config_accessor_reflects_builder() {
    let delay = Duration::from_millis(5);
    let net = BadNet::builder()
        .seed(7)
        .loss(0.25)
        .corrupt(0.10)
        .delay(delay)
        .reorder(0.15)
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)");

    let cfg = net.config();
    assert_eq!(cfg.seed, 7);
    assert!((cfg.loss_rate - 0.25).abs() < f64::EPSILON);
    assert!((cfg.corrupt_rate - 0.10).abs() < f64::EPSILON);
    assert_eq!(cfg.duplicate_rate, 0.0);
    assert_eq!(cfg.delay, delay);
    assert!((cfg.reorder_rate - 0.15).abs() < f64::EPSILON);
}

/// After reconfiguring from 100 % loss to 0 % loss, packets should be delivered.
#[test]
fn reconfigure_clears_loss() {
    let mut net = build_with_loss(1.0);
    let left = net.left_addr();
    let right = net.right_addr();

    let tx = UdpSocket::bind(format!("{left}:0")).unwrap();
    let rx = UdpSocket::bind(format!("{right}:{PORT}")).unwrap();
    let dest = format!("{right}:{PORT}");

    // Phase 1: 100 % loss — nothing should arrive.
    let before = send_and_count(&tx, &rx, &dest, BURST, Duration::from_millis(300));
    assert_eq!(before, 0, "expected 0 packets with 100% loss, got {before}");

    // Lift the loss.
    net.reconfigure(BadNetConfig::default()).expect("reconfigure failed");

    // Verify the stored config was updated.
    assert!((net.config().loss_rate - 0.0).abs() < f64::EPSILON);

    // Phase 2: 0 % loss — most packets should arrive.
    let after = send_and_count(&tx, &rx, &dest, BURST, Duration::from_millis(500));
    assert!(
        after > BURST * 8 / 10,
        "expected >{} packets after clearing loss, got {after}",
        BURST * 8 / 10,
    );
}

/// After reconfiguring from 0 % loss to 100 % loss, no packets should arrive.
#[test]
fn reconfigure_applies_loss() {
    let mut net = build_with_loss(0.0);
    let left = net.left_addr();
    let right = net.right_addr();

    let tx = UdpSocket::bind(format!("{left}:0")).unwrap();
    let rx = UdpSocket::bind(format!("{right}:{PORT}")).unwrap();
    let dest = format!("{right}:{PORT}");

    // Phase 1: 0 % loss — most packets should arrive.
    let before = send_and_count(&tx, &rx, &dest, BURST, Duration::from_millis(500));
    assert!(
        before > BURST * 8 / 10,
        "expected >{} packets with 0% loss, got {before}",
        BURST * 8 / 10,
    );

    // Apply 100 % loss.
    net.reconfigure(BadNetConfig { loss_rate: 1.0, ..BadNetConfig::default() })
        .expect("reconfigure failed");

    // Verify the stored config was updated.
    assert!((net.config().loss_rate - 1.0).abs() < f64::EPSILON);

    // Phase 2: 100 % loss — nothing should arrive.
    let after = send_and_count(&tx, &rx, &dest, BURST, Duration::from_millis(300));
    assert_eq!(after, 0, "expected 0 packets after applying 100% loss, got {after}");
}
