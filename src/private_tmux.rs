//! Shared terminal capability and fixed copy-mode interaction settings for
//! WSNav-owned private tmux servers.
//!
//! Runtime and presentation topology remain in their owning modules. This
//! fragment contains only the settings that must stay identical across both
//! nested terminal layers.

pub(crate) const TERMINAL_CAPABILITY_CONFIG: &str = concat!(
    "set -g default-terminal tmux-256color\n",
    "set-environment -g COLORTERM truecolor\n",
    "set -g extended-keys always\n",
    // tmux 3.4 already emits extended keys as CSI-u but predates the
    // selectable format option; tmux 3.5+ applies the explicit selection.
    "set -q -g extended-keys-format csi-u\n",
    "set -as terminal-features ',xterm-ghostty:RGB:extkeys'\n",
    "set -as terminal-features ',tmux-256color:RGB:extkeys'\n",
);

/// One owned copy-mode wheel binding shared by both private tmux layers.
///
/// The `select-pane` command is deliberately part of each binding. tmux's
/// stock wheel bindings select the pane named by the mouse event before
/// entering copy mode; retaining that command keeps the same pane-selection
/// behavior while changing only the scroll repeat count.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CopyModeScrollBinding {
    table: &'static str,
    key: &'static str,
    direction: &'static str,
}

impl CopyModeScrollBinding {
    /// Returns the exact argv sequence accepted by `tmux bind-key`.
    ///
    /// The escaped semicolon is an argv element rather than a shell command
    /// separator. Both private tmux callers therefore remain shell-free.
    #[must_use]
    pub(crate) const fn arguments(self) -> [&'static str; 11] {
        [
            "bind-key",
            "-T",
            self.table,
            self.key,
            "select-pane",
            "\\;",
            "send-keys",
            "-X",
            "-N",
            "1",
            self.direction,
        ]
    }

    /// Returns the exact startup-config line for this binding.
    #[must_use]
    pub(crate) fn config_line(self) -> String {
        let mut line = self.arguments().join(" ");
        line.push('\n');
        line
    }
}

/// The complete owned interaction profile for private tmux copy mode.
///
/// Keep this table in one place: startup configuration and attach-time
/// reconciliation must install the same four bindings in the same order.
pub(crate) const COPY_MODE_SCROLL_BINDINGS: [CopyModeScrollBinding; 4] = [
    CopyModeScrollBinding {
        table: "copy-mode",
        key: "WheelUpPane",
        direction: "scroll-up",
    },
    CopyModeScrollBinding {
        table: "copy-mode",
        key: "WheelDownPane",
        direction: "scroll-down",
    },
    CopyModeScrollBinding {
        table: "copy-mode-vi",
        key: "WheelUpPane",
        direction: "scroll-up",
    },
    CopyModeScrollBinding {
        table: "copy-mode-vi",
        key: "WheelDownPane",
        direction: "scroll-down",
    },
];

/// Renders the shared interaction profile for a private tmux config file.
#[must_use]
pub(crate) fn copy_mode_scroll_config() -> String {
    COPY_MODE_SCROLL_BINDINGS
        .into_iter()
        .map(CopyModeScrollBinding::config_line)
        .collect()
}
