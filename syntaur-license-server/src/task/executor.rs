use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::backend::router::BackendRouter;
use crate::backend::{BackendError, CompletionRequest, CompletionResponse, RoutePreferences};
use crate::config::ExecutorConfig;

/// Executes completion requests with retry logic, timeouts, and fallback.
pub struct TaskExecutor {
    backend_router: Arc<BackendRouter>,
    config: ExecutorConfig,
}

impl TaskExecutor {
    pub fn new(backend_router: Arc<BackendRouter>, config: ExecutorConfig) -> Self {
        Self {
            backend_router,
            config,
        }
    }

    /// Execute a request with full retry + fallback logic.
    pub async fn execute(
        &self,
        request: &CompletionRequest,
        preferences: &RoutePreferences,
        timeout: Option<Duration>,
    ) -> Result<CompletionResponse, BackendError> {
        let timeout = timeout.unwrap_or(self.config.default_timeout);
        let deadline = Instant::now() + timeout;

        let mut attempt = 0;
        let mut last_error = None;

        while attempt <= self.config.max_retries {
            if Instant::now() > deadline {
                warn!("[executor] deadline exceeded after {} attempts", attempt);
                return Err(BackendError::Timeout);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());

            match tokio::time::timeout(remaining, self.backend_router.route(request, preferences))
                .await
            {
                Ok(Ok(response)) => {
                    info!(
                        "[executor] success on attempt {} via {}",
                        attempt + 1,
                        response.backend_id
                    );
                    return Ok(response);
                }
                Ok(Err(e)) => {
                    warn!("[executor] attempt {} failed: {}", attempt + 1, e);
                    last_error = Some(e.clone());

                    // Don't retry on invalid requests
                    if matches!(e, BackendError::InvalidRequest(_)) {
                        return Err(e);
                    }

                    // Wait before retrying (with backoff)
                    if attempt < self.config.max_retries {
                        let delay = self.config.retry_delay * (attempt + 1);
                        let delay = delay.min(remaining);
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
                Err(_) => {
                    warn!("[executor] attempt {} timed out", attempt + 1);
                    return Err(BackendError::Timeout);
                }
            }

            attempt += 1;
        }

        Err(last_error.unwrap_or(BackendError::Timeout))
    }

    /// Execute multiple independent requests concurrently.
    pub async fn execute_parallel(
        &self,
        requests: Vec<(CompletionRequest, RoutePreferences)>,
        timeout: Option<Duration>,
    ) -> Vec<Result<CompletionResponse, BackendError>> {
        let handles: Vec<_> = requests
            .into_iter()
            .map(|(req, prefs)| {
                let executor = Self {
                    backend_router: self.backend_router.clone(),
                    config: self.config.clone(),
                };
                tokio::spawn(async move { executor.execute(&req, &prefs, timeout).await })
            })
            .collect();

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(BackendError::ModelError(format!(
                    "task join error: {}",
                    e
                )))),
            }
        }
        results
    }
}
