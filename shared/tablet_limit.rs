use serde::{Deserialize, Serialize};

/// On-disk setting shared by the daemon and the settings window.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabletLimit {
    #[serde(rename = "auto")]
    Auto,
    #[serde(untagged)]
    Fixed(u32),
}

impl TabletLimit {
    pub const MAX_FIXED: u32 = 4;

    pub fn sanitize(&mut self) {
        if let Self::Fixed(limit) = self {
            *limit = (*limit).clamp(1, Self::MAX_FIXED);
        }
    }
}
