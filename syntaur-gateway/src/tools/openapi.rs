//! OpenAPI tool ingestion.
//!
//! At startup we load OpenAPI 3.x specs from `config.openapi.specs`,
//! parse them, and generate one `Tool` trait impl per allowlisted endpoint.
//! Generated tools land in the same extension HashMap that internal_search
//! and code_execute use, so they're discoverable by the LLM and dispatchable
//! by the existing tool registry.
//!
//! v1 supports:
//!   * OpenAPI 3.0 / 3.1 (JSON or YAML)
//!   * Path parameters, query parameters, JSON request bodies
//!   * Response: text/plain or JSON, returned as the tool's content
//!   * Auth: per-spec config — bearer token, header, or query param
//!
//! NOT supported in v1 (deferred):
//!   * OAuth2 flows (use a pre-acquired bearer token instead)
//!   * Multipart form data
//!   * Per-user auth (uses shared spec-level credentials only)
//!   * File uploads
//!   * Streaming responses

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, error, info, warn};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tools::extension::{RichToolResult, Tool, ToolContext};

/// One spec entry under `config.openapi.specs[name]`.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct OpenApiSpecConfig {
    /// Path to a local OpenAPI JSON or YAML file.
    pub spec_path: String,
    /// Optional override for the server base URL. If unset, uses the first
    /// `servers[].url` entry from the spec.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Endpoints to expose. Each entry is `"METHOD /path"`. If empty, ALL
    /// endpoints from the spec are exposed.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Default auth strategy used when no per-agent override matches.
    #[serde(default)]
    pub auth: Option<OpenApiAuth>,
    /// Per-agent auth overrides — keys are agent ids, values are the auth
    /// strategy to use when an agent of that id calls the tool. Falls back
    /// to `auth` if no entry matches.
    #[serde(default)]
    pub per_agent_auth: HashMap<String, OpenApiAuth>,
    /// True by default. Set false to disable an entry without removing it.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenApiAuth {
    /// `Authorization: Bearer <token>`
    Bearer { token: String },
    /// Custom header `<name>: <value>`
    Header { name: String, value: String },
    /// Query string parameter `?name=value`
    Query { name: String, value: String },
    /// OAuth2 client credentials flow. Server-to-server only — no user
    /// interaction. The token endpoint is hit lazily when the first call
    /// arrives, the result is cached until ~30s before expiry, then refreshed.
    OAuth2ClientCredentials {
        token_url: String,
        client_id: String,
        client_secret: String,
        #[serde(default)]
        scope: Option<String>,
    },
    /// OAuth2 authorization-code flow with PKCE. User-interactive: the
    /// user must first complete `/api/oauth/start → callback` to mint
    /// their access + refresh tokens, which are persisted in the
    /// `oauth_tokens` table. When a tool call arrives, the caller's
    /// `user_id` + `provider` key into that table; refresh happens
    /// lazily 30s before expiry. Requires Item 3 per-user auth (the
    /// caller's Principal supplies the user_id).
    /// v5 Item 4.
    OAuth2AuthCode {
        /// Provider id used as the cache key and the /connect argument.
        /// e.g. "google_calendar", "github".
        provider: String,
        /// Authorization endpoint URL where the user's browser is sent
        /// to approve the app.
        authorization_url: String,
        /// Token endpoint for code-for-tokens exchange and refresh.
        token_url: String,
        /// OAuth client ID registered with the provider.
        client_id: String,
        /// OAuth client secret.
        client_secret: String,
        /// Space-separated scopes requested at authorization time.
        #[serde(default)]
        scopes: String,
        /// Redirect URI registered with the provider. Must point to our
        /// `/api/oauth/callback` endpoint exposed via Tailscale Funnel.
        redirect_uri: String,
    },
}

/// Cached OAuth2 access token + expiry. Stored in-memory in `OAuthTokenCache`.
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

/// In-process cache of OAuth2 access tokens, keyed by (token_url + client_id).
/// Cloneable as `Arc<OAuthTokenCache>` and shared across all OpenApiTool
/// instances that need it.
pub struct OAuthTokenCache {
    inner: tokio::sync::Mutex<std::collections::HashMap<String, CachedToken>>,
}

