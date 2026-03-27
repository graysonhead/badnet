//! Simulate bad network conditions on loopback using [`tc-netem`].
//!
//! `badnet` provisions a pair of loopback addresses and wires them together
//! through a Linux traffic-control (`tc`) pipeline so that any traffic sent
//! from one address to the other passes through a configurable
//! [`tc-netem`] qdisc.  This lets you reproduce packet loss, corruption,
//! duplication, delay, and reordering in ordinary integration tests without
//! touching real network interfaces.
//!
//! # Platform requirements
//!
//! Linux only.  The calling process needs [`CAP_NET_ADMIN`].  Grant it with
//! `setcap` rather than running the whole process as root — running a test
//! binary as root gives it unrestricted access to the entire system, which is
//! far broader than what this library needs:
//!
//! ```sh
//! sudo setcap cap_net_admin+eip target/debug/deps/<test_binary>
//! ```
//!
//! # Quick start
//!
//! ```no_run
//! use std::net::UdpSocket;
//! use std::time::Duration;
//! use badnet::BadNet;
//!
//! // Create a link that drops 10 % of packets.
//! let net = BadNet::builder()
//!     .loss(0.10)
//!     .build()?;
//!
//! // Bind sockets to the two ends and communicate normally.
//! let tx = UdpSocket::bind(format!("{}:0",   net.left_addr()))?;
//! let rx = UdpSocket::bind(format!("{}:9000", net.right_addr()))?;
//! rx.set_read_timeout(Some(Duration::from_millis(200)))?;
//!
//! tx.send_to(b"hello", format!("{}:9000", net.right_addr()))?;
//! // … the link will drop ~10 % of these …
//!
//! // The tc rules and loopback addresses are removed when `net` is dropped.
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Isolation
//!
//! Each [`BadNet`] instance claims a unique address pair and HTB class, so
//! multiple instances running concurrently do not interfere with each other.
//!
//! # Reorder and gap
//!
//! [`BadNetBuilder::reorder`] requires [`BadNetBuilder::delay`] to have been
//! called first (enforced at compile time via the typestate on
//! [`BadNetBuilder`]).  [`BadNetBuilder::gap`] additionally requires
//! [`BadNetBuilder::reorder`].
//!
//! ```no_run
//! use std::time::Duration;
//! use badnet::BadNet;
//!
//! // 20 ms delay, 30 % of packets reordered probabilistically.
//! let net = BadNet::builder()
//!     .delay(Duration::from_millis(20))
//!     .reorder(0.30)
//!     .build()?;
//!
//! // Every 5th packet reordered deterministically (reorder must be 1.0 for
//! // a fully deterministic pattern).
//! let net = BadNet::builder()
//!     .delay(Duration::from_millis(20))
//!     .reorder(1.0)
//!     .gap(5)
//!     .build()?;
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # Reproducible impairment with seeds
//!
//! `tc-netem` uses an internal PRNG to decide which packets to drop, corrupt,
//! duplicate, or reorder.  Passing the same seed on the same kernel version
//! produces an identical sequence of decisions, making test failures
//! reproducible:
//!
//! ```no_run
//! use badnet::BadNet;
//!
//! // Seed 42 produces the same loss decisions on every run.
//! let net = BadNet::builder().seed(42).loss(0.10).build()?;
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! The default seed is `0`.  Vary the seed when you want statistically
//! independent runs rather than a fixed pattern.
//!
//! # Troubleshooting
//!
//! ## Stale state after a crashed run
//!
//! [`BadNet`] cleans up its TC rules and loopback addresses in its [`Drop`]
//! implementation.  However, if the process is killed before `Drop` runs
//! (e.g. `SIGKILL`, or `Ctrl-C` without a signal handler), the kernel objects
//! are left behind and the next run will fail with errors like:
//!
//! ```text
//! tc qdisc add dev lo root handle 1: htb default 1` failed: Error: Exclusivity flag on, cannot modify.
//! ip addr add 10.0.0.1/32 dev lo` failed: Error: ipv4: Address already assigned.
//! ```
//!
//! To clean up manually:
//!
//! ```sh
//! sudo tc qdisc del dev lo root 2>/dev/null; \
//! sudo ip -4 addr show dev lo | awk '/inet 10\./{print $2}' | \
//!   xargs -r -I{} sudo ip addr del {} dev lo
//! ```
//!
//! ## Ctrl-C during long-running binaries
//!
//! Rust does not run destructors on `SIGINT` by default.  If your binary runs
//! [`BadNet`] instances for an extended period and users may interrupt it, install
//! a signal handler that performs an orderly shutdown so that `Drop` is called:
//!
//! ```ignore
//! # fn run() {}
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use std::sync::Arc;
//!
//! let running = Arc::new(AtomicBool::new(true));
//! let r = running.clone();
//! unsafe {
//!     libc::signal(libc::SIGINT, handle_sigint as libc::sighandler_t);
//! }
//! extern "C" fn handle_sigint(_: libc::c_int) { /* set flag, then std::process::exit(0) */ }
//! ```
//!
//! A crate such as [`ctrlc`] makes this straightforward:
//!
//! ```ignore
//! // ctrlc = "3"
//! ctrlc::set_handler(|| std::process::exit(0)).unwrap();
//! ```
//!
//! Calling [`std::process::exit`] runs destructors, which allows [`BadNet`]'s
//! `Drop` impl to clean up before the process exits.
//!
//! [`tc-netem`]: https://www.man7.org/linux/man-pages/man8/tc-netem.8.html
//! [`CAP_NET_ADMIN`]: https://man7.org/linux/man-pages/man7/capabilities.7.html
//! [`ctrlc`]: https://docs.rs/ctrlc

