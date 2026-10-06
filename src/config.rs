use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::display::Display;
use crate::error::{Error, Result};

/// Represents a display configuration
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DisplayConfig {
    /// The display name or identifier
    pub name: String,
    
    /// The EDID hash for identification
    pub edid_hash: Option<String>,
    
    /// The display resolution (width x height)
    pub resolution: Option<String>,
    
    /// The display orientation (normal, left, right, inverted)
    pub orientation: Option<String>,
    
    /// The display position (x,y coordinates)
    pub position: Option<(i32, i32)>,
    
    /// The display scaling factor
    pub scaling: Option<f64>,
    
    /// Whether this is the primary display
    pub primary: Option<bool>,
    
    /// Whether the display should be enabled
    pub enabled: Option<bool>,
}

/// Represents a configuration profile for a specific monitor setup
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Profile {
    /// The profile name
    pub name: String,
    
    /// A description of the profile
    pub description: Option<String>,
    
    /// The displays in this profile
    pub displays: Vec<DisplayConfig>,
}

/// The main configuration structure
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    /// Default configuration to use when no profile matches
    pub default_profile: Option<String>,
    
    /// Available profiles
    pub profiles: Vec<Profile>,
}

impl Config {
    /// Load configuration from a file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| Error::Config(format!("Failed to parse config: {}", e)))?;
        config.validate()?;

        Ok(config)
    }

    /// Check settings that the TOML format alone can't enforce
    pub fn validate(&self) -> Result<()> {
        for profile in &self.profiles {
            let primaries: Vec<&DisplayConfig> = profile.displays.iter()
                .filter(|d| d.primary == Some(true))
                .collect();

            if primaries.len() > 1 {
                let names: Vec<&str> = primaries.iter().map(|d| d.name.as_str()).collect();
                return Err(Error::Config(format!(
                    "Profile {} marks more than one display as primary: {}",
                    profile.name, names.join(", ")
                )));
            }

            if let Some(primary) = primaries.first()
                && primary.enabled == Some(false)
            {
                return Err(Error::Config(format!(
                    "Profile {}: display {} can't be primary and disabled at the same time",
                    profile.name, primary.name
                )));
            }
        }

        Ok(())
    }
    
    /// Find the best matching profile for the given displays
    pub fn find_matching_profile(&self, available_displays: &[Display]) -> Option<&Profile> {
        // Score each profile based on how well it matches the available displays
        let mut best_match = None;
        let mut best_score = 0;

        for profile in &self.profiles {
            let matches = match_displays(&profile.displays, available_displays);

            // EDID hash matches are strong, name matches are weak
            let mut score: i32 = matches.iter()
                .map(|m| match m.kind {
                    MatchKind::EdidHash => 2,
                    MatchKind::Name => 1,
                })
                .sum();

            // Penalize for displays in profile that don't match any available display
            score -= (profile.displays.len() - matches.len()) as i32;

            // Penalize for available displays that don't match any display in the profile
            score -= (available_displays.len() - matches.len()) as i32;

            // Update best match if this profile has a higher score
            if best_match.is_none() || score > best_score {
                best_match = Some(profile);
                best_score = score;
            }
        }
        
        // If we found a match with a positive score, return it
        if best_score > 0 {
            best_match
        } else if let Some(default_name) = &self.default_profile {
            // Otherwise, try to use the default profile
            self.profiles.iter().find(|p| &p.name == default_name)
        } else {
            // If no default is specified, return the best match anyway
            best_match
        }
    }
    
    /// Save configuration to a file
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| Error::Config(format!("Failed to serialize config: {}", e)))?;
        
        fs::write(path, content)?;
        Ok(())
    }
}

/// How a display config was matched to a connected display
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    EdidHash,
    Name,
}

/// A display config matched to the connected display it applies to
#[derive(Debug, Clone, Copy)]
pub struct DisplayMatch<'a> {
    pub config: &'a DisplayConfig,
    pub display: &'a Display,
    pub kind: MatchKind,
}

/// Match display configs to connected displays.
///
/// Each display is matched to at most one config. EDID hash matches are made before name
/// matches, so a name-only config can't take a display that another config identifies by
/// EDID. Identical monitors without a serial number share an EDID hash; among those, the
/// display whose name equals the config name is preferred, so they can be told apart by
/// setting `name` to the output name.
pub fn match_displays<'a>(configs: &'a [DisplayConfig], displays: &'a [Display]) -> Vec<DisplayMatch<'a>> {
    let mut claimed = vec![false; displays.len()];
    let mut assigned: Vec<Option<(usize, MatchKind)>> = vec![None; configs.len()];

    for (config_index, config) in configs.iter().enumerate() {
        let Some(edid_hash) = &config.edid_hash else { continue };

        let candidates: Vec<usize> = (0..displays.len())
            .filter(|&i| !claimed[i] && displays[i].edid.as_ref().is_some_and(|e| e.hash() == *edid_hash))
            .collect();
        let chosen = candidates.iter().copied()
            .find(|&i| displays[i].name == config.name)
            .or(candidates.first().copied());

        if let Some(i) = chosen {
            claimed[i] = true;
            assigned[config_index] = Some((i, MatchKind::EdidHash));
        }
    }

    // Fall back to name matching for configs without an EDID match
    for (config_index, config) in configs.iter().enumerate() {
        if assigned[config_index].is_some() {
            continue;
        }

        if let Some(i) = (0..displays.len()).find(|&i| !claimed[i] && displays[i].name == config.name) {
            claimed[i] = true;
            assigned[config_index] = Some((i, MatchKind::Name));
        }
    }

    configs.iter().zip(assigned)
        .filter_map(|(config, assigned)| {
            assigned.map(|(i, kind)| DisplayMatch { config, display: &displays[i], kind })
        })
        .collect()
}