impl OAuthTokenCache {
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            inner: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// Fetch a valid token, exchanging via the token endpoint if needed.
    /// Refreshes 30 seconds before actual expiry to avoid race conditions.
    pub async fn get_or_refresh(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &str,
        scope: Option<&str>,
        http: &reqwest::Client,
    ) -> Result<String, String> {
        let key = format!("{}|{}", token_url, client_id);
        // Check cache first
        {
            let cache = self.inner.lock().await;
            if let Some(tok) = cache.get(&key) {
                let cutoff = chrono::Utc::now() + chrono::Duration::seconds(30);
                if tok.expires_at > cutoff {
                    return Ok(tok.access_token.clone());
                }
            }
        }
        // Refresh: POST to token endpoint with client_credentials grant
        let mut form: Vec<(&str, &str)> = vec![
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
        ];
        if let Some(s) = scope {
            form.push(("scope", s));
        }
        let resp = http
            .post(token_url)
            .form(&form)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("oauth2 token endpoint: {}", e))?;
        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("oauth2 token endpoint error: {}", body));
        }
        #[derive(serde::Deserialize)]
        struct TokenResp {
            access_token: String,
            #[serde(default)]
            expires_in: Option<u64>,
        }
        let parsed: TokenResp = resp
            .json()
            .await
            .map_err(|e| format!("oauth2 parse: {}", e))?;
        let expires_in = parsed.expires_in.unwrap_or(3600);
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(expires_in as i64);
        // Insert into cache
        {
            let mut cache = self.inner.lock().await;
            cache.insert(
                key,
                CachedToken {
                    access_token: parsed.access_token.clone(),
                    expires_at,
                },
            );
        }
        log::info!(
            "[oauth2] obtained token from {} (expires in {}s)",
            token_url,
            expires_in
        );
        Ok(parsed.access_token)
    }
}

/// One generated tool. Holds enough state to construct an HTTP request when
/// the LLM calls it.
pub struct OpenApiTool {
    pub spec_name: String,
    pub method: reqwest::Method,
    pub path_template: String,
    pub base_url: String,
    pub wire_name: String,
    pub description: String,
    pub parameters_schema: Value,
    pub auth: Option<OpenApiAuth>,
    /// Per-agent auth overrides. Keyed by agent_id.
    pub per_agent_auth: HashMap<String, OpenApiAuth>,
    pub http_client: reqwest::Client,
    /// Names of path parameters that need substitution at call time.
    pub path_param_names: Vec<String>,
    /// Names of query parameters.
    pub query_param_names: Vec<String>,
    /// True if the operation has a JSON request body.
    pub has_body: bool,
    /// OAuth2 client_credentials token cache (server-to-server flow).
    pub oauth_cache: Option<std::sync::Arc<OAuthTokenCache>>,
    /// OAuth2 authorization_code per-user token cache (v5 Item 4).
    /// When the spec uses `OAuth2AuthCode` auth, this cache is queried
    /// with the calling user's id to get a Bearer token.
    pub auth_code_cache: Option<std::sync::Arc<crate::oauth::AuthCodeTokenCache>>,
}

#[async_trait]
impl Tool for OpenApiTool {
    fn name(&self) -> &str {
        &self.wire_name
    }

    fn schema(&self) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": self.wire_name,
                "description": self.description,
                "parameters": self.parameters_schema,
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext<'_>) -> Result<RichToolResult, String> {
        // Resolve effective auth: per-agent override > spec-level fallback
        let _effective_auth: Option<&OpenApiAuth> = self
            .per_agent_auth
            .get(_ctx.agent_id)
            .or(self.auth.as_ref());
        // Substitute path parameters
        let mut url = format!("{}{}", self.base_url.trim_end_matches('/'), self.path_template);
        for name in &self.path_param_names {
            let value = args
                .get(name)
                .and_then(|v| v.as_str())
                .map(String::from)
                .or_else(|| args.get(name).map(|v| v.to_string()))
                .ok_or_else(|| format!("missing path parameter '{}'", name))?;
            url = url.replace(&format!("{{{}}}", name), &value);
        }

        // Build query string
        let mut query_pairs: Vec<(String, String)> = Vec::new();
        for name in &self.query_param_names {
            if let Some(v) = args.get(name) {
                let s = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                query_pairs.push((name.clone(), s));
            }
        }
        // Apply query auth (per-agent or spec-level)
        if let Some(OpenApiAuth::Query { name, value }) = _effective_auth {
            query_pairs.push((name.clone(), value.clone()));
        }

