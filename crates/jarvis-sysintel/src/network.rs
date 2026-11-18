//! Network: throughput from the adapter table (`GetIfTable2`), link state, and a TCP-connect probe.
//!
//! Throughput is the sum over connected *hardware* adapters only; virtual adapters (Hyper-V, VPN,
//! loopback) are skipped because their traffic is already counted on the physical NIC.

use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::{IfOperStatusUp, MediaConnectStateConnected};

/// A reading of the cumulative counters of all connected hardware adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterTotals {
    pub in_octets: u64,
    pub out_octets: u64,
    /// At least one hardware adapter is connected.
    pub link_up: bool,
}

/// Reads the adapter table. `None` if the call fails.
pub fn read_totals() -> Option<AdapterTotals> {
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    // SAFETY: GetIfTable2 allocates `table`, which is freed with FreeMibTable below.
    unsafe {
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return None;
        }
        let count = (*table).NumEntries as usize;
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), count);
        let mut totals = AdapterTotals {
            in_octets: 0,
            out_octets: 0,
            link_up: false,
        };
        for row in rows {
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            let connected = row.OperStatus == IfOperStatusUp
                && row.MediaConnectState == MediaConnectStateConnected;
            if hardware && connected {
                totals.link_up = true;
                totals.in_octets = totals.in_octets.saturating_add(row.InOctets);
                totals.out_octets = totals.out_octets.saturating_add(row.OutOctets);
            }
        }
        FreeMibTable(table as *const _);
        Some(totals)
    }
}

/// Tracks the previous counters to turn them into bytes per second.
#[derive(Default)]
pub struct Throughput {
    previous: Option<(AdapterTotals, Instant)>,
}

impl Throughput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `(link_up, down_bps, up_bps)`. Rates are `None` on the first call, when the adapter
    /// set changed (counters went backwards), or when the table cannot be read.
    pub fn sample(&mut self) -> (Option<bool>, Option<u64>, Option<u64>) {
        let Some(now) = read_totals() else {
            self.previous = None;
            return (None, None, None);
        };
        let at = Instant::now();
        let rates = self.previous.and_then(|(before, then)| {
            let seconds = at.duration_since(then).as_secs_f64();
            Some((
                crate::storage::rate(before.in_octets, now.in_octets, seconds)?,
                crate::storage::rate(before.out_octets, now.out_octets, seconds)?,
            ))
        });
        self.previous = Some((now, at));
        match rates {
            Some((down, up)) => (Some(now.link_up), Some(down), Some(up)),
            None => (Some(now.link_up), None, None),
        }
    }
}

/// Time to open a TCP connection to `host` (`name:port` or `ip:port`). `None` on any failure.
/// Blocks for at most `timeout` (name resolution aside); run it on a blocking worker.
pub fn probe_rtt(host: &str, timeout: Duration) -> Option<Duration> {
    let addr = host.to_socket_addrs().ok()?.next()?;
    let started = Instant::now();
    let stream = std::net::TcpStream::connect_timeout(&addr, timeout).ok()?;
    let elapsed = started.elapsed();
    drop(stream);
    Some(elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn probe_measures_a_local_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let rtt = probe_rtt(&addr.to_string(), Duration::from_secs(2)).expect("connect");
        assert!(rtt < Duration::from_secs(2));
    }

    #[test]
    fn probe_fails_for_a_closed_port_and_bad_host() {
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        assert!(probe_rtt(&format!("127.0.0.1:{port}"), Duration::from_millis(500)).is_none());
        assert!(probe_rtt("not a host", Duration::from_millis(500)).is_none());
    }

    #[test]
    fn adapter_table_is_readable() {
        let mut throughput = Throughput::new();
        let (link, down, _) = throughput.sample();
        assert!(link.is_some());
        assert!(down.is_none(), "no rate on the first sample");
    }
}
