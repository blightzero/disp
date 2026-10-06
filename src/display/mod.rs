pub mod edid;

#[cfg(not(target_os = "linux"))]
compile_error!("disp only supports Linux (X11 with RandR)");

pub mod linux;

use std::fmt;

use crate::config::DisplayConfig;
use crate::error::Result;
use edid::Edid;

/// Represents a physical display
#[derive(Debug, Clone)]
pub struct Display {
    /// Display identifier
    pub id: String,

    /// Name of the connector the display is plugged into, e.g. "DP-1". Only used to
    /// talk to the display and in messages, never to identify it.
    pub output: String,

    /// What the display is, from its EDID, e.g. "Contoso C27". Unique among the connected
    /// displays; see `assign_display_names`.
    pub name: String,
    
    /// EDID data
    pub edid: Option<Edid>,
    
    /// Current resolution
    pub current_resolution: Option<(u32, u32)>,
    
    /// Available resolutions
    pub available_resolutions: Vec<(u32, u32)>,
    
    /// Current position
    pub position: (i32, i32),
    
    /// Current orientation
    pub orientation: Orientation,
    
    /// Current scaling factor
    pub scaling: f64,

    /// Filter the server uses to scale the display, empty if it was never set
    pub scaling_filter: String,
    
    /// Whether this is the primary display
    pub primary: bool,
    
    /// Whether the display is enabled
    pub enabled: bool,
}

/// Display orientation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    Left,
    Right,
    Inverted,
}

impl fmt::Display for Orientation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Orientation::Normal => write!(f, "normal"),
            Orientation::Left => write!(f, "left"),
            Orientation::Right => write!(f, "right"),
            Orientation::Inverted => write!(f, "inverted"),
        }
    }
}

impl From<&str> for Orientation {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "left" => Orientation::Left,
            "right" => Orientation::Right,
            "inverted" => Orientation::Inverted,
            _ => Orientation::Normal,
        }
    }
}

/// Name given to displays that provide no EDID
pub const UNKNOWN_DISPLAY_NAME: &str = "Unknown display";

/// Filter for scaled displays. Like `xrandr --scale`, this blends neighbouring desktop
/// pixels; "nearest" would drop pixels when scaling down and break up thin font strokes.
pub const SCALING_FILTER: &str = "bilinear";

/// Name every display after what it is rather than the port it is plugged into.
///
/// The base name is the model name from the EDID. Displays that share it get the most
/// meaningful distinguishing EDID detail added: the serial number, otherwise the start of
/// the EDID hash. Only displays whose EDIDs are identical (or missing) are numbered.
pub fn assign_display_names(displays: &mut [Display]) {
    let base_names: Vec<String> = displays.iter()
        .map(|d| d.edid.as_ref().map_or_else(|| UNKNOWN_DISPLAY_NAME.to_string(), |e| e.model_name()))
        .collect();
    let serials: Vec<Option<String>> = displays.iter().map(|d| d.edid.as_ref().and_then(|e| e.serial())).collect();
    let hashes: Vec<Option<String>> = displays.iter().map(|d| d.edid.as_ref().map(|e| e.hash())).collect();

    let names: Vec<String> = (0..displays.len())
        .map(|i| {
            let same_model: Vec<usize> = (0..displays.len()).filter(|&j| base_names[j] == base_names[i]).collect();
            if same_model.len() == 1 {
                return base_names[i].clone();
            }

            let unique_within = |values: &[Option<String>]| {
                values[i].is_some() && same_model.iter().filter(|&&j| values[j] == values[i]).count() == 1
            };

            if unique_within(&serials) {
                format!("{} (S/N {})", base_names[i], serials[i].as_deref().unwrap_or_default())
            } else if unique_within(&hashes) {
                format!("{} ({})", base_names[i], &hashes[i].as_deref().unwrap_or_default()[..8])
            } else {
                // Indistinguishable displays are numbered in the order the server lists them
                let number = same_model.iter().filter(|&&j| j <= i && hashes[j] == hashes[i]).count();
                format!("{} #{}", base_names[i], number)
            }
        })
        .collect();

    for (display, name) in displays.iter_mut().zip(names) {
        display.name = name;
    }
}

/// Display manager trait
pub trait DisplayManager {
    /// Get all connected displays
    fn get_displays(&self) -> Result<Vec<Display>>;
    
    /// Apply a display configuration
    fn apply_config(&self, displays: &[Display], config: &[DisplayConfig]) -> Result<()>;
}

/// Create the display manager
pub fn create_display_manager() -> Result<Box<dyn DisplayManager>> {
    Ok(Box::new(linux::LinuxDisplayManager::new()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EDID for model `name` with an optional text serial
    fn edid(name: &str, serial: Option<&str>) -> Edid {
        let mut data = vec![0u8; 128];
        data[0..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        data[8..10].copy_from_slice(&[0x10, 0xAC]);
        let mut descriptors = vec![(0xFC, name)];
        descriptors.extend(serial.map(|s| (0xFF, s)));
        for (slot, (tag, text)) in descriptors.into_iter().enumerate() {
            let offset = 54 + (slot + 1) * 18;
            data[offset + 3] = tag;
            let mut field = [b' '; 13];
            field[..text.len()].copy_from_slice(text.as_bytes());
            field[text.len()] = 0x0A;
            data[offset + 5..offset + 18].copy_from_slice(&field);
        }
        Edid::parse(&data).unwrap()
    }

    fn display(output: &str, edid: Option<Edid>) -> Display {
        Display {
            id: output.to_string(),
            output: output.to_string(),
            name: String::new(),
            edid,
            current_resolution: None,
            available_resolutions: Vec::new(),
            position: (0, 0),
            orientation: Orientation::Normal,
            scaling: 1.0,
            scaling_filter: String::new(),
            primary: false,
            enabled: true,
        }
    }

    fn names(mut displays: Vec<Display>) -> Vec<String> {
        assign_display_names(&mut displays);
        displays.into_iter().map(|d| d.name).collect()
    }

    #[test]
    fn displays_are_named_after_their_model() {
        let displays = vec![display("eDP-1", Some(edid("Laptop Panel", None))), display("DP-1", Some(edid("Contoso C27", None)))];
        assert_eq!(names(displays), ["Laptop Panel", "Contoso C27"]);
    }

    #[test]
    fn same_model_is_told_apart_by_serial() {
        let displays = vec![
            display("DP-1", Some(edid("Contoso C27", Some("ABC123")))),
            display("DP-2", Some(edid("Contoso C27", Some("XYZ789")))),
        ];
        assert_eq!(names(displays), ["Contoso C27 (S/N ABC123)", "Contoso C27 (S/N XYZ789)"]);
    }

    #[test]
    fn identical_edids_are_numbered() {
        let displays = vec![display("DP-1", Some(edid("Contoso C27", None))), display("DP-2", Some(edid("Contoso C27", None)))];
        assert_eq!(names(displays), ["Contoso C27 #1", "Contoso C27 #2"]);
    }

    #[test]
    fn displays_without_edid_are_unknown() {
        assert_eq!(names(vec![display("HDMI-1", None)]), [UNKNOWN_DISPLAY_NAME]);
    }

    #[test]
    fn names_never_contain_the_port() {
        let displays = vec![
            display("DP-1", Some(edid("Contoso C27", None))),
            display("DP-2", Some(edid("Contoso C27", None))),
            display("HDMI-1", None),
        ];
        for name in names(displays) {
            assert!(!name.contains("DP-") && !name.contains("HDMI"), "{}", name);
        }
    }
}