impl DisplayConfig {
    /// Capture the current state of a connected display
    pub fn from_display(display: &Display) -> DisplayConfig {
        let edid_hash = display.edid.as_ref().map(|e| e.hash());

        // A connected display that is turned off only needs to stay off
        if !display.enabled {
            return DisplayConfig {
                name: display.name.clone(),
                edid_hash,
                resolution: None,
                orientation: None,
                position: None,
                scaling: None,
                primary: None,
                enabled: Some(false),
            };
        }

        DisplayConfig {
            name: display.name.clone(),
            edid_hash,
            resolution: display.current_resolution.map(|(width, height)| format!("{}x{}", width, height)),
            orientation: Some(display.orientation.to_string()),
            position: Some(display.position),
            scaling: Some(display.scaling),
            primary: Some(display.primary),
            enabled: Some(true),
        }
    }
}

impl Profile {
    /// Capture the current state of all connected displays as a profile
    pub fn from_displays(name: &str, displays: &[Display]) -> Profile {
        let names: Vec<&str> = displays.iter().map(|d| d.name.as_str()).collect();

        Profile {
            name: name.to_string(),
            description: Some(format!("Detected layout of {}", names.join(", "))),
            displays: displays.iter().map(DisplayConfig::from_display).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Orientation;
    use crate::display::edid::Edid;

    /// An EDID without a serial number, like many identical monitors report
    fn edid(product_id: u16) -> Edid {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x10, 0xAC]);
        data[10..12].copy_from_slice(&product_id.to_le_bytes());
        Edid::parse(&data).unwrap()
    }

    fn display(id: &str, name: &str, edid: Option<Edid>) -> Display {
        Display {
            id: id.to_string(),
            name: name.to_string(),
            edid,
            current_resolution: Some((1920, 1080)),
            available_resolutions: vec![(1920, 1080)],
            position: (0, 0),
            orientation: Orientation::Normal,
            scaling: 1.0,
            primary: false,
            enabled: true,
        }
    }

    fn config(name: &str, edid_hash: Option<String>) -> DisplayConfig {
        DisplayConfig {
            name: name.to_string(),
            edid_hash,
            resolution: None,
            orientation: None,
            position: None,
            scaling: None,
            primary: None,
            enabled: None,
        }
    }

    fn matched_ids<'a>(matches: &[DisplayMatch<'a>]) -> Vec<(&'a str, &'a str)> {
        matches.iter().map(|m| (m.config.name.as_str(), m.display.id.as_str())).collect()
    }

    #[test]
    fn identical_monitors_match_separate_displays() {
        let hash = edid(1).hash();
        let displays = [display("1", "DP-1", Some(edid(1))), display("2", "DP-2", Some(edid(1)))];
        let configs = [config("left", Some(hash.clone())), config("right", Some(hash))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("left", "1"), ("right", "2")]);
    }

    #[test]
    fn identical_monitors_are_told_apart_by_name() {
        let hash = edid(1).hash();
        let displays = [display("1", "DP-1", Some(edid(1))), display("2", "DP-2", Some(edid(1)))];
        let configs = [config("DP-2", Some(hash.clone())), config("DP-1", Some(hash))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("DP-2", "2"), ("DP-1", "1")]);
    }

    #[test]
    fn edid_match_wins_over_earlier_name_match() {
        let displays = [display("1", "DP-1", Some(edid(1))), display("2", "DP-2", Some(edid(2)))];
        // The name-only config comes first but must not take the display the second config's EDID identifies
        let configs = [config("DP-1", None), config("external", Some(edid(1).hash()))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("external", "1")]);
    }

    #[test]
    fn profile_with_both_identical_monitors_wins() {
        let hash = edid(1).hash();
        let displays = [display("1", "DP-1", Some(edid(1))), display("2", "DP-2", Some(edid(1)))];
        let profile = |name: &str, count| Profile {
            name: name.to_string(),
            description: None,
            displays: (0..count).map(|_| config("monitor", Some(hash.clone()))).collect(),
        };
        let config = Config { default_profile: None, profiles: vec![profile("single", 1), profile("dual", 2)] };

        assert_eq!(config.find_matching_profile(&displays).unwrap().name, "dual");
    }

    fn primary_config(name: &str, enabled: Option<bool>) -> DisplayConfig {
        DisplayConfig { primary: Some(true), enabled, ..config(name, None) }
    }

    fn single_profile(displays: Vec<DisplayConfig>) -> Config {
        Config {
            default_profile: None,
            profiles: vec![Profile { name: "test".to_string(), description: None, displays }],
        }
    }

    #[test]
    fn one_primary_display_is_valid() {
        assert!(single_profile(vec![primary_config("DP-1", Some(true)), config("DP-2", None)]).validate().is_ok());
    }

    #[test]
    fn two_primary_displays_are_rejected() {
        let err = single_profile(vec![primary_config("DP-1", None), primary_config("DP-2", None)]).validate().unwrap_err();
        assert!(err.to_string().contains("more than one display as primary"), "{}", err);
    }

    #[test]
    fn disabled_primary_display_is_rejected() {
        let err = single_profile(vec![primary_config("DP-1", Some(false))]).validate().unwrap_err();
        assert!(err.to_string().contains("can't be primary and disabled"), "{}", err);
    }
}
