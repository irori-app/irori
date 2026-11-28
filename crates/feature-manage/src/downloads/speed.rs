//! Ring buffer of transfer-rate samples of the current download, for the
//! sparkline on the downloads page.

use std::collections::VecDeque;

use irori_core::{GameId, InstallProgress};

/// Samples kept; at the backend's 250 ms progress cadence this is the last
/// half minute or so.
pub(crate) const SPEED_SAMPLES: usize = 120;

/// One rate sample in bytes per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SpeedSample {
    pub download: u64,
    pub disk: u64,
}

/// Rate history of active transfers. Resets when the tracked transfer changes.
#[derive(Debug, Clone)]
pub(crate) struct SpeedHistory {
    capacity: usize,
    samples: VecDeque<SpeedSample>,
    tracking: Option<String>,
    last: Option<InstallProgress>,
    last_web: Option<(u64, u64)>,
}

impl SpeedHistory {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(2),
            samples: VecDeque::with_capacity(capacity),
            tracking: None,
            last: None,
            last_web: None,
        }
    }

    /// Record the latest progress of the download `id` (or `None` when
    /// nothing downloads). A sample is added only when progress changed, so
    /// repeated notifications for unrelated games do not stretch the graph.
    /// Returns whether the history changed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn record(&mut self, id: Option<&GameId>, progress: Option<InstallProgress>) -> bool {
        self.record_combined(id, progress, false, 0, 0)
    }

    /// Record aggregated transfer speed across the active store download and
    /// active web/torrent downloads.
    pub fn record_combined(
        &mut self,
        store_id: Option<&GameId>,
        progress: Option<InstallProgress>,
        web_active: bool,
        web_speed: u64,
        web_bytes_done: u64,
    ) -> bool {
        let key = store_id
            .map(GameId::key)
            .or_else(|| web_active.then(|| "__web__".to_string()));
        if self.tracking != key {
            let had = !self.samples.is_empty();
            self.samples.clear();
            self.last = None;
            self.last_web = None;
            self.tracking = key;
            if self.tracking.is_none() {
                return had;
            }
        }
        if store_id.is_some() {
            let web_sig = web_active.then_some((web_speed, web_bytes_done));
            let store_changed = progress.is_some() && self.last != progress;
            let web_changed = web_sig.is_some() && self.last_web != web_sig;
            if !store_changed && !web_changed {
                return false;
            }
            if let Some(p) = progress {
                self.last = Some(p);
            }
            self.last_web = web_sig;
            let effective = self.last.unwrap_or_default();
            let store_has_rates =
                effective.download_speed.is_some() || effective.disk_speed.is_some();
            if !store_has_rates && !web_active {
                return false;
            }
            let total_down = effective
                .download_speed
                .unwrap_or(0)
                .saturating_add(if web_active { web_speed } else { 0 });
            let disk = effective.disk_speed.unwrap_or(0);
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(SpeedSample {
                download: total_down,
                disk,
            });
            return true;
        }
        if web_active {
            let sig = (web_speed, web_bytes_done);
            if self.last_web == Some(sig) {
                return false;
            }
            self.last_web = Some(sig);
            if web_speed == 0 && self.samples.is_empty() {
                return false;
            }
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(SpeedSample {
                download: web_speed,
                disk: 0,
            });
            return true;
        }
        false
    }

    pub fn samples(&self) -> impl ExactSizeIterator<Item = &SpeedSample> {
        self.samples.iter()
    }

    /// Mean download rate over the kept window, steadier than the latest
    /// reading; `None` until a nonzero rate was seen. Estimates use it so a
    /// passing dip doesn't swing them.
    pub fn average_download(&self) -> Option<u64> {
        let (sum, n) = self
            .samples
            .iter()
            .filter(|s| s.download > 0)
            .fold((0u64, 0u64), |(sum, n), s| {
                (sum.saturating_add(s.download), n + 1)
            });
        (n > 0).then(|| sum / n)
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use irori_core::Runner;

    fn p(down: u64, disk: u64) -> Option<InstallProgress> {
        Some(InstallProgress {
            download_speed: Some(down),
            disk_speed: Some(disk),
            ..Default::default()
        })
    }

    #[test]
    fn keeps_a_bounded_window() {
        let id = GameId::new(Runner::Gog, "a");
        let mut h = SpeedHistory::new(3);
        for i in 1..=5 {
            assert!(h.record(Some(&id), p(i * 10, i)));
        }
        assert_eq!(h.len(), 3);
        let downs: Vec<u64> = h.samples().map(|s| s.download).collect();
        assert_eq!(downs, vec![30, 40, 50]);
    }

    #[test]
    fn ignores_repeats_and_resets_on_new_download() {
        let a = GameId::new(Runner::Gog, "a");
        let b = GameId::new(Runner::Zoom, "b");
        let mut h = SpeedHistory::new(10);
        assert!(h.record(Some(&a), p(5, 5)));
        assert!(!h.record(Some(&a), p(5, 5)));
        assert!(!h.record(Some(&a), None));
        assert_eq!(h.len(), 1);
        assert!(h.record(Some(&b), p(7, 1)));
        assert_eq!(h.len(), 1);
        assert_eq!(h.samples().next().unwrap().download, 7);
        assert!(h.record(None, None));
        assert_eq!(h.len(), 0);
        assert!(!h.record(None, None));
    }

    #[test]
    fn averages_nonzero_download_rates() {
        let a = GameId::new(Runner::Gog, "a");
        let mut h = SpeedHistory::new(10);
        assert_eq!(h.average_download(), None);
        h.record(Some(&a), p(0, 5));
        assert_eq!(h.average_download(), None, "a stall alone says nothing");
        h.record(Some(&a), p(100, 5));
        h.record(Some(&a), p(300, 5));
        assert_eq!(h.average_download(), Some(200));
    }

    #[test]
    fn progress_without_rates_adds_nothing() {
        let a = GameId::new(Runner::Gog, "a");
        let mut h = SpeedHistory::new(10);
        let pct = Some(InstallProgress {
            percent: Some(10.0),
            ..Default::default()
        });
        assert!(!h.record(Some(&a), pct));
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn records_web_only_and_combined_store_plus_web_speeds() {
        let mut h = SpeedHistory::new(5);
        assert!(h.record_combined(None, None, true, 1_000, 100));
        assert!(h.record_combined(None, None, true, 2_000, 300));
        assert_eq!(h.len(), 2);
        assert_eq!(h.average_download(), Some(1_500));

        let id = GameId::new(Runner::Gog, "a");
        assert!(h.record_combined(Some(&id), p(3_000, 500), true, 1_000, 400));
        assert_eq!(h.len(), 1);
        let sample = h.samples().next().copied().unwrap();
        assert_eq!(sample.download, 4_000);
        assert_eq!(sample.disk, 500);
    }
}