        let mut req = self.http_client.request(self.method.clone(), &url);
        if !query_pairs.is_empty() {
            req = req.query(&query_pairs);
        }
        // Apply header auth (OAuth2 fetches a token from the cache first).
        // Uses the per-agent override if one exists for this agent_id.
        match _effective_auth {
            Some(OpenApiAuth::Bearer { token }) => {
                req = req.bearer_auth(token);
            }
            Some(OpenApiAuth::Header { name, value }) => {
                req = req.header(name, value);
            }
            Some(OpenApiAuth::OAuth2ClientCredentials {
                token_url,
                client_id,
                client_secret,
                scope,
            }) => {
                let cache = self
                    .oauth_cache
                    .as_ref()
                    .ok_or_else(|| "oauth2 cache not wired".to_string())?;
                let token = cache
                    .get_or_refresh(
                        token_url,
                        client_id,
                        client_secret,
                        scope.as_deref(),
                        &self.http_client,
                    )
                    .await?;
                req = req.bearer_auth(token);
            }
            Some(OpenApiAuth::OAuth2AuthCode {
                provider,
                token_url,
                client_id,
                client_secret,
                ..
            }) => {
                // Per-user token. Looks up by (ctx.user_id, provider)
                // and refreshes if within 30s of expiry. Returns a clear
                // "run /connect" error when the user hasn't authorized
                // the provider yet.
                let cache = self
                    .auth_code_cache
                    .as_ref()
                    .ok_or_else(|| "oauth2 auth_code cache not wired".to_string())?;
                let token = cache
                    .get(_ctx.user_id, provider, token_url, client_id, client_secret)
                    .await?;
                req = req.bearer_auth(token);
            }
            _ => {}
        }
        // JSON body if applicable
        if self.has_body {
            if let Some(body) = args.get("body") {
                req = req.json(body);
            }
        }

        debug!(
            "[openapi:{}] {} {} (params: {})",
            self.spec_name,
            self.method,
            url,
            args.to_string().chars().take(200).collect::<String>()
        );

        let resp = req
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| format!("read body: {}", e))?;

        let truncated = if body.len() > 8192 {
            format!("{}...\n[truncated — {} chars total]", &body[..7800], body.len())
        } else {
            body.clone()
        };

        let content = format!(
            "## {} {}\nStatus: {}\n\n```\n{}\n```",
            self.method, url, status, truncated
        );

        Ok(RichToolResult {
            content,
            citations: Vec::new(),
            artifacts: Vec::new(),
            structured: Some(json!({
                "method": self.method.to_string(),
                "url": url,
                "status": status.as_u16(),
                "body_bytes": body.len(),
            })),
        })
    }
}

/// Load all configured OpenAPI specs and return the generated tools.
/// Failures are logged and skipped — startup never fails because of one
/// bad spec.
pub fn load_from_config(
    specs: &HashMap<String, Value>,
    http_client: &reqwest::Client,
    oauth_cache: &std::sync::Arc<OAuthTokenCache>,
    auth_code_cache: Option<std::sync::Arc<crate::oauth::AuthCodeTokenCache>>,
) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for (name, raw) in specs {
        let cfg: OpenApiSpecConfig = match serde_json::from_value(raw.clone()) {
            Ok(c) => c,
            Err(e) => {
                warn!("[openapi] bad config for '{}': {}", name, e);
                continue;
            }
        };
        if !cfg.enabled {
            info!("[openapi] '{}' disabled — skipping", name);
            continue;
        }
        match parse_spec(name, &cfg, http_client, oauth_cache, auth_code_cache.as_ref()) {
            Ok(spec_tools) => {
                info!("[openapi] {} tools loaded from '{}'", spec_tools.len(), name);
                for t in spec_tools {
                    tools.push(t);
                }
            }
            Err(e) => {
                error!("[openapi] failed to load '{}': {}", name, e);
            }
        }
    }
    tools
}

