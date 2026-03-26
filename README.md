# badnet

Simulate bad network conditions in Rust integration tests using Linux `tc-netem`.

Each `BadNet` instance provisions a loopback address pair wired through a configurable netem qdisc. Traffic sent between the two addresses passes through the impairment rules. Everything is cleaned up when the handle is dropped.

## Usage

```rust
use std::time::Duration;
use badnet::BadNet;

let net = BadNet::builder()
    .seed(42)           // reproducible pattern across runs
    .loss(0.10)         // 10% packet loss
    .corrupt(0.02)      // 2% bit-flip (appears as loss at the socket layer)
    .duplicate(0.05)    // 5% duplication
    .delay(Duration::from_millis(50))
    .reorder(0.20)      // 20% of packets skip the delay and arrive early
    .build()?;

// Bind sockets to the two ends and communicate normally.
let tx = UdpSocket::bind(format!("{}:0",    net.left_addr()))?;
let rx = UdpSocket::bind(format!("{}:9000", net.right_addr()))?;
```

`reorder` requires `delay`. `gap` (periodic deterministic reorder) requires both:

```rust
let net = BadNet::builder()
    .delay(Duration::from_millis(20))
    .reorder(1.0)
    .gap(5)   // every 5th packet is sent immediately
    .build()?;
```

## Requirements

Linux only. Requires `CAP_NET_ADMIN`. Grant it with `setcap` or run as root:

```sh
sudo setcap cap_net_admin+eip target/debug/deps/<test_binary>
```
