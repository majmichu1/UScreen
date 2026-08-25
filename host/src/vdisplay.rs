//! Discovery of the EVDI virtual display through sysfs.

/// DRM card indices backed by the EVDI driver, read from sysfs.
///
/// Matching on the connector name alone ("contains DVI") is not safe: a real
/// DVI monitor on a dock produces exactly the same name pattern, and acting on
/// it would enable/move the user's physical screen instead of ours.
pub fn evdi_cards() -> Vec<u32> {
    let mut cards = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/devices/platform") else {
        return cards;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("evdi.") {
            continue;
        }
        let Ok(drm) = std::fs::read_dir(entry.path().join("drm")) else {
            continue;
        };
        for card in drm.flatten() {
            if let Some(idx) = card
                .file_name()
                .to_str()
                .and_then(|n| n.strip_prefix("card"))
                .and_then(|n| n.parse::<u32>().ok())
            {
                cards.push(idx);
            }
        }
    }
    cards.sort_unstable();
    cards.dedup();
    cards
}

/// A DRM connector belonging to an EVDI card, e.g. `DVI-I-1`.
/// The name is exactly what KWin/kscreen-doctor calls the output.
pub struct EvdiConnector {
    pub name: String,
    pub card: u32,
    pub connected: bool,
}

/// Enumerate the connectors of every EVDI card. Used to address the virtual
/// display unambiguously instead of guessing from the output name.
pub fn evdi_connectors() -> Vec<EvdiConnector> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return out;
    };
    let cards = evdi_cards();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else { continue };
        // Entries look like "card2-DVI-I-1"; the part after the dash is the
        // connector name the compositor uses.
        let Some((card_part, connector)) = name.split_once('-') else {
            continue;
        };
        let Some(idx) = card_part
            .strip_prefix("card")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if !cards.contains(&idx) {
            continue;
        }
        let connected = std::fs::read_to_string(entry.path().join("status"))
            .map(|s| s.trim() == "connected")
            .unwrap_or(false);
        out.push(EvdiConnector {
            name: connector.to_string(),
            card: idx,
            connected,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
