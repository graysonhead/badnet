use std::net::UdpSocket;
use std::process::Command;

use badnet::BadNet;

fn lo_has_htb() -> bool {
    let out = Command::new("tc")
        .args(["qdisc", "show", "dev", "lo"])
        .output()
        .expect("failed to run tc");
    String::from_utf8_lossy(&out.stdout).contains("htb")
}

fn lo_has_addr(addr: std::net::Ipv4Addr) -> bool {
    let out = Command::new("ip")
        .args(["addr", "show", "lo"])
        .output()
        .expect("failed to run ip");
    String::from_utf8_lossy(&out.stdout).contains(&addr.to_string())
}

fn build() -> BadNet {
    BadNet::builder()
        .build()
        .expect("failed to create BadNet — grant CAP_NET_ADMIN via setcap (see library docs)")
}

#[test]
fn addresses_removed_after_drop() {
    let net = build();
    let left = net.left_addr();
    let right = net.right_addr();

    assert!(lo_has_addr(left), "left addr should exist before drop");
    assert!(lo_has_addr(right), "right addr should exist before drop");

    drop(net);

    assert!(!lo_has_addr(left), "left addr should be gone after drop");
    assert!(!lo_has_addr(right), "right addr should be gone after drop");

    // Binding to a removed address must fail.
    assert!(
        UdpSocket::bind(format!("{left}:0")).is_err(),
        "should not be able to bind to removed left addr"
    );
    assert!(
        UdpSocket::bind(format!("{right}:0")).is_err(),
        "should not be able to bind to removed right addr"
    );
}

#[test]
fn root_qdisc_survives_until_all_dropped() {
    let a = build();
    let b = build();

    assert!(lo_has_htb(), "root HTB qdisc should exist with two instances alive");

    drop(a);
    assert!(lo_has_htb(), "root HTB qdisc should survive while one instance remains");

    drop(b);
    assert!(!lo_has_htb(), "root HTB qdisc should be gone after all instances drop");
}
