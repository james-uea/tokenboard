//! Client-definition registry for AI coding agents the scanner supports.
//!
//! Each [`ClientDef`] describes where to find an agent's session files on disk,
//! what file pattern to match, and which parser to use. This replaces the old
//! hardcoded `KNOWN_AGENTS` constant with a data-driven registry.

// ============================================================================
// Path root — where to anchor the search
// ============================================================================

/// Where to find a client's session data on disk.
pub enum PathRoot {
    /// User home directory.
    Home,
    /// `XDG_DATA_HOME` (Linux) / `~/Library/Application Support` (macOS) / `%APPDATA%` (Windows).
    XdgData,
    /// `XDG_CONFIG_HOME` / `~/.config` / `%APPDATA%`.
    Config,
    /// Environment variable with a hardcoded fallback relative to home.
    EnvVar {
        var: &'static str,
        fallback: &'static str,
    },
}

// ============================================================================
// Parser kind — what format the session files use
// ============================================================================

/// What format a client's session files use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParserKind {
    /// JSONL stream — one JSON object per line.
    Jsonl,
    /// Single JSON object or array per file.
    Json,
    /// CSV file (Cursor usage.csv).
    Csv,
    /// SQLite database.
    Sqlite,
}

// ============================================================================
// Client definition
// ============================================================================

/// Descriptor for one AI coding agent the scanner supports.
pub struct ClientDef {
    /// CLI id (e.g. `"claude"`, `"cursor"`). Used for `--client` filtering.
    pub id: &'static str,
    /// Human-readable label for output.
    pub label: &'static str,
    /// Path root for discovery.
    pub root: PathRoot,
    /// Relative path from the root (may include glob-style wildcards).
    pub relative_path: &'static str,
    /// File pattern for WalkDir filtering (e.g. `*.jsonl`, `*.json|*.jsonl`).
    pub pattern: &'static str,
    /// Parser to use for discovered files.
    pub parser: ParserKind,
    /// Whether to include this client by default (vs. opt-in only).
    pub submit_default: bool,
}

// ============================================================================
// Path resolution
// ============================================================================

impl PathRoot {
    /// Resolve this root to an absolute path given the user's home directory.
    pub fn resolve(&self, home_dir: &str) -> String {
        match self {
            PathRoot::Home => home_dir.to_string(),
            PathRoot::XdgData => std::env::var("XDG_DATA_HOME")
                .unwrap_or_else(|_| format!("{}/.local/share", home_dir)),
            PathRoot::Config => {
                std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", home_dir))
            }
            PathRoot::EnvVar { var, fallback } => {
                std::env::var(*var).unwrap_or_else(|_| format!("{}/{}", home_dir, fallback))
            }
        }
    }
}

impl ClientDef {
    /// Resolve this client's scan root to an absolute path.
    pub fn resolve_path(&self, home_dir: &str) -> String {
        format!("{}/{}", self.root.resolve(home_dir), self.relative_path)
    }
}

// ============================================================================
// The registry — ALL supported clients
// ============================================================================

