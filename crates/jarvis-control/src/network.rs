//! Internet check: DNS resolve + TCP connect RTT, plus adapter status (`GetAdaptersAddresses`).

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::ERROR_BUFFER_OVERFLOW;
use windows::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
    IP_ADAPTER_ADDRESSES_LH,
};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Networking::WinSock::AF_UNSPEC;

use crate::error::{ControlError, ControlResult};

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// IANA ifType values that are not real connectivity.
const IF_TYPE_LOOPBACK: u32 = 24;
const IF_TYPE_TUNNEL: u32 = 131;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Probe {
    pub target: String,
    pub ok: bool,
    pub rtt_ms: Option<u128>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Adapter {
    pub name: String,
    pub up: bool,
    pub if_type: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NetworkReport {
    pub online: bool,
    pub dns_ok: bool,
    pub dns_ms: Option<u128>,
    pub probes: Vec<Probe>,
    pub adapters: Vec<Adapter>,
}

/// Resolves a host name with a hard timeout (the resolver call itself cannot be cancelled,
/// so it runs on a helper thread that is simply abandoned on timeout).
fn resolve_with_timeout(host: &str, timeout: Duration) -> Option<Duration> {
    let (sender, receiver) = mpsc::channel();
    let host = format!("{host}:443");
    let started = Instant::now();
    std::thread::spawn(move || {
        let found = host
            .to_socket_addrs()
            .map(|mut addrs| addrs.next().is_some());
        let _ = sender.send(found.unwrap_or(false));
    });
    match receiver.recv_timeout(timeout) {
        Ok(true) => Some(started.elapsed()),
        _ => None,
    }
}

fn tcp_probe(address: SocketAddr, timeout: Duration) -> Probe {
    let started = Instant::now();
    let ok = TcpStream::connect_timeout(&address, timeout).is_ok();
    Probe {
        target: address.to_string(),
        ok,
        rtt_ms: ok.then(|| started.elapsed().as_millis()),
    }
}

/// Status of all non-loopback adapters.
pub fn adapters() -> ControlResult<Vec<Adapter>> {
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
    let mut size = 16 * 1024u32;
    for _ in 0..4 {
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        // SAFETY: the u64 buffer is 8-byte aligned and at least `size` bytes long.
        let code = unsafe {
            GetAdaptersAddresses(
                u32::from(AF_UNSPEC.0),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>()),
                &mut size,
            )
        };
        if code == ERROR_BUFFER_OVERFLOW.0 {
            continue;
        }
        if code != 0 {
            return Err(ControlError::system(format!(
                "GetAdaptersAddresses failed with code {code}"
            )));
        }
        let mut result = Vec::new();
        let mut current = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: the list was written by the API into `buffer`, which is alive here; nodes are
        // linked by pointers into the same buffer and terminated by null.
        unsafe {
            while !current.is_null() {
                let adapter = &*current;
                let name = adapter.FriendlyName.to_string().unwrap_or_default();
                if adapter.IfType != IF_TYPE_LOOPBACK && adapter.IfType != IF_TYPE_TUNNEL {
                    result.push(Adapter {
                        name,
                        up: adapter.OperStatus == IfOperStatusUp,
                        if_type: adapter.IfType,
                    });
                }
                current = adapter.Next;
            }
        }
        return Ok(result);
    }
    Err(ControlError::system("adapter list kept changing size"))
}

/// Full connectivity report: DNS, TCP RTT to 1.1.1.1:443 and 8.8.8.8:53, adapter status.
pub fn check() -> NetworkReport {
    let dns = resolve_with_timeout("one.one.one.one", PROBE_TIMEOUT);
    let targets: [SocketAddr; 2] = [
        SocketAddr::from(([1, 1, 1, 1], 443)),
        SocketAddr::from(([8, 8, 8, 8], 53)),
    ];
    let handles: Vec<_> = targets
        .iter()
        .map(|address| {
            let address = *address;
            std::thread::spawn(move || tcp_probe(address, PROBE_TIMEOUT))
        })
        .collect();
    let probes: Vec<Probe> = handles
        .into_iter()
        .zip(targets)
        .map(|(handle, address)| {
            handle.join().unwrap_or(Probe {
                target: address.to_string(),
                ok: false,
                rtt_ms: None,
            })
        })
        .collect();
    NetworkReport {
        online: probes.iter().any(|probe| probe.ok),
        dns_ok: dns.is_some(),
        dns_ms: dns.map(|elapsed| elapsed.as_millis()),
        probes,
        adapters: adapters().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_to_closed_local_port_reports_failure() {
        // Port 9 on localhost is not listening; the probe must fail quickly and not panic.
        let probe = tcp_probe(
            SocketAddr::from(([127, 0, 0, 1], 9)),
            Duration::from_millis(500),
        );
        assert!(!probe.ok);
        assert!(probe.rtt_ms.is_none());
    }

    #[test]
    fn probe_to_local_listener_reports_rtt() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("addr");
        let probe = tcp_probe(address, Duration::from_secs(1));
        assert!(probe.ok);
        assert!(probe.rtt_ms.is_some());
    }

    #[test]
    fn adapters_enumerate() {
        let adapters = adapters().expect("adapters");
        assert!(adapters.iter().all(|adapter| !adapter.name.is_empty()));
    }
}
