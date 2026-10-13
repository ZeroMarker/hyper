//! Named provider settings and an explicit, single-request connection diagnostic.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Profile {
    protocol: String,
    base_url: String,
    model: String,
    auth: Auth,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    endpoints: BTreeMap<String, String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
enum Auth {
    Env { variable: String },
    Config { api_key: String },
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

pub(super) fn validate_url(value: &str) -> Result<()> {
    // Credentials in URLs would reach diagnostics and effective-config events.
    let url = reqwest::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid provider URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("provider URL must be HTTP(S) without credentials, query or fragment");
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        bail!("profile name must contain 1-64 ASCII letters, digits, '_' or '-'");
    }
    Ok(())
}

pub(super) fn resolve(
    env: &impl Fn(&str) -> Option<String>,
    stored: &StoredConfig,
) -> Result<Settings> {
    let name = stored
        .active_profile
        .as_deref()
        .context("no active profile")?;
    validate_name(name)?;
    let profile = stored
        .profiles
        .get(name)
        .context("selected provider profile does not exist")?;
    let (api_key, auth_source) = if let Some(key) = nonempty(env("DEEPSEEK_API_KEY")) {
        (key, "env:DEEPSEEK_API_KEY".into())
    } else {
        match &profile.auth {
            Auth::Env { variable } => {
                if variable.is_empty()
                    || !variable
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    bail!("invalid profile authentication environment variable name");
                }
                (
                    nonempty(env(variable)).context(
                        "authentication: profile environment variable is unset or empty",
                    )?,
                    format!("env:{variable}"),
                )
            }
            Auth::Config { api_key } => (
                nonempty(Some(api_key.clone()))
                    .context("authentication: profile API key is empty")?,
                "config:profile".into(),
            ),
        }
    };
    let base_url =
        nonempty(env("DEEPSEEK_BASE_URL")).unwrap_or_else(|| profile.base_url.trim().into());
    let model = nonempty(env("DEEPSEEK_MODEL")).unwrap_or_else(|| profile.model.trim().into());
    if model.is_empty() {
        bail!("profile model must not be empty");
    }
    validate_url(&base_url)?;
    let protocol = resolve_protocol(
        nonempty(env("DEEPSEEK_PROTOCOL")).or_else(|| Some(profile.protocol.clone())),
        &base_url,
        &model,
    )?;
    let endpoints = resolve_endpoints(&base_url, &profile.endpoints)?;
    Ok(Settings {
        api_key,
        base_url,
        model,
        protocol,
        profile: Some(name.into()),
        auth_source,
        endpoints,
    })
}

fn resolve_endpoints(
    base_url: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut endpoints = BTreeMap::new();
    for (protocol, override_path) in overrides {
        if Protocol::parse(protocol).is_none()
            || !["chat", "responses", "messages"].contains(&protocol.as_str())
        {
            bail!("unknown endpoint protocol; expected chat, responses or messages");
        }
        let url = if override_path.starts_with("/") {
            reqwest::Url::parse(base_url)?
                .join(override_path)
                .map_err(|_| anyhow::anyhow!("invalid endpoint path"))?
                .to_string()
        } else if override_path.contains("://") {
            override_path.clone()
        } else {
            format!("{}/{}", base_url.trim_end_matches('/'), override_path)
        };
        validate_url(&url)?;
        endpoints.insert(protocol.clone(), url);
    }
    Ok(endpoints)
}

pub(crate) fn list() -> Result<serde_json::Value> {
    let stored = read_stored_config(&config_path()?)?.unwrap_or_default();
    for name in stored.profiles.keys() {
        validate_name(name)?;
    }
    for profile in stored.profiles.values() {
        validate_url(&profile.base_url)?;
        resolve_endpoints(&profile.base_url, &profile.endpoints)?;
    }
    // Listing works without loading a credential and never serializes Auth.
    Ok(
        json!({"activeProfile":stored.active_profile,"profiles":stored.profiles.iter().map(|(name, profile)| {
        json!({"name":name,"protocol":profile.protocol,"baseUrl":profile.base_url,"model":profile.model,"endpoints":profile.endpoints,
            "authSource":match &profile.auth { Auth::Env { variable } => format!("env:{variable}"), Auth::Config { .. } => "config:profile".into() }})
    }).collect::<Vec<_>>(),"legacy":!stored.deepseek_api_key.is_empty() || stored.base_url.is_some() || stored.model.is_some()}),
    )
}

