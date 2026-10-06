use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use crate::display::{Display, Orientation, SCALING_FILTER};
use crate::error::{Error, Result};

/// Represents a display configuration
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DisplayConfig {
    /// What the display is, as shown by `disp list`, e.g. "Contoso C27". Never the port
    /// it's plugged into. Used to match displays when no EDID hash is given.
    pub name: String,

    /// The EDID hash that identifies the display, regardless of the port it's plugged into
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
/// Displays are identified by what they are, never by the port they're plugged into, so a
/// config applies to its monitor on whichever port it is connected to. A config with an
/// EDID hash matches only the monitor with that hash, which includes its serial number;
/// a config without one matches by the display name from the EDID. Each display is
/// matched to at most one config, and EDID hash matches are made first. When several
/// displays share an EDID hash, the one whose name equals the config name is preferred.
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

    // Configs without an EDID hash are matched by name instead. A config with a hash only
    // ever matches that exact monitor: another unit of the same model, e.g. at a different
    // dock, must not pick up its settings.
    for (config_index, config) in configs.iter().enumerate() {
        if config.edid_hash.is_some() {
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

    /// Describe how the connected displays differ from this profile's layout.
    ///
    /// Returns nothing when the profile is fully applied. Only settings the profile
    /// specifies are compared. Displays the profile doesn't cover should be off, because
    /// applying the profile turns them off.
    pub fn layout_differences(&self, displays: &[Display]) -> Vec<String> {
        let matches = match_displays(&self.displays, displays);
        let mut differences = Vec::new();

        for display in displays {
            let Some(config) = matches.iter().find(|m| std::ptr::eq(m.display, display)).map(|m| m.config) else {
                if display.enabled {
                    differences.push(format!("{} is on but not part of the profile", display.name));
                }
                continue;
            };

            if config.enabled == Some(false) {
                if display.enabled {
                    differences.push(format!("{} is on but should be off", display.name));
                }
                continue;
            }

            if !display.enabled {
                differences.push(format!("{} is off but should be on", display.name));
                continue;
            }

            let current_resolution = display.current_resolution.map(|(width, height)| format!("{}x{}", width, height));
            if let Some(resolution) = &config.resolution
                && current_resolution.as_deref() != Some(resolution.trim())
            {
                differences.push(format!(
                    "{} runs at {} instead of {}",
                    display.name, current_resolution.as_deref().unwrap_or("no resolution"), resolution
                ));
            }

            if let Some(position) = config.position
                && position != display.position
            {
                differences.push(format!("{} is at {:?} instead of {:?}", display.name, display.position, position));
            }

            if let Some(orientation) = &config.orientation
                && Orientation::from(orientation.as_str()) != display.orientation
            {
                differences.push(format!("{} is rotated {} instead of {}", display.name, display.orientation, orientation));
            }

            // Compare the fixed-point values the server stores rather than the decimals,
            // which can differ while describing the same transform
            let fixed_point = |scaling: f64| (scaling * 65536.0).round() as i64;
            if let Some(scaling) = config.scaling
                && fixed_point(scaling) != fixed_point(display.scaling)
            {
                differences.push(format!("{} is scaled {} instead of {}", display.name, display.scaling, scaling));
            }

            // A scaled display drawn with any other filter has distorted text, e.g. one scaled
            // with "nearest" by an older version of disp
            if fixed_point(display.scaling) != 65536 && display.scaling_filter != SCALING_FILTER {
                let filter = if display.scaling_filter.is_empty() { "no" } else { display.scaling_filter.as_str() };
                differences.push(format!(
                    "{} is scaled with the {} filter instead of {}, which distorts text",
                    display.name, filter, SCALING_FILTER
                ));
            }

            if config.primary == Some(true) && !display.primary {
                differences.push(format!("{} is not the primary display", display.name));
            }
        }

        differences
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::edid::Edid;

    /// An EDID without a serial number, like many identical monitors report
    fn edid(product_id: u16) -> Edid {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x04, 0x6D]);
        data[10..12].copy_from_slice(&product_id.to_le_bytes());
        Edid::parse(&data).unwrap()
    }

    /// A connected display: `id` and `output` describe where it is plugged in, `name` what it is
    fn display(id: &str, output: &str, name: &str, edid: Option<Edid>) -> Display {
        Display {
            id: id.to_string(),
            output: output.to_string(),
            name: name.to_string(),
            edid,
            current_resolution: Some((1920, 1080)),
            available_resolutions: vec![(1920, 1080)],
            position: (0, 0),
            orientation: Orientation::Normal,
            scaling: 1.0,
            scaling_filter: String::new(),
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
        let displays = [display("1", "DP-1", "ACM 0001 #1", Some(edid(1))), display("2", "DP-2", "ACM 0001 #2", Some(edid(1)))];
        let configs = [config("left", Some(hash.clone())), config("right", Some(hash))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("left", "1"), ("right", "2")]);
    }

    #[test]
    fn identical_monitors_are_told_apart_by_name() {
        let hash = edid(1).hash();
        let displays = [display("1", "DP-1", "ACM 0001 #1", Some(edid(1))), display("2", "DP-2", "ACM 0001 #2", Some(edid(1)))];
        let configs = [config("ACM 0001 #2", Some(hash.clone())), config("ACM 0001 #1", Some(hash))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("ACM 0001 #2", "2"), ("ACM 0001 #1", "1")]);
    }

    #[test]
    fn edid_match_wins_over_earlier_name_match() {
        let displays = [display("1", "DP-1", "ACM 0001", Some(edid(1))), display("2", "DP-2", "ACM 0002", Some(edid(2)))];
        // The name-only config comes first but must not take the display the second config's EDID identifies
        let configs = [config("ACM 0001", None), config("external", Some(edid(1).hash()))];

        assert_eq!(matched_ids(&match_displays(&configs, &displays)), [("external", "1")]);
    }

    #[test]
    fn config_follows_its_monitor_to_another_port() {
        let configs = [config("left", Some(edid(1).hash())), config("right", Some(edid(2).hash()))];
        let before = [display("1", "DP-1", "ACM 0001", Some(edid(1))), display("2", "DP-2", "ACM 0002", Some(edid(2)))];
        // The same two monitors with their cables swapped
        let after = [display("1", "DP-1", "ACM 0002", Some(edid(2))), display("2", "DP-2", "ACM 0001", Some(edid(1)))];

        assert_eq!(matched_ids(&match_displays(&configs, &before)), [("left", "1"), ("right", "2")]);
        assert_eq!(matched_ids(&match_displays(&configs, &after)), [("left", "2"), ("right", "1")]);
    }

    #[test]
    fn names_match_what_a_display_is_not_its_port() {
        let displays = [display("1", "DP-1", "Contoso C27", Some(edid(1)))];

        assert_eq!(matched_ids(&match_displays(&[config("Contoso C27", None)], &displays)), [("Contoso C27", "1")]);
        assert!(match_displays(&[config("DP-1", None)], &displays).is_empty());
    }

    #[test]
    fn captured_display_config_has_no_port() {
        let captured = DisplayConfig::from_display(&display("1", "DP-1", "Contoso C27", Some(edid(1))));

        assert_eq!(captured.name, "Contoso C27");
        assert_eq!(captured.edid_hash, Some(edid(1).hash()));
    }

    /// The same model as `edid(product_id)` but a specific unit with a serial number
    fn unit(product_id: u16, serial: u32) -> Edid {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x04, 0x6D]);
        data[10..12].copy_from_slice(&product_id.to_le_bytes());
        data[12..16].copy_from_slice(&serial.to_le_bytes());
        Edid::parse(&data).unwrap()
    }

    /// A config for display `edid` with a position, as `create-config` writes it
    fn placed(name: &str, edid: &Edid, position: (i32, i32)) -> DisplayConfig {
        DisplayConfig {
            resolution: Some("1920x1080".to_string()),
            position: Some(position),
            enabled: Some(true),
            ..config(name, Some(edid.hash()))
        }
    }

    fn profile(name: &str, displays: Vec<DisplayConfig>) -> Profile {
        Profile { name: name.to_string(), description: None, displays }
    }

    #[test]
    fn hashed_config_ignores_another_unit_of_the_same_model() {
        let home_monitor = unit(1, 1111);
        let office_monitor = unit(1, 2222);
        let displays = [display("1", "DP-1", "ACM 0001", Some(office_monitor))];

        assert!(match_displays(&[placed("ACM 0001", &home_monitor, (0, 0))], &displays).is_empty());
    }

    /// Profiles for a laptop used on its own, at a home dock and at an office dock
    fn laptop_config() -> Config {
        let laptop = unit(10, 1);
        Config {
            default_profile: Some("mobile".to_string()),
            profiles: vec![
                profile("mobile", vec![placed("Laptop", &laptop, (0, 0))]),
                profile("home", vec![
                    DisplayConfig { enabled: Some(false), ..config("Laptop", Some(laptop.hash())) },
                    placed("Home left", &unit(20, 1), (0, 0)),
                    placed("Home right", &unit(20, 2), (1920, 0)),
                ]),
                profile("office", vec![
                    placed("Laptop", &laptop, (0, 0)),
                    placed("Office", &unit(30, 1), (1920, 0)),
                ]),
            ],
        }
    }

    #[test]
    fn profile_follows_the_laptop_between_docks() {
        let config = laptop_config();
        let laptop = || display("1", "eDP-1", "Laptop", Some(unit(10, 1)));
        let chosen = |displays: &[Display]| config.find_matching_profile(displays).unwrap().name.clone();

        assert_eq!(chosen(&[laptop()]), "mobile");
        assert_eq!(chosen(&[laptop(), display("2", "DP-3-1", "Home left", Some(unit(20, 1))), display("3", "DP-3-2", "Home right", Some(unit(20, 2)))]), "home");
        // The dock came back from sleep with different port numbers, and the monitors swapped ports
        assert_eq!(chosen(&[laptop(), display("2", "DP-5-2", "Home right", Some(unit(20, 2))), display("3", "DP-5-1", "Home left", Some(unit(20, 1)))]), "home");
        assert_eq!(chosen(&[laptop(), display("2", "HDMI-1", "Office", Some(unit(30, 1)))]), "office");
    }

    #[test]
    fn unknown_dock_falls_back_to_the_default_profile() {
        let config = laptop_config();
        let displays = [display("1", "eDP-1", "Laptop", Some(unit(10, 1))), display("2", "DP-1", "Elsewhere", Some(unit(40, 1)))];

        assert_eq!(config.find_matching_profile(&displays).unwrap().name, "mobile");
    }

    #[test]
    fn applied_layout_has_no_differences() {
        let config = laptop_config();
        let office = &config.profiles[2];
        let mut screen = display("2", "HDMI-1", "Office", Some(unit(30, 1)));
        screen.position = (1920, 0);
        let displays = [display("1", "eDP-1", "Laptop", Some(unit(10, 1))), screen];

        assert!(office.layout_differences(&displays).is_empty(), "{:?}", office.layout_differences(&displays));
    }

    #[test]
    fn layout_reset_after_sleep_is_detected() {
        let config = laptop_config();
        let home = &config.profiles[1];
        // Back from sleep: the laptop panel came back on and the right monitor is off
        let mut left = display("2", "DP-3-1", "Home left", Some(unit(20, 1)));
        left.position = (0, 0);
        let mut right = display("3", "DP-3-2", "Home right", Some(unit(20, 2)));
        right.enabled = false;
        let displays = [display("1", "eDP-1", "Laptop", Some(unit(10, 1))), left, right];

        assert_eq!(home.layout_differences(&displays), [
            "Laptop is on but should be off",
            "Home right is off but should be on",
        ]);
    }

    #[test]
    fn displays_outside_the_profile_must_be_off() {
        let config = laptop_config();
        let mobile = &config.profiles[0];
        let displays = [display("1", "eDP-1", "Laptop", Some(unit(10, 1))), display("2", "DP-1", "Elsewhere", Some(unit(40, 1)))];

        assert_eq!(mobile.layout_differences(&displays), ["Elsewhere is on but not part of the profile"]);
    }

    #[test]
    fn scaling_is_compared_as_the_server_stores_it() {
        let laptop = unit(10, 1);
        let mut scaled = display("1", "eDP-1", "Laptop", Some(laptop.clone()));
        scaled.scaling = 1.32999;
        scaled.scaling_filter = SCALING_FILTER.to_string();
        // Written differently but the same fixed-point transform
        let profile = profile("p", vec![DisplayConfig { scaling: Some(1.329987), ..placed("Laptop", &laptop, (0, 0)) }]);

        assert!(profile.layout_differences(&[scaled]).is_empty());
    }

    #[test]
    fn scaling_with_the_nearest_filter_is_a_difference() {
        let monitor = unit(20, 1);
        let mut scaled = display("1", "DP-4", "Home left", Some(monitor.clone()));
        scaled.scaling = 2.0;
        scaled.scaling_filter = "nearest".to_string();
        let profile = profile("p", vec![DisplayConfig { scaling: Some(2.0), ..placed("Home left", &monitor, (0, 0)) }]);

        assert_eq!(profile.layout_differences(&[scaled.clone()]), [
            "Home left is scaled with the nearest filter instead of bilinear, which distorts text",
        ]);

        scaled.scaling_filter = SCALING_FILTER.to_string();
        assert!(profile.layout_differences(&[scaled]).is_empty());
    }

    #[test]
    fn profile_with_both_identical_monitors_wins() {
        let hash = edid(1).hash();
        let displays = [display("1", "DP-1", "ACM 0001 #1", Some(edid(1))), display("2", "DP-2", "ACM 0001 #2", Some(edid(1)))];
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
        assert!(single_profile(vec![primary_config("Contoso C27", Some(true)), config("Laptop Panel", None)]).validate().is_ok());
    }

    #[test]
    fn two_primary_displays_are_rejected() {
        let err = single_profile(vec![primary_config("Contoso C27", None), primary_config("Laptop Panel", None)]).validate().unwrap_err();
        assert!(err.to_string().contains("more than one display as primary"), "{}", err);
    }

    #[test]
    fn disabled_primary_display_is_rejected() {
        let err = single_profile(vec![primary_config("Contoso C27", Some(false))]).validate().unwrap_err();
        assert!(err.to_string().contains("can't be primary and disabled"), "{}", err);
    }
}
