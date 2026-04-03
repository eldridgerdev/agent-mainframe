use std::collections::BTreeMap;
use std::time::{Duration, Instant};

const SUMMARY_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Default)]
struct LatencyMetric {
    total_count: u64,
    interval_samples_us: Vec<u64>,
}

impl LatencyMetric {
    fn record(&mut self, duration: Duration) {
        let micros = duration.as_micros().min(u64::MAX as u128) as u64;
        self.total_count += 1;
        self.interval_samples_us.push(micros);
    }

    fn take_snapshot(&mut self) -> Option<LatencySnapshot> {
        if self.interval_samples_us.is_empty() {
            return None;
        }

        let mut samples = std::mem::take(&mut self.interval_samples_us);
        samples.sort_unstable();

        let count = samples.len();
        let total_us: u128 = samples.iter().map(|v| *v as u128).sum();
        let avg_us = (total_us / count as u128) as u64;
        let p50_us = percentile(&samples, 0.50);
        let p95_us = percentile(&samples, 0.95);
        let max_us = *samples.last().unwrap_or(&0);

        Some(LatencySnapshot {
            interval_count: count,
            total_count: self.total_count,
            avg_us,
            p50_us,
            p95_us,
            max_us,
        })
    }
}

fn percentile(samples: &[u64], pct: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }

    let idx = ((samples.len() - 1) as f64 * pct).round() as usize;
    samples[idx.min(samples.len() - 1)]
}

struct LatencySnapshot {
    interval_count: usize,
    total_count: u64,
    avg_us: u64,
    p50_us: u64,
    p95_us: u64,
    max_us: u64,
}

impl LatencySnapshot {
    fn format(&self, name: &str) -> String {
        format!(
            "{name} n={} total={} avg={} p50={} p95={} max={}",
            self.interval_count,
            self.total_count,
            format_micros(self.avg_us),
            format_micros(self.p50_us),
            format_micros(self.p95_us),
            format_micros(self.max_us),
        )
    }
}

fn format_micros(micros: u64) -> String {
    if micros >= 1_000 {
        format!("{:.2}ms", micros as f64 / 1_000.0)
    } else {
        format!("{micros}us")
    }
}

#[derive(Default)]
pub struct PerfCollector {
    latencies: BTreeMap<&'static str, LatencyMetric>,
    counters: BTreeMap<&'static str, u64>,
    last_summary_at: Option<Instant>,
    pending_input_draw_started_at: Option<Instant>,
}

impl PerfCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_duration(&mut self, name: &'static str, duration: Duration) {
        self.latencies.entry(name).or_default().record(duration);
    }

    pub fn increment_counter(&mut self, name: &'static str) {
        *self.counters.entry(name).or_default() += 1;
    }

    pub fn note_input_for_next_draw(&mut self) {
        self.pending_input_draw_started_at = Some(Instant::now());
    }

    pub fn note_draw_completed(&mut self) {
        self.increment_counter("ui.draw.count");
        if let Some(started_at) = self.pending_input_draw_started_at.take() {
            self.record_duration("ui.input_to_draw", started_at.elapsed());
        }
    }

    pub fn take_due_summary_lines(&mut self) -> Vec<String> {
        let now = Instant::now();
        match self.last_summary_at {
            Some(last) if now.duration_since(last) < SUMMARY_INTERVAL => {
                return Vec::new();
            }
            _ => {
                self.last_summary_at = Some(now);
            }
        }

        let mut lines = Vec::new();

        for name in [
            "view.capture_pane_ansi",
            "view.pipe_read",
            "view.render_snapshot_lines",
            "view.cursor_position",
            "view.send_literal",
            "view.send_key_name",
            "ui.draw",
            "ui.input_to_draw",
            "main.handle_key",
            "main.handle_mouse",
            "main.handle_paste",
            "sync.statuses",
            "sync.session_status",
            "sync.thinking_status",
            "scan.notifications",
            "usage.refresh",
            "summary.poll_result",
        ] {
            if let Some(metric) = self.latencies.get_mut(name)
                && let Some(snapshot) = metric.take_snapshot()
            {
                lines.push(snapshot.format(name));
            }
        }

        let redraws = self.counters.remove("ui.draw.count").unwrap_or(0);
        let key_events = self.counters.remove("event.key").unwrap_or(0);
        let mouse_events = self.counters.remove("event.mouse").unwrap_or(0);
        let paste_events = self.counters.remove("event.paste").unwrap_or(0);

        if redraws > 0 || key_events > 0 || mouse_events > 0 || paste_events > 0 {
            lines.push(format!(
                "counters redraws={} key_events={} mouse_events={} paste_events={}",
                redraws, key_events, mouse_events, paste_events
            ));
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::PerfCollector;
    use std::time::Duration;

    #[test]
    fn summary_includes_recorded_latency_metrics() {
        let mut perf = PerfCollector::new();
        perf.record_duration("ui.draw", Duration::from_millis(2));
        perf.record_duration("ui.draw", Duration::from_millis(4));

        let lines = perf.take_due_summary_lines();
        assert!(
            lines.iter().any(|line| line.contains("ui.draw n=2")),
            "expected ui.draw summary line, got: {lines:?}"
        );
    }

    #[test]
    fn draw_completion_records_input_to_draw_latency() {
        let mut perf = PerfCollector::new();
        perf.note_input_for_next_draw();
        perf.note_draw_completed();

        let lines = perf.take_due_summary_lines();
        assert!(
            lines.iter().any(|line| line.contains("ui.input_to_draw")),
            "expected input_to_draw metric, got: {lines:?}"
        );
    }
}
