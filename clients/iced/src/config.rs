// SPDX-FileCopyrightText: 2026 Chris Palmer <chris@red-oxide.org>
// SPDX-License-Identifier: GPL-3.0-only

//! Where the server address and token live. These are the same files the
//! terminal client uses, so a machine paired with `pressbox pair` (or set up
//! by the Home Manager module) needs no setup here, and the reverse.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `~/.config/pressbox/client.json`, written by pressbox's Home Manager
/// module or by this app after pairing.
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClientConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
}

impl ClientConfig {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("pressbox/client.json"))
    }

    /// `$XDG_STATE_HOME/pressbox/token`, where `pressbox pair` saves the
    /// token when no `tokenFile` is set.
    pub fn default_token_path() -> Option<PathBuf> {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .map(|d| d.join("pressbox/token"))
    }

    /// Reads the config file; a missing file is an empty config.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// The token file this config points at, or the default one.
    pub fn token_path(&self) -> Option<PathBuf> {
        self.token_file.clone().or_else(Self::default_token_path)
    }

    /// Reads the token, if there is one.
    pub fn token(&self) -> Option<String> {
        let path = self.token_path()?;
        let token = std::fs::read_to_string(path).ok()?;
        let token = token.trim();
        (!token.is_empty()).then(|| token.to_owned())
    }
}

/// Everything needed to talk to the server, if this machine is paired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub server: String,
    pub token: String,
}

/// Loads the saved session: the server from `client.json` and its token.
pub fn load_session() -> (ClientConfig, Option<Session>) {
    let config = ClientConfig::default_path()
        .and_then(|p| ClientConfig::load(&p).ok())
        .unwrap_or_default();
    let session = match (&config.server, config.token()) {
        (Some(server), Some(token)) => Some(Session {
            server: server.clone(),
            token,
        }),
        _ => None,
    };
    (config, session)
}

/// Saves a newly paired session. The token goes to the token file (mode
/// 0600); the server is written to `client.json` only if it has none, so a
/// file managed by Home Manager is left alone.
pub fn save_session(config: &ClientConfig, session: &Session) -> Result<()> {
    let token_path = config
        .token_path()
        .context("no state directory for the token")?;
    write_private(&token_path, &format!("{}\n", session.token))?;

    if config.server.as_deref() != Some(session.server.as_str()) {
        let path = ClientConfig::default_path().context("no config directory")?;
        let updated = ClientConfig {
            server: Some(session.server.clone()),
            token_file: config.token_file.clone(),
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&updated)? + "\n")
            .with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

fn write_private(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options
        .open(path)
        .and_then(|mut f| f.write_all(contents.as_bytes()))
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_home_manager_config() {
        let dir = std::env::temp_dir().join(format!("pressbox-iced-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("client.json");
        std::fs::write(
            &path,
            r#"{"server":"https://rss.example","tokenFile":"/run/secrets/pressbox"}"#,
        )
        .unwrap();
        let config = ClientConfig::load(&path).unwrap();
        assert_eq!(config.server.as_deref(), Some("https://rss.example"));
        assert_eq!(
            config.token_path().as_deref(),
            Some(Path::new("/run/secrets/pressbox"))
        );
        assert_eq!(
            ClientConfig::load(&dir.join("missing.json")).unwrap(),
            ClientConfig::default()
        );

        let token = dir.join("token");
        write_private(&token, "abc\n").unwrap();
        let config = ClientConfig {
            server: None,
            token_file: Some(token.clone()),
        };
        assert_eq!(config.token().as_deref(), Some("abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&token).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
