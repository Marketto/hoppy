//! Receiver-side limits of protocol v1 (spec §7.3 "Aborted handshakes", §7.7).
//!
//! The party that sends handshake message 3 learns the session's SAS before the other side
//! shows it. A man in the middle that plays the sender towards a receiver can therefore
//! abort and retry until the receiver's code equals the one it already fixed with the real
//! sender (online grinding). Every v1 session that ends after message 2 without a first
//! encrypted frame counts as an abort; [`Guard`] caps them per source IP and in total, with
//! an exponential per-IP backoff, so within the sender's 130 s deadline an attacker gets a
//! few dozen tries at a 1 in 2^24 chance each. It also rate-limits user-facing warnings.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Aborted handshakes allowed per source IP in any 60 s.
pub const MAX_ABORTS_PER_IP: usize = 5;
/// Aborted handshakes allowed from all sources together in any 60 s.
pub const MAX_ABORTS_TOTAL: usize = 20;
const WINDOW: Duration = Duration::from_secs(60);
/// First per-IP block after hitting the cap; doubles on each new strike, at most 1 h.
const BACKOFF_START: Duration = Duration::from_secs(60);
const BACKOFF_MAX: Duration = Duration::from_secs(3600);
/// Strikes are forgotten after this long without one.
const STRIKE_MEMORY: Duration = Duration::from_secs(3600);
/// The same warning (same key) reaches the user at most this often.
pub const WARNING_INTERVAL: Duration = Duration::from_secs(60);
const MAX_TRACKED: usize = 1024;

#[derive(Debug, Default)]
struct PerIp {
    aborts: VecDeque<Instant>,
    strikes: u32,
    last_strike: Option<Instant>,
    blocked_until: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Guard {
    ips: HashMap<IpAddr, PerIp>,
    total: VecDeque<Instant>,
    warned: HashMap<String, Instant>,
}

/// Why a v1 handshake is refused before it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused {
    /// Aborts seen from that IP (or in total, for the global cap) in the last minute.
    pub recent: usize,
}

impl Guard {
    fn prune(&mut self, now: Instant) {
        while self
            .total
            .front()
            .is_some_and(|t| now.duration_since(*t) >= WINDOW)
        {
            self.total.pop_front();
        }
        self.ips.retain(|_, p| {
            while p
                .aborts
                .front()
                .is_some_and(|t| now.duration_since(*t) >= WINDOW)
            {
                p.aborts.pop_front();
            }
            if p.last_strike
                .is_some_and(|t| now.duration_since(t) >= STRIKE_MEMORY)
            {
                p.strikes = 0;
                p.last_strike = None;
            }
            !p.aborts.is_empty() || p.strikes > 0 || p.blocked_until.is_some_and(|t| t > now)
        });
        if self.warned.len() > MAX_TRACKED {
            self.warned
                .retain(|_, t| now.duration_since(*t) < WARNING_INTERVAL);
        }
    }

    /// May a v1 handshake from `ip` start now (before message 2 is written)?
    pub fn admit(&mut self, ip: IpAddr, now: Instant) -> Result<(), Refused> {
        self.prune(now);
        if self.total.len() >= MAX_ABORTS_TOTAL {
            return Err(Refused {
                recent: self.total.len(),
            });
        }
        match self.ips.get(&ip) {
            Some(p) if p.blocked_until.is_some_and(|t| t > now) => Err(Refused {
                recent: p.aborts.len().max(MAX_ABORTS_PER_IP),
            }),
            Some(p) if p.aborts.len() >= MAX_ABORTS_PER_IP => Err(Refused {
                recent: p.aborts.len(),
            }),
            _ => Ok(()),
        }
    }

    /// A handshake from `ip` ended after message 2 without a first frame. Returns the
    /// number of recent aborts from that IP.
    pub fn record_abort(&mut self, ip: IpAddr, now: Instant) -> usize {
        self.prune(now);
        if self.ips.len() >= MAX_TRACKED && !self.ips.contains_key(&ip) {
            // Too many sources: the global cap still applies.
            self.total.push_back(now);
            return 1;
        }
        self.total.push_back(now);
        let p = self.ips.entry(ip).or_default();
        p.aborts.push_back(now);
        if p.aborts.len() >= MAX_ABORTS_PER_IP {
            p.strikes += 1;
            p.last_strike = Some(now);
            let factor = 1u32 << (p.strikes - 1).min(6);
            p.blocked_until = Some(now + (BACKOFF_START * factor).min(BACKOFF_MAX));
        }
        p.aborts.len()
    }

    /// `true` if a warning with this key may be shown now (at most one per
    /// [`WARNING_INTERVAL`]).
    pub fn should_warn(&mut self, key: &str, now: Instant) -> bool {
        match self.warned.get(key) {
            Some(t) if now.duration_since(*t) < WARNING_INTERVAL => false,
            _ => {
                self.warned.insert(key.to_string(), now);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn per_ip_cap_and_backoff() {
        let mut g = Guard::default();
        let t0 = Instant::now();
        let a = ip("192.168.1.9");
        for i in 0..MAX_ABORTS_PER_IP {
            assert!(g.admit(a, t0).is_ok(), "{i}");
            g.record_abort(a, t0);
        }
        assert!(g.admit(a, t0).is_err());
        assert!(
            g.admit(ip("192.168.1.10"), t0).is_ok(),
            "others still served"
        );
        // Window passed but the backoff (60 s) holds until then; then free again.
        assert!(g.admit(a, t0 + Duration::from_secs(59)).is_err());
        assert!(g.admit(a, t0 + Duration::from_secs(61)).is_ok());
        // Second strike: 120 s.
        let t1 = t0 + Duration::from_secs(61);
        for _ in 0..MAX_ABORTS_PER_IP {
            g.record_abort(a, t1);
        }
        assert!(g.admit(a, t1 + Duration::from_secs(100)).is_err());
        assert!(g.admit(a, t1 + Duration::from_secs(121)).is_ok());
    }

    #[test]
    fn global_cap_bounds_many_sources() {
        let mut g = Guard::default();
        let t0 = Instant::now();
        for i in 0..MAX_ABORTS_TOTAL {
            let src = IpAddr::from([10, 0, 0, i as u8]);
            assert!(g.admit(src, t0).is_ok());
            g.record_abort(src, t0);
        }
        assert!(g.admit(ip("10.0.1.1"), t0).is_err(), "global cap");
        assert!(g.admit(ip("10.0.1.1"), t0 + WINDOW).is_ok());
        // In any 130 s an attacker gets at most 3 windows' worth of aborts.
        let mut tries = 0;
        let mut t = t0 + WINDOW;
        let end = t + Duration::from_secs(130);
        let mut n = 0u32;
        while t < end {
            n += 1;
            let src = IpAddr::from([10, 1, (n >> 8) as u8, n as u8]);
            if g.admit(src, t).is_ok() {
                g.record_abort(src, t);
                tries += 1;
            }
            t += Duration::from_millis(100);
        }
        assert!(tries <= 3 * MAX_ABORTS_TOTAL, "{tries}");
    }

    #[test]
    fn warnings_are_rate_limited() {
        let mut g = Guard::default();
        let t0 = Instant::now();
        assert!(g.should_warn("k", t0));
        assert!(!g.should_warn("k", t0 + Duration::from_secs(10)));
        assert!(g.should_warn("other", t0));
        assert!(g.should_warn("k", t0 + WARNING_INTERVAL));
    }
}
