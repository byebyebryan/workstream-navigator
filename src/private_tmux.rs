//! Terminal capability settings shared by WSNav-owned private tmux servers.
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
