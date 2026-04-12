use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use log::{debug, info, warn};
use tokio::sync::RwLock;

use super::{
    Backend, BackendError, BackendHealth, CompletionRequest, CompletionResponse, RoutePreferences,
};

/// Metrics tracked per backend for routing decisions.
#[derive(Debug)]
struct BackendMetrics {
    total_requests: u64,
    total_failures: u64,
    avg_latency_ms: f64,
    last_failure: Option<Instant>,
    consecutive_failures: u32,
}

impl Default for BackendMetrics {
    fn default() -> Self {
        Self {
            total_requests: 0,
            total_failures: 0,
            avg_latency_ms: 0.0,
            last_failure: None,
            consecutive_failures: 0,
        }
    }
}

/// Load-aware router that selects the best backend for each request,
/// with automatic fallback on failure and concurrency tracking.
pub struct BackendRouter {
    backends: RwLock<Vec<Arc<dyn Backend>>>,
    /// Shared between health check tasks and request path via Arc.
    health_cache: Arc<DashMap<String, BackendHealth>>,
    metrics: DashMap<String, BackendMetrics>,
    /// Number of in-flight requests per backend.
    in_flight: DashMap<String, AtomicU32>,
    health_check_interval: Duration,
}

impl BackendRouter {
    pub fn new() -> Self {
        Self {
            backends: RwLock::new(Vec::new()),
            health_cache: Arc::new(DashMap::new()),
            metrics: DashMap::new(),
            in_flight: DashMap::new(),
            health_check_interval: Duration::from_secs(30),
        }
    }

    pub async fn add_backend(&self, backend: Arc<dyn Backend>) {
        let id = backend.id().to_string();
        info!("[router] adding backend: {} ({})", id, backend.provider());
        self.metrics.insert(id.clone(), BackendMetrics::default());
        self.in_flight
            .insert(id.clone(), AtomicU32::new(0));
        self.backends.write().await.push(backend);
    }

    pub async fn list_backends(&self) -> Vec<BackendInfo> {
        let backends = self.backends.read().await;
        let mut infos = Vec::new();
        for b in backends.iter() {
            let health = self.health_cache.get(b.id()).map(|h| h.available);
            let metrics = self.metrics.get(b.id());
            let in_flight = self
                .in_flight
                .get(b.id())
                .map(|v| v.load(Ordering::Relaxed))
                .unwrap_or(0);
            infos.push(BackendInfo {
                id: b.id().to_string(),
                provider: b.provider().to_string(),
                model: b.capabilities().model_name.clone(),
                tags: b.capabilities().tags.clone(),
                healthy: health,
                avg_latency_ms: metrics.as_ref().map(|m| m.avg_latency_ms),
                total_requests: metrics.as_ref().map(|m| m.total_requests).unwrap_or(0),
                in_flight,
            });
        }
        infos
    }

