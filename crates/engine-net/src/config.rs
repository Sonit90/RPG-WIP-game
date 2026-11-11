//! Where the realm is: four addresses, never compiled in for production.
//!
//! Resolution order, later wins: development defaults → a JSON file (`--realm-config`) →
//! environment variables (`SPACETIMEDB_URL`, `SPACETIMEDB_DATABASE`, `AUTH_API_URL`,
//! `PERSISTENCE_API_URL`). The defaults point at this machine, which is what `tools/realm-dev`
//! starts; staging and production are always a file or the environment.

use std::path::Path;

use serde::Deserialize;

/// The four addresses a client needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmEndpoints {
    /// The SpacetimeDB server, `http(s)://host:port`.
    pub spacetimedb_url: String,
    /// The realm database's name on it.
    pub spacetimedb_database: String,
    /// The authentication API's base URL.
    pub auth_api_url: String,
    /// The persistence API's base URL (the same service as auth in the default deployment).
    pub persistence_api_url: String,
}

#[derive(Deserialize, Default)]
struct EndpointsFile {
    spacetimedb_url: Option<String>,
    spacetimedb_database: Option<String>,
    auth_api_url: Option<String>,
    persistence_api_url: Option<String>,
}

impl RealmEndpoints {
    /// Everything on this machine, as `tools/realm-dev.ps1` starts it.
    #[must_use]
    pub fn development() -> Self {
        Self {
            spacetimedb_url: "http://127.0.0.1:3000".into(),
            spacetimedb_database: "realm".into(),
            auth_api_url: "http://127.0.0.1:8080".into(),
            persistence_api_url: "http://127.0.0.1:8080".into(),
        }
    }

    /// Resolves the endpoints: defaults, then `file` if given, then the environment.
    ///
    /// # Errors
    ///
    /// The file cannot be read or parsed, or a resulting address is not an http(s) URL.
    pub fn resolve(file: Option<&Path>) -> Result<Self, String> {
        let mut endpoints = Self::development();
        if let Some(path) = file {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            endpoints.apply_file(&text)?;
        }
        endpoints.apply(|key| std::env::var(key).ok());
        endpoints.validate()?;
        Ok(endpoints)
    }

    /// Overrides from a JSON document's fields.
    ///
    /// # Errors
    ///
    /// The text is not a JSON object of string fields.
    pub fn apply_file(&mut self, text: &str) -> Result<(), String> {
        let file: EndpointsFile =
            serde_json::from_str(text).map_err(|error| format!("realm config: {error}"))?;
        let set = |target: &mut String, value: Option<String>| {
            if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
                *target = value.trim().trim_end_matches('/').to_owned();
            }
        };
        set(&mut self.spacetimedb_url, file.spacetimedb_url);
        set(&mut self.spacetimedb_database, file.spacetimedb_database);
        set(&mut self.auth_api_url, file.auth_api_url);
        set(&mut self.persistence_api_url, file.persistence_api_url);
        Ok(())
    }

    /// Overrides from environment-style lookups.
    pub fn apply(&mut self, lookup: impl Fn(&str) -> Option<String>) {
        let set = |target: &mut String, key: &str| {
            if let Some(value) = lookup(key).filter(|v| !v.trim().is_empty()) {
                *target = value.trim().trim_end_matches('/').to_owned();
            }
        };
        set(&mut self.spacetimedb_url, "SPACETIMEDB_URL");
        set(&mut self.spacetimedb_database, "SPACETIMEDB_DATABASE");
        set(&mut self.auth_api_url, "AUTH_API_URL");
        set(&mut self.persistence_api_url, "PERSISTENCE_API_URL");
    }

    /// Whether every address is usable.
    ///
    /// # Errors
    ///
    /// Names the first address that is not an http(s) URL, or an empty database name.
    pub fn validate(&self) -> Result<(), String> {
        for (name, url) in [
            ("SPACETIMEDB_URL", &self.spacetimedb_url),
            ("AUTH_API_URL", &self.auth_api_url),
            ("PERSISTENCE_API_URL", &self.persistence_api_url),
        ] {
            let lower = url.to_ascii_lowercase();
            let rest = lower
                .strip_prefix("https://")
                .or_else(|| lower.strip_prefix("http://"));
            match rest {
                Some(host) if !host.is_empty() && !host.contains(char::is_whitespace) => {}
                _ => return Err(format!("{name} must be an http(s) URL, got {url:?}")),
            }
        }
        if self.spacetimedb_database.trim().is_empty() {
            return Err("SPACETIMEDB_DATABASE must not be empty".into());
        }
        Ok(())
    }

    /// Whether any address talks to a remote host in clear text.
    ///
    /// Plain HTTP is fine against this machine; against anything else a password would cross
    /// the network unencrypted. The client warns, loudly, rather than refusing, so a LAN test
    /// still works.
    #[must_use]
    pub fn insecure_remote(&self) -> bool {
        [
            &self.spacetimedb_url,
            &self.auth_api_url,
            &self.persistence_api_url,
        ]
        .into_iter()
        .any(|url| {
            let lower = url.to_ascii_lowercase();
            lower.starts_with("http://")
                && !lower.starts_with("http://127.0.0.1")
                && !lower.starts_with("http://localhost")
                && !lower.starts_with("http://[::1]")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_overrides_defaults() {
        let mut endpoints = RealmEndpoints::development();
        endpoints.apply(|key| match key {
            "SPACETIMEDB_URL" => Some("https://realm.example.com/".into()),
            "AUTH_API_URL" => Some("   ".into()),
            _ => None,
        });
        assert_eq!(endpoints.spacetimedb_url, "https://realm.example.com");
        assert_eq!(endpoints.auth_api_url, "http://127.0.0.1:8080");
        assert!(endpoints.validate().is_ok());
    }

    #[test]
    fn files_override_defaults() {
        let mut endpoints = RealmEndpoints::development();
        endpoints
            .apply_file(r#"{"spacetimedb_database":"realm-staging"}"#)
            .ok();
        assert_eq!(endpoints.spacetimedb_database, "realm-staging");
        assert!(endpoints.apply_file("not json").is_err());
    }

    #[test]
    fn bad_addresses_are_refused() {
        let mut endpoints = RealmEndpoints::development();
        endpoints.auth_api_url = "ftp://nope".into();
        assert!(endpoints.validate().is_err());
    }

    #[test]
    fn remote_clear_text_is_flagged() {
        assert!(!RealmEndpoints::development().insecure_remote());
        let mut endpoints = RealmEndpoints::development();
        endpoints.auth_api_url = "http://203.0.113.5:8080".into();
        assert!(endpoints.insecure_remote());
    }
}