fn select(path: &Path, name: &str) -> Result<()> {
    validate_name(name)?;
    let mut stored = read_stored_config(path)?.context("no provider configuration file")?;
    if !stored.profiles.contains_key(name) {
        bail!("provider profile does not exist");
    }
    stored.active_profile = Some(name.into());
    save_config(path, &stored)
}

pub(crate) fn use_profile(name: &str) -> Result<()> {
    select(&config_path()?, name)
}

/// The model the next run will ask for, resolved without loading a credential
/// so the TUI can show it even before a key is configured.
pub(crate) fn current_model() -> String {
    let stored = config_path()
        .ok()
        .and_then(|path| read_stored_config(&path).ok().flatten())
        .unwrap_or_default();
    model_for(&|key| std::env::var(key).ok(), stored)
}

fn model_for(env: &impl Fn(&str) -> Option<String>, stored: StoredConfig) -> String {
    let from_store = match stored.active_profile.as_deref() {
        Some(name) => stored
            .profiles
            .get(name)
            .map(|profile| profile.model.clone()),
        None => stored.model,
    };
    nonempty(env("DEEPSEEK_MODEL"))
        .or_else(|| nonempty(from_store))
        .unwrap_or_else(|| DEFAULT_MODEL.into())
}

/// Classify only status and structured error codes, never arbitrary provider text.
pub(super) fn status_category(status: StatusCode, body: &str) -> &'static str {
    if matches!(status.as_u16(), 401 | 403) {
        return "authentication";
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return "rate_limit";
    }
    if matches!(status.as_u16(), 408 | 504) {
        return "timeout";
    }
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    let code = value
        .pointer("/error/code")
        .or_else(|| value.pointer("/error/type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match code {
        "model_not_found" | "invalid_model" | "unsupported_model" => "model",
        "invalid_api_key" | "authentication_error" | "permission_error" => "authentication",
        "rate_limit_exceeded" | "rate_limit_error" => "rate_limit",
        "unsupported_protocol" | "invalid_request_error" | "invalid_request" => "protocol",
        _ if matches!(status.as_u16(), 404 | 405) => "path",
        _ if matches!(status.as_u16(), 400 | 415 | 422) => "protocol",
        _ => "http",
    }
}

fn probe(config: &DeepSeekConfig) -> Result<()> {
    let messages = [json!({"role":"user","content":"Reply OK."})];
    let body = match config.protocol {
        Protocol::Chat => chat_body(config, &messages, None, false),
        Protocol::Responses => responses_body(config, &messages, None, false),
        Protocol::Messages => messages_body(config, &messages, None, false),
    };
    // No retries: a diagnostic must report rate limits and timeouts directly.
    let response = config
        .client
        .post(config.request_endpoint())
        .json(&body)
        .send()
        .map_err(|error| {
            anyhow::anyhow!(
                "profile test: {}",
                if error.is_timeout() {
                    "timeout"
                } else {
                    "connection"
                }
            )
        })?;
    let status = response.status();
    let body = response.text().map_err(|error| {
        anyhow::anyhow!(
            "profile test: {}",
            if error.is_timeout() {
                "timeout"
            } else {
                "connection"
            }
        )
    })?;
    if !status.is_success() {
        bail!(
            "profile test: {} (HTTP {})",
            status_category(status, &body),
            status.as_u16()
        );
    }
    let value: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| anyhow::anyhow!("profile test: protocol (invalid JSON)"))?;
    let valid = match config.protocol {
        Protocol::Chat => value
            .pointer("/choices/0/message")
            .is_some_and(serde_json::Value::is_object),
        Protocol::Responses => value["output"].is_array() && value["status"] == "completed",
        Protocol::Messages => {
            value["content"].is_array()
                && matches!(
                    value["stop_reason"].as_str(),
                    Some("end_turn" | "stop_sequence")
                )
        }
    };
    if !valid {
        bail!("profile test: protocol (unexpected response shape or completion)");
    }
    if let Some(model) = value["model"].as_str()
        && model != config.model
    {
        bail!("profile test: model (response model differs from requested model)");
    }
    Ok(())
}