    /// Route a completion request to the best available backend.
    pub async fn route(
        &self,
        request: &CompletionRequest,
        preferences: &RoutePreferences,
    ) -> Result<CompletionResponse, BackendError> {
        let candidates = self.select_candidates(request, preferences).await;

        if candidates.is_empty() {
            return Err(BackendError::Unavailable(
                "no backends available matching requirements".into(),
            ));
        }

        let mut last_error = None;

        for backend in &candidates {
            let backend_id = backend.id().to_string();
            let start = Instant::now();
            debug!("[router] trying backend: {}", backend_id);

            // Track in-flight
            if let Some(counter) = self.in_flight.get(&backend_id) {
                counter.fetch_add(1, Ordering::Relaxed);
            }

            let result = backend.complete(request).await;

            // Decrement in-flight
            if let Some(counter) = self.in_flight.get(&backend_id) {
                counter.fetch_sub(1, Ordering::Relaxed);
            }

            match result {
                Ok(response) => {
                    self.record_success(&backend_id, start.elapsed());
                    return Ok(response);
                }
                Err(e) => {
                    let elapsed = start.elapsed();
                    warn!(
                        "[router] backend {} failed ({:.1}ms): {}",
                        backend_id,
                        elapsed.as_secs_f64() * 1000.0,
                        e
                    );
                    self.record_failure(&backend_id);
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(BackendError::Unavailable("no backends tried".into())))
    }

    /// Select and rank candidate backends for a request.
    async fn select_candidates(
        &self,
        request: &CompletionRequest,
        preferences: &RoutePreferences,
    ) -> Vec<Arc<dyn Backend>> {
        let backends = self.backends.read().await;
        let mut candidates: Vec<(Arc<dyn Backend>, f64)> = Vec::new();

        for backend in backends.iter() {
            // Check tag requirements
            let tags = &backend.capabilities().tags;
            let required: Vec<_> = request
                .required_tags
                .iter()
                .chain(preferences.required_tags.iter())
                .collect();

            let tags_ok =
                required.is_empty() || required.iter().all(|t| tags.contains(t));

            if !tags_ok {
                continue;
            }

            // Check if backend is in circuit-breaker cooldown
            if let Some(metrics) = self.metrics.get(backend.id()) {
                if metrics.consecutive_failures >= 5 {
                    if let Some(last_fail) = metrics.last_failure {
                        if last_fail.elapsed() < Duration::from_secs(60) {
                            debug!("[router] skipping {} (circuit breaker)", backend.id());
                            continue;
                        }
                    }
                }
            }

            // Score: lower is better
            let mut score: f64;

            // Factor in average latency — untested backends get a neutral baseline
            // instead of 0.0 (which would unfairly rank them as fastest).
            if let Some(metrics) = self.metrics.get(backend.id()) {
                if metrics.total_requests > 0 {
                    score = metrics.avg_latency_ms;
                } else {
                    // No data yet: use a neutral score (won't be ranked first or last)
                    score = 500.0;
                }
                // Penalize backends with recent failures
                score += metrics.consecutive_failures as f64 * 100.0;
            } else {
                score = 500.0;
            }

            // Prefer explicitly requested backend
            if let Some(ref pref) = preferences.preferred_backend {
                if backend.id() == pref {
                    score -= 1000.0;
                }
            }

            // Penalize backends with many in-flight requests (concurrency-aware).
            // Penalty scales with the backend's own average latency — each queued
            // request adds ~half the expected response time, modeling real queue wait.
            // This means 3-4 queued requests on a fast backend will naturally overflow
            // to a slower-but-idle alternative.
            if let Some(counter) = self.in_flight.get(backend.id()) {
                let active = counter.load(Ordering::Relaxed);
                if active > 0 {
                    let latency_penalty = score.max(500.0) * 0.5;
                    score += active as f64 * latency_penalty;
                }
            }

            // Check cached health
            if let Some(health) = self.health_cache.get(backend.id()) {
                if !health.available {
                    score += 5000.0;
                }
            }

            candidates.push((backend.clone(), score));
        }

        // Sort by score (lower = better)
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // If fallback order is specified, append those in order
        if !preferences.fallback_backends.is_empty() {
            let mut ordered = Vec::new();
            for (b, _) in &candidates {
                if !preferences.fallback_backends.contains(&b.id().to_string()) {
                    ordered.push(b.clone());
                }
            }
            for fb_id in &preferences.fallback_backends {
                if let Some((b, _)) = candidates.iter().find(|(b, _)| b.id() == fb_id) {
                    ordered.push(b.clone());
                }
            }
            return ordered;
        }

        candidates.into_iter().map(|(b, _)| b).collect()
    }

    fn record_success(&self, backend_id: &str, latency: Duration) {
        let latency_ms = latency.as_secs_f64() * 1000.0;
        let mut metrics = self.metrics.entry(backend_id.to_string()).or_default();
        metrics.total_requests += 1;
        metrics.consecutive_failures = 0;
        if metrics.avg_latency_ms == 0.0 {
            metrics.avg_latency_ms = latency_ms;
        } else {
            metrics.avg_latency_ms = metrics.avg_latency_ms * 0.8 + latency_ms * 0.2;
        }
    }

    fn record_failure(&self, backend_id: &str) {
        let mut metrics = self.metrics.entry(backend_id.to_string()).or_default();
        metrics.total_requests += 1;
        metrics.total_failures += 1;
        metrics.consecutive_failures += 1;
        metrics.last_failure = Some(Instant::now());
    }

    /// Run health checks on all registered backends.
    pub async fn health_check_all(&self) {
        let backends = self.backends.read().await;
        let mut handles = Vec::new();

        for backend in backends.iter() {
            let b = backend.clone();
            // Clone the Arc, not the DashMap — writes go to the shared map.
            let cache = Arc::clone(&self.health_cache);
            handles.push(tokio::spawn(async move {
                let health = b.health().await;
                let id = b.id().to_string();
                debug!(
                    "[router] health check {}: available={}",
                    id, health.available
                );
                cache.insert(id, health);
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    }

    /// Background loop that periodically checks backend health.
    pub async fn run_health_loop(self: Arc<Self>) {
        // Run first health check immediately on startup
        self.health_check_all().await;
        info!(
            "[router] initial health check complete, interval={}s",
            self.health_check_interval.as_secs()
        );
        loop {
            tokio::time::sleep(self.health_check_interval).await;
            self.health_check_all().await;
        }
    }

    pub async fn backend_count(&self) -> usize {
        self.backends.read().await.len()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendInfo {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub tags: Vec<String>,
    pub healthy: Option<bool>,
    pub avg_latency_ms: Option<f64>,
    pub total_requests: u64,
    pub in_flight: u32,
}