fn parse_spec(
    name: &str,
    cfg: &OpenApiSpecConfig,
    http_client: &reqwest::Client,
    oauth_cache: &std::sync::Arc<OAuthTokenCache>,
    auth_code_cache: Option<&std::sync::Arc<crate::oauth::AuthCodeTokenCache>>,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let raw = std::fs::read_to_string(&cfg.spec_path)
        .map_err(|e| format!("read {}: {}", cfg.spec_path, e))?;
    // Auto-detect: JSON if it starts with `{`, otherwise try YAML
    let spec: Value = if raw.trim_start().starts_with('{') {
        serde_json::from_str(&raw).map_err(|e| format!("JSON parse: {}", e))?
    } else {
        // YAML path (serde_yaml deserializes into serde_json::Value via serde)
        serde_yaml::from_str(&raw).map_err(|e| format!("YAML parse: {}", e))?
    };

    // Determine base URL
    let base_url = cfg
        .base_url
        .clone()
        .or_else(|| {
            spec.get("servers")
                .and_then(|s| s.as_array())
                .and_then(|arr| arr.first())
                .and_then(|srv| srv.get("url"))
                .and_then(|u| u.as_str())
                .map(String::from)
        })
        .ok_or_else(|| "no base URL (set spec.servers[0].url or override in config)".to_string())?;

    let paths = spec
        .get("paths")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "spec has no `paths` object".to_string())?;

    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for (path_template, path_item) in paths {
        let item_obj = match path_item.as_object() {
            Some(o) => o,
            None => continue,
        };
        for method_str in &["get", "post", "put", "patch", "delete"] {
            let op = match item_obj.get(*method_str) {
                Some(o) => o,
                None => continue,
            };
            // Allowlist filter
            let endpoint_label = format!("{} {}", method_str.to_uppercase(), path_template);
            if !cfg.allow.is_empty() && !cfg.allow.iter().any(|a| a == &endpoint_label) {
                continue;
            }
            let operation_id = op
                .get("operationId")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| {
                    format!(
                        "{}_{}",
                        method_str,
                        path_template.replace(['/', '{', '}'], "_")
                    )
                });
            let summary = op
                .get("summary")
                .and_then(|v| v.as_str())
                .or_else(|| op.get("description").and_then(|v| v.as_str()))
                .unwrap_or("(no description)")
                .to_string();
            let description = format!("[openapi:{}] {} {} — {}", name, method_str.to_uppercase(), path_template, summary);

            // Collect parameters
            let mut path_param_names: Vec<String> = Vec::new();
            let mut query_param_names: Vec<String> = Vec::new();
            // Build properties + required separately, then assemble at the end
            // to avoid double-mutable-borrow on a single Value.
            let mut props_map = serde_json::Map::new();
            let mut required: Vec<Value> = Vec::new();

            if let Some(param_arr) = op.get("parameters").and_then(|v| v.as_array()) {
                for p in param_arr {
                    let pname = match p.get("name").and_then(|v| v.as_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    let pin = p.get("in").and_then(|v| v.as_str()).unwrap_or("");
                    let pdesc = p.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    let pschema = p
                        .get("schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "string"}));
                    let mut prop = pschema;
                    if let Some(o) = prop.as_object_mut() {
                        o.insert("description".to_string(), Value::String(pdesc.to_string()));
                    }
                    props_map.insert(pname.clone(), prop);
                    if p.get("required").and_then(|v| v.as_bool()).unwrap_or(false) {
                        required.push(Value::String(pname.clone()));
                    }
                    if pin == "path" {
                        path_param_names.push(pname);
                    } else if pin == "query" {
                        query_param_names.push(pname);
                    }
                }
            }

            // Request body (JSON only)
            let has_body = op
                .get("requestBody")
                .and_then(|rb| rb.get("content"))
                .and_then(|c| c.get("application/json"))
                .is_some();
            if has_body {
                props_map.insert("body".to_string(), json!({
                    "type": "object",
                    "description": "JSON request body"
                }));
                required.push(Value::String("body".to_string()));
            }

            // Wire name: openapi__<spec>__<operation_id>, sanitized for OpenAI naming
            let wire_name: String = format!("openapi__{}__{}", name, operation_id)
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect();
            let wire_name: String = wire_name.chars().take(64).collect();

            let params_schema = json!({
                "type": "object",
                "properties": props_map,
                "required": required,
            });

            let method = match *method_str {
                "get" => reqwest::Method::GET,
                "post" => reqwest::Method::POST,
                "put" => reqwest::Method::PUT,
                "patch" => reqwest::Method::PATCH,
                "delete" => reqwest::Method::DELETE,
                _ => continue,
            };

            let tool = OpenApiTool {
                spec_name: name.to_string(),
                method,
                path_template: path_template.clone(),
                base_url: base_url.clone(),
                wire_name,
                description,
                parameters_schema: params_schema,
                auth: cfg.auth.clone(),
                per_agent_auth: cfg.per_agent_auth.clone(),
                http_client: http_client.clone(),
                path_param_names,
                query_param_names,
                has_body,
                oauth_cache: Some(std::sync::Arc::clone(oauth_cache)),
                auth_code_cache: auth_code_cache.cloned(),
            };
            tools.push(Arc::new(tool) as Arc<dyn Tool>);
        }
    }
    Ok(tools)
}
