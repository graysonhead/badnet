//! Demonstrates live reconfiguration of a [`BadNet`] link from a second thread.
//!
//! Run with:
//! ```sh
//! cargo run --example dynamic_impairment
//! ```
//!
//! Expected output: ~0 packets received during the first 500 ms (100 % loss),
//! then normal delivery resumes after `reconfigure` clears the loss.

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use badnet::{BadNet, BadNetConfig};

fn main() -> std::io::Result<()> {
    // Build a link with 100 % packet loss.
    let net = BadNet::builder().loss(1.0).build()?;

    let left = net.left_addr();
    let right = net.right_addr();

    // Wrap in Arc<Mutex> so the controller thread can call reconfigure.
    let net = Arc::new(Mutex::new(net));

    // ── Receiver thread ──────────────────────────────────────────────────────
    // Counts packets received and records the timestamp of the first delivery
    // after reconfigure.
    let rx_sock = UdpSocket::bind(format!("{right}:9100"))?;
    rx_sock.set_read_timeout(Some(Duration::from_millis(50)))?;

    let received_before = Arc::new(Mutex::new(0u32));
    let received_after  = Arc::new(Mutex::new(0u32));
    let start           = Instant::now();
    let reconfigure_at  = Arc::new(Mutex::new(None::<Duration>));

    let rb = Arc::clone(&received_before);
    let ra = Arc::clone(&received_after);
    let rat = Arc::clone(&reconfigure_at);

    let rx_thread = std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        // Run for ~2.5 s to cover the full sender window.
        while start.elapsed() < Duration::from_millis(2500) {
            if rx_sock.recv(&mut buf).is_ok() {
                let elapsed = start.elapsed();
                let phase_switch = *rat.lock().unwrap();
                match phase_switch {
                    None => *rb.lock().unwrap() += 1,
                    Some(t) if elapsed >= t => *ra.lock().unwrap() += 1,
                    _ => *rb.lock().unwrap() += 1,
                }
            }
        }
    });

    // ── Controller thread ────────────────────────────────────────────────────
    // Sleeps 500 ms, then reconfigures the link to 0 % loss.
    let net_ctrl = Arc::clone(&net);
    let rat2 = Arc::clone(&reconfigure_at);

    let ctrl_thread = std::thread::spawn(move || -> std::io::Result<()> {
        std::thread::sleep(Duration::from_millis(500));
        let mut net = net_ctrl.lock().unwrap();
        net.reconfigure(BadNetConfig::default())?;
        *rat2.lock().unwrap() = Some(start.elapsed());
        println!("reconfigured at {:.0} ms — loss cleared", start.elapsed().as_secs_f64() * 1000.0);
        Ok(())
    });

    // ── Sender (main thread) ─────────────────────────────────────────────────
    // Send 200 packets spaced 10 ms apart (~2 s total).
    let tx_sock = UdpSocket::bind(format!("{left}:0"))?;
    let dest = format!("{right}:9100");

    for i in 0u32..200 {
        tx_sock.send_to(&i.to_le_bytes(), &dest)?;
        std::thread::sleep(Duration::from_millis(10));
    }

    rx_thread.join().ok();
    ctrl_thread.join().ok().transpose()?;

    let before = *received_before.lock().unwrap();
    let after  = *received_after.lock().unwrap();
    println!("packets received BEFORE reconfigure (~500 ms, 100% loss): {before}");
    println!("packets received AFTER  reconfigure (~1500 ms, 0% loss):  {after}");

    Ok(())
}