use std::io;
use std::marker::PhantomData;
use std::net::Ipv4Addr;
use std::time::Duration;
use std::sync::{
    atomic::{AtomicU16, Ordering},
    Mutex,
};

// Each BadNet instance gets a unique address pair:
//   left  = 10.<id>>8>.<id&0xFF>.1
//   right = 10.<id>>8>.<id&0xFF>.2
// Safe for up to 65 536 instances before wrapping.
static SUBNET_COUNTER: AtomicU16 = AtomicU16::new(0);

// ── Global TC state for the shared `lo` interface ──────────────────────────
//
// We install a single HTB root qdisc on `lo` (handle 1:, default class 1:1).
// Class 1:1 is a high-rate passthrough for all unclassified traffic.
// Each BadNet instance claims a unique minor ≥ 2 (up to 65 534 concurrent),
// attaches an HTB class + netem leaf to it, and adds two u32 filters that
// steer its address pair into that class.

struct LoTcState {
    instance_count: u32,
    next_minor: u32, // monotonically increasing; never reused within a process run
}

impl LoTcState {
    const fn new() -> Self {
        Self { instance_count: 0, next_minor: 2 }
    }

    fn alloc_minor(&mut self) -> u32 {
        let m = self.next_minor;
        self.next_minor += 1;
        m
    }
}

static LO_TC: Mutex<LoTcState> = Mutex::new(LoTcState::new());

// ── Public API ─────────────────────────────────────────────────────────────

/// Runtime-configurable impairment parameters for a [`BadNet`] link.
///
/// Construct via `Default` (all zeros / no impairment) and set only the
/// fields you care about, then pass to [`BadNet::reconfigure`].
#[derive(Clone, Debug)]
pub struct BadNetConfig {
    pub seed: u64,
    pub delay: Duration,
    pub loss_rate: f64,
    pub corrupt_rate: f64,
    pub duplicate_rate: f64,
    pub reorder_rate: f64,
    pub gap: u32,
}

impl Default for BadNetConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            delay: Duration::ZERO,
            loss_rate: 0.0,
            corrupt_rate: 0.0,
            duplicate_rate: 0.0,
            reorder_rate: 0.0,
            gap: 0,
        }
    }
}

