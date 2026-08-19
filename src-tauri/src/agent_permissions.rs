//! Permission modes each supported client actually exposes.
//!
//! Every entry was read from a real installation of that CLI. A client whose
//! modes have not been verified declares none rather than a guess, because a
//! mode the runtime does not accept fails only when the Agent is already running.

/// One mode a client's CLI accepts, with the arguments that select it.
pub struct PermissionMode {
    pub id: &'static str,
    pub args: &'static [&'static str],
}

pub struct ClientPermissions {
    pub modes: &'static [PermissionMode],
    /// Applied when a call names no mode, so a delegated Agent is as capable as
    /// the same Agent run by hand.
    pub default_mode: Option<&'static str>,
}

const CLAUDE: &[PermissionMode] = &[
    PermissionMode {
        id: "acceptEdits",
        args: &["--permission-mode", "acceptEdits"],
    },
    PermissionMode {
        id: "auto",
        args: &["--permission-mode", "auto"],
    },
    PermissionMode {
        id: "bypassPermissions",
        args: &["--permission-mode", "bypassPermissions"],
    },
    PermissionMode {
        id: "manual",
        args: &["--permission-mode", "manual"],
    },
    PermissionMode {
        id: "dontAsk",
        args: &["--permission-mode", "dontAsk"],
    },
    PermissionMode {
        id: "plan",
        args: &["--permission-mode", "plan"],
    },
];

const CODEX: &[PermissionMode] = &[
    PermissionMode {
        id: "read-only",
        args: &["-s", "read-only", "-a", "never"],
    },
    PermissionMode {
        id: "workspace-write",
        args: &["-s", "workspace-write", "-a", "never"],
    },
    PermissionMode {
        id: "danger-full-access",
        args: &["-s", "danger-full-access", "-a", "never"],
    },
];

const GEMINI: &[PermissionMode] = &[
    PermissionMode {
        id: "default",
        args: &["--approval-mode", "default"],
    },
    PermissionMode {
        id: "auto_edit",
        args: &["--approval-mode", "auto_edit"],
    },
    PermissionMode {
        id: "yolo",
        args: &["--approval-mode", "yolo"],
    },
    PermissionMode {
        id: "plan",
        args: &["--approval-mode", "plan"],
    },
];

const KIMI: &[PermissionMode] = &[
    PermissionMode {
        id: "auto",
        args: &["--auto"],
    },
    PermissionMode {
        id: "yolo",
        args: &["-y"],
    },
];

const HERMES: &[PermissionMode] = &[PermissionMode {
    id: "yolo",
    args: &["--yolo"],
}];

const OPENCODE: &[PermissionMode] = &[PermissionMode {
    id: "auto",
    args: &["--auto"],
}];

const GROK_BUILD: &[PermissionMode] = &[
    PermissionMode {
        id: "default",
        args: &["--permission-mode", "default"],
    },
    PermissionMode {
        id: "acceptEdits",
        args: &["--permission-mode", "acceptEdits"],
    },
    PermissionMode {
        id: "auto",
        args: &["--permission-mode", "auto"],
    },
    PermissionMode {
        id: "dontAsk",
        args: &["--permission-mode", "dontAsk"],
    },
    PermissionMode {
        id: "bypassPermissions",
        args: &["--permission-mode", "bypassPermissions"],
    },
    PermissionMode {
        id: "plan",
        args: &["--permission-mode", "plan"],
    },
];

pub fn permissions(client_id: &str) -> ClientPermissions {
    match client_id {
        "claude_code" | "claude_desktop" => ClientPermissions {
            modes: CLAUDE,
            default_mode: Some("auto"),
        },
        "codex" => ClientPermissions {
            modes: CODEX,
            default_mode: Some("workspace-write"),
        },
        "gemini" => ClientPermissions {
            modes: GEMINI,
            default_mode: Some("auto_edit"),
        },
        "kimi_code" => ClientPermissions {
            modes: KIMI,
            default_mode: Some("auto"),
        },
        "hermes" => ClientPermissions {
            modes: HERMES,
            default_mode: Some("yolo"),
        },
        "opencode" => ClientPermissions {
            modes: OPENCODE,
            default_mode: Some("auto"),
        },
        "grok_build" => ClientPermissions {
            modes: GROK_BUILD,
            default_mode: Some("auto"),
        },
        // Pi exposes no permission switch at all.
        _ => ClientPermissions {
            modes: &[],
            default_mode: None,
        },
    }
}

/// Resolves the arguments for a requested mode, failing closed on an id the
/// client does not accept.
pub fn resolve(
    client_id: &str,
    requested: Option<&str>,
) -> Result<&'static [&'static str], String> {
    let permissions = permissions(client_id);
    let Some(id) = requested.or(permissions.default_mode) else {
        return Ok(&[]);
    };
    permissions
        .modes
        .iter()
        .find(|mode| mode.id == id)
        .map(|mode| mode.args)
        .ok_or_else(|| {
            let available = permissions
                .modes
                .iter()
                .map(|mode| mode.id)
                .collect::<Vec<_>>()
                .join(", ");
            if available.is_empty() {
                format!("{client_id} exposes no permission mode")
            } else {
                format!("unsupported {client_id} permission mode: {id}; available: {available}")
            }
        })
}