pub const CLIENTS: &[ClientDef] = &[
    // ── Already fully implemented ──────────────────────────────────────
    ClientDef {
        id: "claude",
        label: "Claude Code",
        root: PathRoot::Home,
        relative_path: ".claude/projects",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "codex",
        label: "OpenAI Codex",
        root: PathRoot::EnvVar {
            var: "CODEX_HOME",
            fallback: ".codex",
        },
        relative_path: "sessions",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    // ── JSONL clients ──────────────────────────────────────────────────
    ClientDef {
        id: "gemini",
        label: "Gemini CLI",
        root: PathRoot::Home,
        relative_path: ".gemini/tmp",
        pattern: "*.json|*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "openclaw",
        label: "OpenClaw",
        root: PathRoot::Home,
        relative_path: ".openclaw/agents",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "pi",
        label: "Pi AI",
        root: PathRoot::Home,
        relative_path: ".pi/agent/sessions",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "kimi",
        label: "Kimi",
        root: PathRoot::Home,
        relative_path: ".kimi/sessions",
        pattern: "wire.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "qwen",
        label: "Qwen Code",
        root: PathRoot::Home,
        relative_path: ".qwen/projects",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "copilot",
        label: "GitHub Copilot",
        root: PathRoot::Home,
        relative_path: ".copilot/otel",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: true,
    },
    ClientDef {
        id: "antigravity",
        label: "Antigravity",
        root: PathRoot::Config,
        relative_path: "tokscale/antigravity-cache/sessions",
        pattern: "*.jsonl",
        parser: ParserKind::Jsonl,
        submit_default: false,
    },
    // ── JSON clients ───────────────────────────────────────────────────
    ClientDef {
        id: "opencode",
        label: "OpenCode",
        root: PathRoot::XdgData,
        relative_path: "opencode/opencode.db",
        pattern: "opencode.db",
        parser: ParserKind::Sqlite,
        submit_default: true,
    },
    ClientDef {
        id: "amp",
        label: "Amp",
        root: PathRoot::XdgData,
        relative_path: "amp/threads",
        pattern: "T-*.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    ClientDef {
        id: "droid",
        label: "Factory Droid",
        root: PathRoot::Home,
        relative_path: ".factory/sessions",
        pattern: "*.settings.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    ClientDef {
        id: "roocode",
        label: "RooCode",
        root: PathRoot::Home,
        relative_path: ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks",
        pattern: "ui_messages.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    ClientDef {
        id: "kilocode",
        label: "KiloCode",
        root: PathRoot::Home,
        relative_path: ".config/Code/User/globalStorage/kilocode.kilo-code/tasks",
        pattern: "ui_messages.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    ClientDef {
        id: "mux",
        label: "Mux",
        root: PathRoot::Home,
        relative_path: ".mux/sessions",
        pattern: "session-usage.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    ClientDef {
        id: "codebuff",
        label: "Codebuff",
        root: PathRoot::EnvVar {
            var: "CODEBUFF_DATA_DIR",
            fallback: ".config/manicode",
        },
        relative_path: "projects",
        pattern: "chat-messages.json",
        parser: ParserKind::Json,
        submit_default: true,
    },
    // ── CSV client ─────────────────────────────────────────────────────
    ClientDef {
        id: "cursor",
        label: "Cursor IDE",
        root: PathRoot::Home,
        relative_path: ".config/tokscale/cursor-cache",
        pattern: "usage*.csv",
        parser: ParserKind::Csv,
        submit_default: true,
    },
    // ── SQLite clients ─────────────────────────────────────────────────
    ClientDef {
        id: "hermes",
        label: "Hermes Agent",
        root: PathRoot::EnvVar {
            var: "HERMES_HOME",
            fallback: ".hermes",
        },
        relative_path: "state.db",
        pattern: "state.db",
        parser: ParserKind::Sqlite,
        submit_default: true,
    },
    ClientDef {
        id: "kilo",
        label: "Kilo Code",
        root: PathRoot::XdgData,
        relative_path: "kilo/kilo.db",
        pattern: "kilo.db",
        parser: ParserKind::Sqlite,
        submit_default: true,
    },
    ClientDef {
        id: "goose",
        label: "Goose",
        root: PathRoot::XdgData,
        relative_path: "goose/sessions/sessions.db",
        pattern: "sessions.db",
        parser: ParserKind::Sqlite,
        submit_default: true,
    },
    ClientDef {
        id: "crush",
        label: "Crush",
        root: PathRoot::XdgData,
        relative_path: "crush",
        pattern: "crush.db",
        parser: ParserKind::Sqlite,
        submit_default: false,
    },
];

// ============================================================================
// Registry queries
// ============================================================================

/// Find a client by id (case-insensitive).
pub fn find_client(id: &str) -> Option<&'static ClientDef> {
    CLIENTS.iter().find(|c| c.id.eq_ignore_ascii_case(id))
}

/// All client ids (for help text, filter validation).
pub fn all_ids() -> Vec<&'static str> {
    CLIENTS.iter().map(|c| c.id).collect()
}