/// A running virtual link backed by loopback addresses and `tc-netem`.
///
/// Dropping the value removes the TC rules and the loopback addresses.
pub struct BadNet {
    left_addr: Ipv4Addr,
    right_addr: Ipv4Addr,
    /// Class minor in the root HTB qdisc (e.g. `1:3`).
    class_minor: u32,
    /// TC filter priorities for the two directions.
    filter_prio_fwd: u32,
    filter_prio_rev: u32,
    /// Current impairment parameters.
    config: BadNetConfig,
}

// ── Typestate markers for BadNetBuilder ────────────────────────────────────
#[doc(hidden)] pub struct NoDelay;
#[doc(hidden)] pub struct WithDelay;
#[doc(hidden)] pub struct NoReorder;
#[doc(hidden)] pub struct WithReorder;

/// Builder for a [`BadNet`] link.
///
/// Obtain one via [`BadNet::builder`].  Call impairment methods in any order,
/// then call [`build`](BadNetBuilder::build) to provision the link.
///
/// # Method availability
///
/// [`reorder`](BadNetBuilder::reorder) is only available after
/// [`delay`](BadNetBuilder::delay) has been called; [`gap`](BadNetBuilder::gap)
/// additionally requires [`reorder`](BadNetBuilder::reorder).  These
/// constraints are enforced at **compile time** via the type parameters `D`
/// and `R` — you will get a type error, not a runtime panic, if you call them
/// in the wrong order.
pub struct BadNetBuilder<D = NoDelay, R = NoReorder> {
    seed: u64,
    delay: Duration,
    loss_rate: f64,
    corrupt_rate: f64,
    duplicate_rate: f64,
    reorder_rate: f64,
    gap: u32,
    _state: PhantomData<(D, R)>,
}

impl Default for BadNetBuilder<NoDelay, NoReorder> {
    fn default() -> Self {
        Self {
            seed: 0,
            delay: Duration::ZERO,
            loss_rate: 0.0,
            corrupt_rate: 0.0,
            duplicate_rate: 0.0,
            reorder_rate: 0.0,
            gap: 0,
            _state: PhantomData,
        }
    }
}

impl BadNet {
    /// Create a builder for configuring and provisioning a new link.
    pub fn builder() -> BadNetBuilder {
        BadNetBuilder::default()
    }

    /// Address of the left end of the link.
    ///
    /// Bind sockets to this address to send traffic that will pass through
    /// the configured impairments before reaching [`right_addr`](Self::right_addr).
    pub fn left_addr(&self) -> Ipv4Addr { self.left_addr }

    /// Address of the right end of the link.
    ///
    /// Bind sockets to this address to send traffic that will pass through
    /// the configured impairments before reaching [`left_addr`](Self::left_addr).
    pub fn right_addr(&self) -> Ipv4Addr { self.right_addr }

    /// Returns a reference to the current impairment configuration.
    pub fn config(&self) -> &BadNetConfig { &self.config }

    /// Atomically replace the netem impairment parameters on the live link.
    ///
    /// Uses `tc qdisc change` so there is no teardown/rebuild gap.
    ///
    /// # Errors
    ///
    /// Returns an error if the `tc` command fails.
    pub fn reconfigure(&mut self, config: BadNetConfig) -> io::Result<()> {
        netem_tc("change", self.class_minor, &config)?;
        self.config = config;
        Ok(())
    }
}

impl<D, R> BadNetBuilder<D, R> {
    // Converts between builder states, carrying all field values across.
    fn restate<D2, R2>(self) -> BadNetBuilder<D2, R2> {
        BadNetBuilder {
            seed: self.seed,
            delay: self.delay,
            loss_rate: self.loss_rate,
            corrupt_rate: self.corrupt_rate,
            duplicate_rate: self.duplicate_rate,
            reorder_rate: self.reorder_rate,
            gap: self.gap,
            _state: PhantomData,
        }
    }

    /// Seed for `tc-netem`'s internal PRNG.
    ///
    /// Using the same seed on the same kernel version produces a reproducible
    /// impairment pattern, which is useful for deterministic tests.  The
    /// default seed is `0`.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Fraction of packets to drop, in `[0.0, 1.0]`.
    ///
    /// # Panics
    ///
    /// Panics if `rate` is outside `[0.0, 1.0]`.
    pub fn loss(mut self, rate: f64) -> Self {
        assert!((0.0..=1.0).contains(&rate), "loss rate must be in [0.0, 1.0]");
        self.loss_rate = rate;
        self
    }

