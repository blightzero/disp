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
    
    /// Display name
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
