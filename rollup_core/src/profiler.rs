use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Performance profiler for identifying bottlenecks
pub struct PerformanceProfiler {
    metrics: Arc<DashMap<String, MetricData>>,
    spans: Arc<DashMap<u64, Span>>,
    span_counter: AtomicU64,
    enabled: bool,
}

#[derive(Debug, Clone)]
struct MetricData {
    total_calls: u64,
    total_time_us: u64,
    min_time_us: u64,
    max_time_us: u64,
    errors: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub span_id: u64,
    pub name: String,
    pub start_time: u64,
    pub duration_us: Option<u64>,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileReport {
    pub function: String,
    pub total_calls: u64,
    pub total_time_ms: f64,
    pub avg_time_us: u64,
    pub min_time_us: u64,
    pub max_time_us: u64,
    pub errors: u64,
    pub calls_per_second: f64,
}

impl PerformanceProfiler {
    pub fn new(enabled: bool) -> Self {
        Self {
            metrics: Arc::new(DashMap::new()),
            spans: Arc::new(DashMap::new()),
            span_counter: AtomicU64::new(0),
            enabled,
        }
    }

    /// Start timing a function
    pub fn start(&self, name: &str) -> Timer {
        if !self.enabled {
            return Timer::disabled();
        }

        Timer {
            name: name.to_string(),
            start: Instant::now(),
            profiler: Some(self.metrics.clone()),
        }
    }

    /// Create a span for distributed tracing
    pub fn start_span(&self, name: String) -> SpanGuard {
        let span_id = self.span_counter.fetch_add(1, Ordering::SeqCst);
        let start = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        let span = Span {
            span_id,
            name: name.clone(),
            start_time: start,
            duration_us: None,
            metadata: std::collections::HashMap::new(),
        };

        self.spans.insert(span_id, span);

        SpanGuard {
            span_id,
            spans: self.spans.clone(),
            start: Instant::now(),
        }
    }

    /// Record metric manually
    pub fn record(&self, name: &str, duration: Duration, error: bool) {
        if !self.enabled {
            return;
        }

        let duration_us = duration.as_micros() as u64;

        self.metrics
            .entry(name.to_string())
            .and_modify(|data| {
                data.total_calls += 1;
                data.total_time_us += duration_us;
                data.min_time_us = data.min_time_us.min(duration_us);
                data.max_time_us = data.max_time_us.max(duration_us);
                if error {
                    data.errors += 1;
                }
            })
            .or_insert(MetricData {
                total_calls: 1,
                total_time_us: duration_us,
                min_time_us: duration_us,
                max_time_us: duration_us,
                errors: if error { 1 } else { 0 },
            });
    }