    /// Fraction of packets to corrupt by flipping a random bit, in `[0.0, 1.0]`.
    ///
    /// Corrupted packets fail their UDP checksum and are **silently dropped**
    /// by the kernel before reaching the receiving socket, so corruption
    /// manifests as additional packet loss at the application layer.
    ///
    /// # Panics
    ///
    /// Panics if `rate` is outside `[0.0, 1.0]`.
    pub fn corrupt(mut self, rate: f64) -> Self {
        assert!((0.0..=1.0).contains(&rate), "corrupt rate must be in [0.0, 1.0]");
        self.corrupt_rate = rate;
        self
    }

    /// Delay applied to every packet.
    ///
    /// Calling this method unlocks [`reorder`](BadNetBuilder::reorder), which
    /// in turn unlocks [`gap`](BadNetBuilder::gap).
    pub fn delay(mut self, d: Duration) -> BadNetBuilder<WithDelay, R> {
        self.delay = d;
        self.restate()
    }

    /// Fraction of packets to duplicate, in `[0.0, 1.0]`.
    ///
    /// Each duplicated packet is delivered twice to the receiver.
    ///
    /// # Panics
    ///
    /// Panics if `rate` is outside `[0.0, 1.0]`.
    pub fn duplicate(mut self, rate: f64) -> Self {
        assert!((0.0..=1.0).contains(&rate), "duplicate rate must be in [0.0, 1.0]");
        self.duplicate_rate = rate;
        self
    }

    /// Provision the link and return a [`BadNet`] handle.
    ///
    /// Allocates a loopback address pair, installs an HTB class and a
    /// `tc-netem` qdisc, and adds u32 filters to steer traffic between the
    /// two addresses through the qdisc.  All resources are released when the
    /// returned [`BadNet`] is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if any `ip` or `tc` command fails — most commonly
    /// because the process lacks [`CAP_NET_ADMIN`].
    ///
    /// # Capability
    ///
    /// Requires [`CAP_NET_ADMIN`].  Use `setcap` to grant only this
    /// capability to the test binary rather than running it as root:
    ///
    /// ```sh
    /// sudo setcap cap_net_admin+eip target/debug/deps/<test_binary>
    /// ```
    ///
    /// Running test binaries as root is strongly discouraged — it grants the
    /// entire process unrestricted system access far beyond what this library
    /// requires.
    ///
    /// [`CAP_NET_ADMIN`]: https://man7.org/linux/man-pages/man7/capabilities.7.html
    pub fn build(self) -> io::Result<BadNet> {
        let id = SUBNET_COUNTER.fetch_add(1, Ordering::SeqCst);
        let hi = (id >> 8) as u8;
        let lo = (id & 0xFF) as u8;
        let left_addr = Ipv4Addr::new(10, hi, lo, 1);
        let right_addr = Ipv4Addr::new(10, hi, lo, 2);

        let config = BadNetConfig {
            seed: self.seed,
            delay: self.delay,
            loss_rate: self.loss_rate,
            corrupt_rate: self.corrupt_rate,
            duplicate_rate: self.duplicate_rate,
            reorder_rate: self.reorder_rate,
            gap: self.gap,
        };

        // Add addresses to loopback.
        ip(&["addr", "add", &format!("{left_addr}/32"), "dev", "lo"])?;
        ip(&["addr", "add", &format!("{right_addr}/32"), "dev", "lo"])
            .map_err(|e| { let _ = ip_best_effort_del(left_addr); e })?;

        // TC setup under the global lock.
        let (class_minor, filter_prio_fwd, filter_prio_rev) =
            match setup_tc(left_addr, right_addr, &config) {
                Ok(v) => v,
                Err(e) => {
                    let _ = ip_best_effort_del(left_addr);
                    let _ = ip_best_effort_del(right_addr);
                    return Err(e);
                }
            };

        Ok(BadNet { left_addr, right_addr, class_minor, filter_prio_fwd, filter_prio_rev, config })
    }
}

