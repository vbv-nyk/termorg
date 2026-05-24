/// On-disk format for preserving terminal organisation across server restarts.
///
/// Saved to ~/.local/state/termorg/layout.toml every time the structure
/// changes.  On the next server start the groups and terminals are recreated
/// (with fresh shell processes) in the same arrangement.
use crate::proto::GroupColor;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Default)]
pub struct Layout {
    #[serde(default)]
    pub group: Vec<GroupEntry>,
    #[serde(default)]
    pub terminal: Vec<TerminalEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GroupEntry {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct TerminalEntry {
    #[serde(default)]
    pub title: String,
    /// Name of the group this terminal belongs to (kept for human readability).
    /// None = ungrouped.
    pub group: Option<String>,
    /// Zero-based index into the `[[group]]` array for this terminal.
    /// This is the authoritative field for group assignment on reload — the name
    /// field is only informational.  None = ungrouped.
    pub group_index: Option<usize>,
}

pub fn load(path: &Path) -> Layout {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, layout: &Layout) -> Result<()> {
    let s = toml::to_string_pretty(layout)?;
    std::fs::write(path, s)?;
    Ok(())
}

pub fn color_from_str(s: &str) -> GroupColor {
    match s {
        "Sage"  => GroupColor::Sage,
        "Terra" => GroupColor::Terra,
        "Slate" => GroupColor::Slate,
        _       => GroupColor::Ochre,
    }
}

pub fn color_to_str(c: GroupColor) -> &'static str {
    match c {
        GroupColor::Ochre => "Ochre",
        GroupColor::Sage  => "Sage",
        GroupColor::Terra => "Terra",
        GroupColor::Slate => "Slate",
    }
}