pub(crate) fn test_profile(name: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
    if timeout_ms == 0 {
        bail!("profile test timeout must be positive");
    }
    let mut stored = read_stored_config(&config_path()?)?.unwrap_or_default();
    if let Some(name) = name {
        validate_name(name)?;
        stored.active_profile = Some(name.into());
    }
    let settings = resolve_settings(|key| env::var(key).ok(), Some(stored)).map_err(|error| {
        anyhow::anyhow!(
            "profile test: {}",
            if error.to_string().contains("authentication")
                || error.to_string().contains("API key is not configured")
            {
                "authentication (missing credential)"
            } else {
                "configuration (check profile settings)"
            }
        )
    })?;
    let mut config = DeepSeekConfig::with_timeout(settings, Duration::from_millis(timeout_ms))?;
    config.budget.output_tokens = 32;
    probe(&config)?;
    Ok(json!({"status":"ok","config":config.effective_config()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, net::TcpListener};

    fn stored() -> StoredConfig {
        serde_json::from_value(json!({
            "deepseek_api_key":"legacy-secret","base_url":"https://legacy.example/v1",
            "active_profile":"local", "profiles":{
                "local":{"protocol":"chat","base_url":"https://profile.example/v1","model":"stub",
                    "auth":{"source":"env","variable":"LOCAL_API_KEY"},
                    "endpoints":{"chat":"custom/chat","responses":"/custom/responses","messages":"https://messages.example/custom"}},
                "saved":{"protocol":"messages","base_url":"https://saved.example","model":"saved-model",
                    "auth":{"source":"config","api_key":"saved-secret"}}
            }
        })).unwrap()
    }

    #[test]
    fn current_model_follows_env_then_active_profile_then_legacy() {
        let none = |_: &str| None;
        assert_eq!(model_for(&none, stored()), "stub");
        let mut saved = stored();
        saved.active_profile = Some("saved".into());
        assert_eq!(model_for(&none, saved), "saved-model");
        let overridden = |key: &str| (key == "DEEPSEEK_MODEL").then(|| "env-model".into());
        assert_eq!(model_for(&overridden, stored()), "env-model");
        let legacy = StoredConfig {
            model: Some("legacy-model".into()),
            ..Default::default()
        };
        assert_eq!(model_for(&none, legacy), "legacy-model");
        assert_eq!(model_for(&none, StoredConfig::default()), DEFAULT_MODEL);
    }

    #[test]
    fn named_settings_preserve_environment_priority_and_auth_provenance() {
        let saved = stored();
        let config = resolve_settings(
            |key| (key == "LOCAL_API_KEY").then(|| "local-secret".into()),
            Some(saved.clone()),
        )
        .unwrap();
        assert_eq!(config.api_key, "local-secret");
        assert_eq!(config.auth_source, "env:LOCAL_API_KEY");
        assert_eq!(config.profile.as_deref(), Some("local"));
        assert_eq!(
            config.endpoints["chat"],
            "https://profile.example/v1/custom/chat"
        );
        assert_eq!(
            config.endpoints["responses"],
            "https://profile.example/custom/responses"
        );
        assert_eq!(
            config.endpoints["messages"],
            "https://messages.example/custom"
        );
        let override_config = resolve_settings(
            |key| match key {
                "DEEPSEEK_API_KEY" => Some("override-secret".into()),
                "DEEPSEEK_BASE_URL" => Some("https://override.example/api".into()),
                "DEEPSEEK_MODEL" => Some("override-model".into()),
                "DEEPSEEK_PROTOCOL" => Some("responses".into()),
                _ => None,
            },
            Some(saved.clone()),
        )
        .unwrap();
        assert_eq!(override_config.auth_source, "env:DEEPSEEK_API_KEY");
        assert_eq!(override_config.api_key, "override-secret");
        assert_eq!(override_config.protocol, Protocol::Responses);
        assert_eq!(override_config.model, "override-model");
        assert_eq!(
            override_config.endpoints["chat"],
            "https://override.example/api/custom/chat"
        );
        let mut saved = saved;
        saved.active_profile = Some("saved".into());
        let config = resolve_settings(|_| None, Some(saved)).unwrap();
        assert_eq!(config.api_key, "saved-secret");
        assert_eq!(config.auth_source, "config:profile");
    }

    #[test]
    fn selection_preserves_legacy_keys_profiles_and_private_file_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        save_config(&path, &stored()).unwrap();
        select(&path, "saved").unwrap();
        let saved = read_stored_config(&path).unwrap().unwrap();
        assert_eq!(saved.active_profile.as_deref(), Some("saved"));
        assert_eq!(saved.deepseek_api_key, "legacy-secret");
        assert_eq!(saved.profiles.len(), 2);
        let before = fs::read(&path).unwrap();
        assert!(select(&path, "unknown").is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn bad_profiles_and_credential_urls_fail_closed() {
        assert!(resolve_settings(|_| None, Some(stored())).is_err());
        let mut saved = stored();
        saved.active_profile = Some("absent".into());
        assert!(resolve_settings(|_| Some("secret".into()), Some(saved)).is_err());
        for url in [
            "https://user:secret@example.com",
            "https://example.com?key=secret",
            "https://example.com/#secret",
            "file:///secret",
        ] {
            let error = validate_url(url).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        let mut saved = stored();
        saved
            .profiles
            .get_mut("local")
            .unwrap()
            .endpoints
            .insert("typo".into(), "path".into());
        assert!(resolve_settings(|_| Some("secret".into()), Some(saved)).is_err());
    }

    fn stub(
        status: u16,
        body: String,
        delay: Duration,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|s| s.trim().parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= length {
                        break;
                    }
                }
            }
            std::thread::sleep(delay);
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}"), handle)
    }

    fn config(protocol: Protocol, url: &str, timeout: Duration) -> DeepSeekConfig {
        DeepSeekConfig::with_timeout(
            Settings {
                api_key: "probe-secret".into(),
                base_url: url.into(),
                model: "stub".into(),
                protocol,
                profile: Some("local".into()),
                auth_source: "env:LOCAL_API_KEY".into(),
                endpoints: BTreeMap::from([(protocol.as_str().into(), format!("{url}/custom"))]),
            },
            timeout,
        )
        .unwrap()
    }

    #[test]
    fn diagnostic_uses_each_protocol_override_and_never_exposes_credentials() {
        for (protocol, reply) in [
            (
                Protocol::Chat,
                json!({"model":"stub","choices":[{"message":{"role":"assistant","content":"OK"}}]}),
            ),
            (
                Protocol::Responses,
                json!({"model":"stub","status":"completed","output":[]}),
            ),
            (
                Protocol::Messages,
                json!({"model":"stub","stop_reason":"end_turn","content":[]}),
            ),
        ] {
            let (url, handle) = stub(200, reply.to_string(), Duration::ZERO);
            let config = config(protocol, &url, Duration::from_secs(2));
            probe(&config).unwrap();
            let request = handle.join().unwrap();
            assert!(request.starts_with("POST /custom HTTP/1.1"));
            assert!(request.contains(if protocol == Protocol::Messages {
                "x-api-key: probe-secret"
            } else {
                "authorization: Bearer probe-secret"
            }));
            let body: serde_json::Value =
                serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["model"], "stub");
            assert!(
                body[match protocol {
                    Protocol::Chat => "messages",
                    Protocol::Responses => "input",
                    Protocol::Messages => "messages",
                }]
                .is_array()
            );
            assert!(!format!("{config:?}").contains("probe-secret"));
            assert!(
                !config
                    .effective_config()
                    .to_string()
                    .contains("probe-secret")
            );
        }
    }

    #[test]
    fn diagnostic_distinguishes_auth_path_protocol_model_rate_limit_and_timeout() {
        for (status, code, expected) in [
            (401, "", "authentication"),
            (403, "", "authentication"),
            (404, "", "path"),
            (400, "unsupported_protocol", "protocol"),
            (404, "model_not_found", "model"),
            (429, "rate_limit_exceeded", "rate_limit"),
            (408, "", "timeout"),
            (504, "", "timeout"),
        ] {
            let reply = json!({"error":{"code":code,"message":"echo probe-secret"}}).to_string();
            let (url, handle) = stub(status, reply, Duration::ZERO);
            let error = probe(&config(Protocol::Chat, &url, Duration::from_secs(2)))
                .unwrap_err()
                .to_string();
            handle.join().unwrap();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("probe-secret"));
        }
        let (url, handle) = stub(200, "{}".into(), Duration::from_millis(150));
        let error = probe(&config(Protocol::Chat, &url, Duration::from_millis(30)))
            .unwrap_err()
            .to_string();
        handle.join().unwrap();
        assert!(error.contains("timeout"), "{error}");
    }

    #[test]
    fn diagnostic_refuses_wrong_success_protocol_and_model() {
        for (body, expected) in [
            (json!({"output":[],"status":"completed"}), "protocol"),
            (json!({"choices":[{"message":{}}],"model":"wrong"}), "model"),
        ] {
            let (url, handle) = stub(200, body.to_string(), Duration::ZERO);
            let error = probe(&config(Protocol::Chat, &url, Duration::from_secs(2)))
                .unwrap_err()
                .to_string();
            handle.join().unwrap();
            assert!(error.contains(expected), "{error}");
        }
    }
}