impl<R> BadNetBuilder<WithDelay, R> {
    /// Fraction of packets to deliver out of order, in `[0.0, 1.0]`.
    ///
    /// `tc-netem` implements reorder by sending `rate` fraction of packets
    /// *immediately* while holding the remainder for the configured
    /// [`delay`](BadNetBuilder::delay).  The immediately-sent packets arrive
    /// ahead of the delayed ones, causing reordering.
    ///
    /// Calling this method unlocks [`gap`](BadNetBuilder::gap).
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use badnet::BadNet;
    ///
    /// let net = BadNet::builder()
    ///     .delay(Duration::from_millis(20))
    ///     .reorder(0.30)   // 30 % of packets skip the delay and arrive early
    ///     .build()?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `rate` is outside `[0.0, 1.0]`.
    pub fn reorder(mut self, rate: f64) -> BadNetBuilder<WithDelay, WithReorder> {
        assert!((0.0..=1.0).contains(&rate), "reorder rate must be in [0.0, 1.0]");
        self.reorder_rate = rate;
        self.restate()
    }
}

impl BadNetBuilder<WithDelay, WithReorder> {
    /// Reorder every `n`th packet deterministically.
    ///
    /// In `tc-netem`, `gap` is a sub-option of `reorder`: every `n`th packet
    /// is a candidate for immediate delivery, subject to the reorder
    /// probability.  Setting [`reorder`](BadNetBuilder::reorder) to `1.0`
    /// makes the pattern fully deterministic (exactly 1-in-`n` packets skip
    /// the delay).
    ///
    /// Both [`delay`](BadNetBuilder::delay) and
    /// [`reorder`](BadNetBuilder::reorder) must be configured first; this is
    /// enforced at compile time.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use std::time::Duration;
    /// use badnet::BadNet;
    ///
    /// // Every 5th packet is sent immediately; the other four are held for
    /// // 20 ms.  With reorder(1.0) the pattern is fully deterministic.
    /// let net = BadNet::builder()
    ///     .delay(Duration::from_millis(20))
    ///     .reorder(1.0)
    ///     .gap(5)
    ///     .build()?;
    /// # Ok::<(), std::io::Error>(())
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `n` is `0`.
    pub fn gap(mut self, n: u32) -> Self {
        assert!(n >= 1, "gap must be >= 1");
        self.gap = n;
        self
    }
}

// ── TC setup / teardown ────────────────────────────────────────────────────

/// Issue `tc qdisc <subcommand>` (either `"add"` or `"change"`) for the netem
/// leaf attached to `class_minor`, using the parameters in `cfg`.
fn netem_tc(subcommand: &str, class_minor: u32, cfg: &BadNetConfig) -> io::Result<()> {
    let netem_seed = (cfg.seed & 0xFFFF_FFFF) as u32;
    let class_parent = format!("1:{class_minor}");
    let class_handle = format!("{}:", class_minor * 100);
    let delay_us = format!("{}us", cfg.delay.as_micros());
    let reorder_pct = format!("{:.4}%", cfg.reorder_rate * 100.0);
    let loss_pct = format!("{:.4}%", cfg.loss_rate * 100.0);
    let corrupt_pct = format!("{:.4}%", cfg.corrupt_rate * 100.0);
    let duplicate_pct = format!("{:.4}%", cfg.duplicate_rate * 100.0);
    let gap_str = cfg.gap.to_string();
    let seed_str = netem_seed.to_string();
    let mut netem_args: Vec<&str> = vec![
        "qdisc", subcommand, "dev", "lo",
        "parent", &class_parent,
        "handle", &class_handle,
        "netem",
        "delay", &delay_us,
        "reorder", &reorder_pct,
    ];
    if cfg.gap > 0 {
        netem_args.extend_from_slice(&["gap", &gap_str]);
    }
    netem_args.extend_from_slice(&[
        "loss", &loss_pct,
        "corrupt", &corrupt_pct,
        "duplicate", &duplicate_pct,
        "seed", &seed_str,
    ]);
    tc(&netem_args)
}