    /// Get performance report
    pub fn get_report(&self) -> Vec<ProfileReport> {
        let mut reports: Vec<_> = self
            .metrics
            .iter()
            .map(|entry| {
                let name = entry.key().clone();
                let data = entry.value().clone();

                ProfileReport {
                    function: name,
                    total_calls: data.total_calls,
                    total_time_ms: data.total_time_us as f64 / 1000.0,
                    avg_time_us: if data.total_calls > 0 {
                        data.total_time_us / data.total_calls
                    } else {
                        0
                    },
                    min_time_us: data.min_time_us,
                    max_time_us: data.max_time_us,
                    errors: data.errors,
                    calls_per_second: data.total_calls as f64
                        / (data.total_time_us as f64 / 1_000_000.0).max(1.0),
                }
            })
            .collect();

        // Sort by total time (slowest first)
        reports.sort_by(|a, b| {
            b.total_time_ms
                .partial_cmp(&a.total_time_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        reports
    }

    /// Get top N slowest functions
    pub fn get_slowest(&self, n: usize) -> Vec<ProfileReport> {
        let mut report = self.get_report();
        report.truncate(n);
        report
    }

    /// Get all spans
    pub fn get_spans(&self) -> Vec<Span> {
        self.spans.iter().map(|e| e.value().clone()).collect()
    }

    /// Clear all metrics
    pub fn clear(&self) {
        self.metrics.clear();
        self.spans.clear();
    }

    /// Get summary statistics
    pub fn get_summary(&self) -> ProfileSummary {
        let reports = self.get_report();

        let total_time: f64 = reports.iter().map(|r| r.total_time_ms).sum();
        let total_calls: u64 = reports.iter().map(|r| r.total_calls).sum();
        let total_errors: u64 = reports.iter().map(|r| r.errors).sum();

        ProfileSummary {
            total_functions: reports.len(),
            total_time_ms: total_time,
            total_calls,
            total_errors,
            avg_call_time_us: if total_calls > 0 {
                (total_time * 1000.0) as u64 / total_calls
            } else {
                0
            },
        }
    }
}

pub struct Timer {
    name: String,
    start: Instant,
    profiler: Option<Arc<DashMap<String, MetricData>>>,
}

impl Timer {
    fn disabled() -> Self {
        Self {
            name: String::new(),
            start: Instant::now(),
            profiler: None,
        }
    }

    pub fn stop(self) {
        self.stop_with_error(false);
    }

    pub fn stop_with_error(self, error: bool) {
        if let Some(profiler) = self.profiler {
            let duration_us = self.start.elapsed().as_micros() as u64;

            profiler
                .entry(self.name.clone())
                .and_modify(|data| {
                    data.total_calls += 1;
                    data.total_time_us += duration_us;
                    data.min_time_us = data.min_time_us.min(duration_us);
                    data.max_time_us = data.max_time_us.max(duration_us);
                    if error {
                        data.errors += 1;
                    }
                })
                .or_insert(MetricData {
                    total_calls: 1,
                    total_time_us: duration_us,
                    min_time_us: duration_us,
                    max_time_us: duration_us,
                    errors: if error { 1 } else { 0 },
                });
        }
    }
}

pub struct SpanGuard {
    span_id: u64,
    spans: Arc<DashMap<u64, Span>>,
    start: Instant,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if let Some(mut span) = self.spans.get_mut(&self.span_id) {
            span.duration_us = Some(self.start.elapsed().as_micros() as u64);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub total_functions: usize,
    pub total_time_ms: f64,
    pub total_calls: u64,
    pub total_errors: u64,
    pub avg_call_time_us: u64,
}

impl Default for PerformanceProfiler {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_profiler() {
        let profiler = PerformanceProfiler::new(true);

        {
            let timer = profiler.start("test_function");
            thread::sleep(Duration::from_millis(10));
            timer.stop();
        }

        let report = profiler.get_report();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].function, "test_function");
        assert_eq!(report[0].total_calls, 1);
        assert!(report[0].avg_time_us > 1000); // At least 1ms
    }

    #[test]
    fn test_multiple_calls() {
        let profiler = PerformanceProfiler::new(true);

        for _ in 0..5 {
            let timer = profiler.start("repeated_function");
            thread::sleep(Duration::from_millis(1));
            timer.stop();
        }

        let report = profiler.get_report();
        assert_eq!(report[0].total_calls, 5);
    }

    #[test]
    fn test_span() {
        let profiler = PerformanceProfiler::new(true);

        {
            let _span = profiler.start_span("test_span".to_string());
            thread::sleep(Duration::from_millis(5));
        }

        let spans = profiler.get_spans();
        assert_eq!(spans.len(), 1);
        assert!(spans[0].duration_us.is_some());
    }

    #[test]
    fn test_summary() {
        let profiler = PerformanceProfiler::new(true);

        for i in 0..3 {
            let timer = profiler.start(&format!("func{}", i));
            thread::sleep(Duration::from_millis(1));
            timer.stop();
        }

        let summary = profiler.get_summary();
        assert_eq!(summary.total_functions, 3);
        assert_eq!(summary.total_calls, 3);
    }
}
