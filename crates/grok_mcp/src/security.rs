//! Security validation for MCP servers (paths, commands, URLs).

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::{McpServerConfigExt, McpTransport};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SecurityVerdict {
    Allow,
    Deny { reason: String },
    Warn { reason: String },
}

/// Sensitive path prefixes that must never be exposed via filesystem MCP.
const DENIED_PATH_SUFFIXES: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".config/gcloud",
    ".docker/config.json",
    "Library/Keychains",
    ".grok/mcp_credentials.json",
    ".bombcode/mcp_credentials.json",
];

const DENIED_EXACT: &[&str] = &["/", "/etc", "/private/etc", "/System", "/usr", "/bin", "/sbin"];

/// Validate filesystem MCP allowed paths.
pub fn validate_filesystem_paths(paths: &[PathBuf], read_only: bool) -> SecurityVerdict {
    if paths.is_empty() {
        return SecurityVerdict::Deny {
            reason: "at least one allowed path is required".into(),
        };
    }
    for p in paths {
        if !p.is_absolute() {
            return SecurityVerdict::Deny {
                reason: format!("path must be absolute: {}", p.display()),
            };
        }
        if has_parent_escape(p) {
            return SecurityVerdict::Deny {
                reason: format!("path traversal not allowed: {}", p.display()),
            };
        }
        if !p.exists() {
            return SecurityVerdict::Deny {
                reason: format!("path does not exist: {}", p.display()),
            };
        }
        // Canonicalize so a symlink (~/notes -> ~/.ssh) can't smuggle a
        // denied target past the string checks.
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        for candidate in [p, &canonical] {
            let s = candidate.to_string_lossy();
            for exact in DENIED_EXACT {
                if s == *exact {
                    return SecurityVerdict::Deny {
                        reason: format!("refusing sensitive path: {exact}"),
                    };
                }
            }
            // Component-suffix match, not substring — `/Users/me/.ssh-notes`
            // is fine; `/Users/me/.ssh` and children are not.
            for denied in DENIED_PATH_SUFFIXES {
                let denied_parts: Vec<&str> = denied.split('/').collect();
                let parts: Vec<String> = candidate
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                if parts
                    .windows(denied_parts.len())
                    .any(|w| w.iter().map(String::as_str).eq(denied_parts.iter().copied()))
                {
                    return SecurityVerdict::Deny {
                        reason: format!("refusing sensitive path containing `{denied}`"),
                    };
                }
            }
        }
    }
    if !read_only {
        SecurityVerdict::Warn {
            reason: "filesystem MCP has write access; prefer read-only when possible".into(),
        }
    } else {
        SecurityVerdict::Allow
    }
}

fn has_parent_escape(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Validate custom / generic MCP server configs.
pub fn validate_custom_server(cfg: &McpServerConfigExt) -> SecurityVerdict {
    if cfg.name.is_empty()
        || !cfg
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return SecurityVerdict::Deny {
            reason: format!("invalid MCP name: {}", cfg.name),
        };
    }

    match cfg.transport {
        McpTransport::Stdio => {
            let Some(ref cmd) = cfg.command else {
                return SecurityVerdict::Deny {
                    reason: "stdio transport requires command".into(),
                };
            };
            if cmd.trim().is_empty() {
                return SecurityVerdict::Deny {
                    reason: "command must not be empty".into(),
                };
            }
            // Block obvious shell injection in command itself
            if cmd.contains('|') || cmd.contains(';') || cmd.contains('`') || cmd.contains('$') {
                return SecurityVerdict::Deny {
                    reason: "command contains shell metacharacters".into(),
                };
            }
            for a in &cfg.args {
                if a.contains('\0') {
                    return SecurityVerdict::Deny {
                        reason: "args contain NUL".into(),
                    };
                }
                // Args are exec'd without a shell locally, but they also get
                // mirrored into `grok mcp add` — keep metachars out of them.
                if a.contains('`') || a.contains("$(") || a.contains(";\n") {
                    return SecurityVerdict::Deny {
                        reason: format!("arg contains shell metacharacters: {a}"),
                    };
                }
            }
            if cfg.high_risk || cfg.kind == "browser" || cfg.kind == "custom" {
                return SecurityVerdict::Warn {
                    reason: "custom/high-risk stdio MCP requires explicit user approval".into(),
                };
            }
            SecurityVerdict::Allow
        }
        McpTransport::Http | McpTransport::Sse => {
            let Some(ref url) = cfg.url else {
                return SecurityVerdict::Deny {
                    reason: "http/sse transport requires url".into(),
                };
            };
            if !is_allowed_mcp_url(url) {
                return SecurityVerdict::Deny {
                    reason: "only https (or loopback http) URLs are allowed".into(),
                };
            }
            SecurityVerdict::Allow
        }
    }
}

/// https anywhere; plain http only to a real loopback host. Parses the host
/// properly — prefix/substring checks accepted `http://127.0.0.1.evil.com`.
pub fn is_allowed_mcp_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "https" => true,
        "http" => matches!(
            parsed.host_str(),
            Some("localhost") | Some("127.0.0.1") | Some("[::1]") | Some("::1")
        ),
        _ => false,
    }
}

/// Combined validation used by McpManager::add.
pub fn validate_server(cfg: &McpServerConfigExt) -> Result<(), String> {
    if cfg.kind == "filesystem" {
        match validate_filesystem_paths(&cfg.allowed_paths, cfg.read_only) {
            SecurityVerdict::Deny { reason } => return Err(reason),
            SecurityVerdict::Warn { .. } | SecurityVerdict::Allow => {}
        }
    }
    match validate_custom_server(cfg) {
        SecurityVerdict::Deny { reason } => Err(reason),
        SecurityVerdict::Warn { .. } | SecurityVerdict::Allow => Ok(()),
    }
}

/// Mask secret-looking values for logs/UI.
pub fn mask_secret(value: &str) -> String {
    // char-based, not byte-sliced — multibyte input must not panic.
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 8 {
        return "****".into();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 2..].iter().collect();
    format!("{head}…{tail}")
}

pub fn looks_like_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("token")
        || k.contains("secret")
        || k.contains("password")
        || k.contains("api_key")
        || k.contains("apikey")
        || k.ends_with("_key")
        || k.contains("authorization")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn denies_root_and_ssh() {
        assert!(matches!(
            validate_filesystem_paths(&[PathBuf::from("/")], true),
            SecurityVerdict::Deny { .. }
        ));
        assert!(matches!(
            validate_filesystem_paths(&[PathBuf::from("/Users/me/.ssh")], true),
            SecurityVerdict::Deny { .. }
        ));
    }

    #[test]
    fn allows_existing_temp() {
        let dir = tempdir().unwrap();
        let v = validate_filesystem_paths(&[dir.path().to_path_buf()], true);
        assert_eq!(v, SecurityVerdict::Allow);
    }

    #[test]
    fn rejects_http_non_local() {
        let cfg = McpServerConfigExt {
            name: "x".into(),
            transport: McpTransport::Http,
            url: Some("http://evil.example".into()),
            ..Default::default()
        };
        assert!(matches!(
            validate_custom_server(&cfg),
            SecurityVerdict::Deny { .. }
        ));
    }

    #[test]
    fn accepts_https() {
        let cfg = McpServerConfigExt {
            name: "linear".into(),
            transport: McpTransport::Http,
            url: Some("https://mcp.linear.app/mcp".into()),
            kind: "linear".into(),
            ..Default::default()
        };
        assert_eq!(validate_custom_server(&cfg), SecurityVerdict::Allow);
    }
}