fn setup_tc(
    left: Ipv4Addr,
    right: Ipv4Addr,
    cfg: &BadNetConfig,
) -> io::Result<(u32, u32, u32)> {
    let mut state = LO_TC.lock().unwrap();

    if state.instance_count == 0 {
        // First instance: install the root HTB qdisc.
        // default 1 ⇒ all unclassified traffic goes to class 1:1 (passthrough).
        tc(&["qdisc", "add", "dev", "lo", "root", "handle", "1:", "htb", "default", "1"])?;
        // Class 1:1 — uncapped passthrough for all non-test traffic.
        tc(&["class", "add", "dev", "lo", "parent", "1:", "classid", "1:1",
             "htb", "rate", "1tbit"])?;
    }

    let class_minor = state.alloc_minor();
    state.instance_count += 1;
    // Release the lock before issuing more `tc` commands.
    drop(state);

    // filter prios: unique per minor, two per instance (fwd + rev)
    let filter_prio_fwd = (class_minor - 2) * 2 + 1;
    let filter_prio_rev = filter_prio_fwd + 1;

    // HTB class for this instance (uncapped — netem provides the impairment).
    tc(&["class", "add", "dev", "lo", "parent", "1:", "classid", &format!("1:{class_minor}"),
         "htb", "rate", "1tbit"])?;

    // Netem qdisc as the leaf of our HTB class.
    netem_tc("add", class_minor, cfg)?;

    // Steer left→right and right→left traffic into our HTB+netem class.
    add_filter("lo", "1:", filter_prio_fwd, &left, &right, class_minor)?;
    add_filter("lo", "1:", filter_prio_rev, &right, &left, class_minor)?;

    Ok((class_minor, filter_prio_fwd, filter_prio_rev))
}

fn add_filter(dev: &str, parent: &str, prio: u32, src: &Ipv4Addr, dst: &Ipv4Addr, minor: u32) -> io::Result<()> {
    tc(&[
        "filter", "add", "dev", dev,
        "parent", parent,
        "prio", &prio.to_string(),
        "protocol", "ip", "u32",
        "match", "ip", "src", &format!("{src}/32"),
        "match", "ip", "dst", &format!("{dst}/32"),
        "flowid", &format!("1:{minor}"),
    ])
}

impl Drop for BadNet {
    fn drop(&mut self) {
        // Remove filters.
        let _ = tc(&["filter", "del", "dev", "lo", "parent", "1:",
                     "prio", &self.filter_prio_fwd.to_string()]);
        let _ = tc(&["filter", "del", "dev", "lo", "parent", "1:",
                     "prio", &self.filter_prio_rev.to_string()]);

        // Remove netem qdisc and HTB class for this instance.
        let _ = tc(&["qdisc", "del", "dev", "lo",
                     "parent", &format!("1:{}", self.class_minor)]);
        let _ = tc(&["class", "del", "dev", "lo",
                     "classid", &format!("1:{}", self.class_minor)]);

        // Remove root HTB qdisc (and the passthrough class 1:1) when last instance exits.
        let mut state = LO_TC.lock().unwrap();
        state.instance_count -= 1;
        if state.instance_count == 0 {
            let _ = tc(&["qdisc", "del", "dev", "lo", "root"]);
        }
        drop(state);

        // Remove loopback addresses.
        let _ = ip_best_effort_del(self.left_addr);
        let _ = ip_best_effort_del(self.right_addr);
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn ip_best_effort_del(addr: Ipv4Addr) -> io::Result<()> {
    ip(&["addr", "del", &format!("{addr}/32"), "dev", "lo"])
}

fn tc(args: &[&str]) -> io::Result<()> { run("tc", args) }
fn ip(args: &[&str]) -> io::Result<()> { run("ip", args) }

fn run(prog: &str, args: &[&str]) -> io::Result<()> {
    let out = std::process::Command::new(prog).args(args).output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("`{prog} {}` failed: {}", args.join(" "), stderr.trim()),
        ));
    }
    Ok(())
}
