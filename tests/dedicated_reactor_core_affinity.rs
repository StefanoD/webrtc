//! Integration test for pinning a dedicated reactor thread to a CPU core
//! (issue #101 follow-up): `PeerConnectionBuilder::with_dedicated_reactor_thread_on_core`.
//!
//! Building the connection exercises the pinning path (`core_affinity::set_for_current`
//! runs at the start of the reactor thread, before the runtime is built, so it has
//! definitely executed by the time `build()` returns). On Linux we then prove the
//! pin took effect by reading the reactor thread's `Cpus_allowed_list` from
//! `/proc/self/task` and asserting it equals the requested core. Each `tests/*.rs`
//! is its own process, so the `webrtc-reactor` thread lookup is unambiguous.
use std::sync::Arc;

use webrtc::peer_connection::{
    PeerConnectionBuilder, PeerConnectionEventHandler, available_core_ids,
};
use webrtc::runtime::block_on;

#[cfg(target_os = "linux")]
use std::time::Duration;
#[cfg(target_os = "linux")]
use webrtc::runtime::sleep;

struct NoopHandler;

#[async_trait::async_trait]
impl PeerConnectionEventHandler for NoopHandler {}

#[test]
fn test_dedicated_reactor_pinned_to_core() {
    block_on(run());
}

async fn run() {
    // Pick a core the process is actually allowed to run on, so the pin can't be
    // rejected by a restrictive cpuset. Skip if the platform reports none.
    let core = match available_core_ids().into_iter().next() {
        Some(core) => core,
        None => return,
    };

    let pc = PeerConnectionBuilder::new()
        .with_handler(Arc::new(NoopHandler))
        .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
        .with_dedicated_reactor_thread_on_core(core)
        .build()
        .await
        .expect("build pinned dedicated-reactor peer connection");

    // On Linux, verify the reactor thread was actually pinned to `core`.
    #[cfg(target_os = "linux")]
    {
        let allowed = wait_for_reactor_cpus(Duration::from_secs(5)).await;
        let expected = core.to_string();
        assert_eq!(
            allowed.as_deref(),
            Some(expected.as_str()),
            "reactor thread should be pinned to core {core}, but Cpus_allowed_list = {allowed:?}",
        );
    }

    // Keep the connection (and thus the reactor thread) alive until the check above.
    drop(pc);
}

/// The `Cpus_allowed_list` of the `webrtc-reactor` thread, or `None` if the thread
/// is not present yet. `webrtc-reactor` is 14 bytes, within Linux's 15-char `comm`
/// limit, so it is not truncated.
#[cfg(target_os = "linux")]
fn reactor_thread_cpus_allowed() -> Option<String> {
    for entry in std::fs::read_dir("/proc/self/task").ok()?.flatten() {
        let is_reactor = std::fs::read_to_string(entry.path().join("comm"))
            .map(|comm| comm.trim() == "webrtc-reactor")
            .unwrap_or(false);
        if !is_reactor {
            continue;
        }
        let status = std::fs::read_to_string(entry.path().join("status")).ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Cpus_allowed_list:") {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// Poll until the reactor thread exists (and thus has set its affinity) or timeout.
#[cfg(target_os = "linux")]
async fn wait_for_reactor_cpus(timeout: Duration) -> Option<String> {
    let step = Duration::from_millis(10);
    let mut waited = Duration::ZERO;
    loop {
        if let Some(cpus) = reactor_thread_cpus_allowed() {
            return Some(cpus);
        }
        if waited >= timeout {
            return None;
        }
        sleep(step).await;
        waited += step;
    }
}
