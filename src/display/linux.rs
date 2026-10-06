use std::collections::{HashMap, HashSet};
use x11rb::connection::Connection;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{self, ConnectionExt as _};

use crate::config::{DisplayConfig, match_displays};
use crate::display::{Display, DisplayManager, Edid, Orientation, SCALING_FILTER, assign_display_names};
use crate::error::{Error, Result};

pub struct LinuxDisplayManager {
    conn: std::sync::Arc<x11rb::rust_connection::RustConnection>,
    root: xproto::Window,
    /// Millimeters per pixel (horizontal, vertical), used to keep the DPI when resizing the screen
    mm_per_px: (f64, f64),
    /// Whether to make the server probe outputs for changes when reading screen resources
    probe_outputs: bool,
}

/// The target state of one enabled display, computed before anything is changed
struct CrtcPlan<'a> {
    display: &'a Display,
    output: randr::Output,
    /// CRTC to drive this display with, assigned after every display is planned
    crtc: randr::Crtc,
    /// CRTC currently driving this display, or 0 if it is off
    current_crtc: randr::Crtc,
    possible_crtcs: Vec<randr::Crtc>,
    mode: randr::Mode,
    /// Width and height of `mode`
    mode_size: (u16, u16),
    position: (i16, i16),
    rotation: randr::Rotation,
    orientation: Orientation,
    scaling: f64,
    primary: bool,
    /// Area covered on the screen, after rotation and scaling
    size: (u32, u32),
}

impl CrtcPlan<'_> {
    /// One line describing what the display is set to, e.g.
    /// "Contoso C27 on DP-3-3: 3840x2160 at (5120, 0), scaled 1.32999"
    fn summary(&self) -> String {
        let mut summary = format!(
            "{} on {}: {}x{} at ({}, {})",
            self.display.name, self.display.output, self.mode_size.0, self.mode_size.1, self.position.0, self.position.1
        );
        if self.orientation != Orientation::Normal {
            summary.push_str(&format!(", rotated {}", self.orientation));
        }
        if self.scaling != 1.0 {
            summary.push_str(&format!(", scaled {}", self.scaling));
        }
        if self.primary {
            summary.push_str(", primary");
        }
        summary
    }
}

impl LinuxDisplayManager {
    pub fn new() -> Result<Self> {
        // Connect to the X server
        let (conn, screen_num) = x11rb::connect(None)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to connect to X server: {}", e)))?;
        let conn = std::sync::Arc::new(conn);
        
        // Get the root window
        let setup = conn.setup();
        let screen = setup.roots.get(screen_num as usize)
            .ok_or_else(|| Error::PlatformSpecific("Failed to get screen".to_string()))?;
        let root = screen.root;

        // Fall back to 96 DPI if the server reports no physical size
        let ratio = |mm: u16, px: u16| if mm > 0 && px > 0 { mm as f64 / px as f64 } else { 25.4 / 96.0 };
        let mm_per_px = (
            ratio(screen.width_in_millimeters, screen.width_in_pixels),
            ratio(screen.height_in_millimeters, screen.height_in_pixels),
        );

        Ok(LinuxDisplayManager {
            conn,
            root,
            mm_per_px,
            probe_outputs: true,
        })
    }

    /// Read the server's current state instead of making it probe every output.
    ///
    /// Probing is slow and can make some displays flicker. It isn't needed when the
    /// server reports hotplugs through RandR events, as it does for `watch`.
    pub fn without_probing(mut self) -> Self {
        self.probe_outputs = false;
        self
    }

    /// Fetch the screen resources from the server.
    ///
    /// This is done on every call rather than once at startup: a hotplug changes the
    /// config timestamp and the mode list, and the server refuses SetCrtcConfig
    /// requests that carry an outdated timestamp.
    fn screen_resources(&self) -> Result<randr::GetScreenResourcesReply> {
        if self.probe_outputs {
            return self.conn.randr_get_screen_resources(self.root)
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen resources: {}", e)))?
                .reply()
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen resources reply: {}", e)));
        }

        let current = self.conn.randr_get_screen_resources_current(self.root)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen resources: {}", e)))?
            .reply()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen resources reply: {}", e)))?;

        // Both replies carry the same fields
        Ok(randr::GetScreenResourcesReply {
            sequence: current.sequence,
            length: current.length,
            timestamp: current.timestamp,
            config_timestamp: current.config_timestamp,
            crtcs: current.crtcs,
            outputs: current.outputs,
            modes: current.modes,
            names: current.names,
        })
    }

    fn find_mode(output_info: &randr::GetOutputInfoReply, resources: &randr::GetScreenResourcesReply, width: u32, height: u32) -> Option<randr::Mode> {
        output_info.modes.iter()
            .filter_map(|&mode_id| {
                resources.modes.iter()
                    .find(|m| m.id == mode_id && m.width as u32 == width && m.height as u32 == height)
                    .map(|m| m.id)
            })
            .next()
    }

    /// Set a CRTC configuration and check the status the server replies with
    fn set_crtc(&self, crtc: randr::Crtc, config_timestamp: xproto::Timestamp, position: (i16, i16), mode: randr::Mode, rotation: randr::Rotation, outputs: &[randr::Output]) -> std::result::Result<(), String> {
        let reply = self.conn.randr_set_crtc_config(
            crtc,
            0, // current_timestamp
            config_timestamp,
            position.0,
            position.1,
            mode,
            rotation,
            outputs,
        )
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;

        if reply.status != randr::SetConfig::SUCCESS {
            return Err(format!("server refused CRTC configuration (status {:?})", reply.status));
        }
        Ok(())
    }

    fn set_screen_size(&self, width: u32, height: u32) -> Result<()> {
        log::debug!("Setting screen size to {}x{}", width, height);
        let mm_width = (width as f64 * self.mm_per_px.0).round() as u32;
        let mm_height = (height as f64 * self.mm_per_px.1).round() as u32;

        self.conn.randr_set_screen_size(self.root, width as u16, height as u16, mm_width, mm_height)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to set screen size: {}", e)))?
            .check()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to set screen size to {}x{}: {}", width, height, e)))
    }

    /// Read the scaling factor of a CRTC from its transform, the same value `apply_config`
    /// sets from a config, and the filter used for it. Returns 1.0 and no filter if the
    /// server doesn't support transforms.
    fn crtc_scaling(&self, crtc: randr::Crtc, output_name: &str) -> (f64, String) {
        let reply = match self.conn.randr_get_crtc_transform(crtc).ok().and_then(|cookie| cookie.reply().ok()) {
            Some(reply) => reply,
            None => return (1.0, String::new()),
        };
        let transform = reply.current_transform;
        let filter = String::from_utf8_lossy(&reply.current_filter_name).to_string();

        if transform.matrix11 <= 0 {
            log::warn!("The display on {} has an unsupported transform, reporting scaling 1.0", output_name);
            return (1.0, filter);
        }
        if transform.matrix11 != transform.matrix22 {
            log::warn!("The display on {} is scaled differently horizontally and vertically, reporting the horizontal scaling", output_name);
        }

        (Self::scaling_from_matrix_value(transform.matrix11), filter)
    }

    /// The scaling factor for a 16.16 fixed-point transform matrix diagonal.
    ///
    /// Scaling has the same meaning as `xrandr --scale`: the number of desktop pixels per
    /// monitor pixel, so 2.0 shows a desktop twice the size of the mode in each direction.
    /// Uses the fewest decimals that still convert back to exactly the same matrix value
    /// in `scaling_matrix_value`, so configs show e.g. 1.33 rather than 1.3299865723.
    fn scaling_from_matrix_value(matrix_value: i32) -> f64 {
        let scaling = matrix_value as f64 / 65536.0;
        (0..=12)
            .map(|decimals| {
                let factor = 10f64.powi(decimals);
                (scaling * factor).round() / factor
            })
            .find(|&rounded| rounded > 0.0 && Self::scaling_matrix_value(rounded) == matrix_value)
            .unwrap_or(scaling)
    }

    /// The 16.16 fixed-point diagonal of the transform matrix for a scaling factor
    fn scaling_matrix_value(scaling: f64) -> i32 {
        (scaling * 65536.0).round() as i32
    }

    fn current_screen_size(&self) -> Result<(u32, u32)> {
        let geometry = self.conn.get_geometry(self.root)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen size: {}", e)))?
            .reply()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen size reply: {}", e)))?;
        Ok((geometry.width as u32, geometry.height as u32))
    }

    fn parse_resolution(mode_info: &randr::ModeInfo) -> (u32, u32) {
        (mode_info.width as u32, mode_info.height as u32)
    }
    
    fn parse_orientation(rotation: u16) -> Orientation {
        if rotation == randr::Rotation::ROTATE0.into() {
            Orientation::Normal
        } else if rotation == randr::Rotation::ROTATE90.into() {
            Orientation::Right
        } else if rotation == randr::Rotation::ROTATE180.into() {
            Orientation::Inverted
        } else if rotation == randr::Rotation::ROTATE270.into() {
            Orientation::Left
        } else {
            Orientation::Normal
        }
    }
    
    fn get_edid_from_output(&self, output: randr::Output) -> Result<Option<Edid>> {
        // Get the EDID atom
        let edid_atom = self.conn.intern_atom(true, b"EDID")
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get EDID atom: {}", e)))?
            .reply()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get EDID atom reply: {}", e)))?
            .atom;

        // The atom only exists once some output has exposed an EDID property
        if edid_atom == x11rb::NONE {
            return Ok(None);
        }

        // Get the EDID property
        let edid_prop = self.conn.randr_get_output_property(
            output,
            edid_atom,
            xproto::Atom::from(xproto::AtomEnum::NONE),
            0,
            128, // EDID is typically 128 bytes
            false,
            false,
        )
        .map_err(|e| Error::PlatformSpecific(format!("Failed to get EDID property: {}", e)))?
        .reply()
        .map_err(|e| Error::PlatformSpecific(format!("Failed to get EDID property reply: {}", e)))?;
        
        // Check if we got EDID data
        if edid_prop.format != 8 || edid_prop.type_ == xproto::Atom::from(xproto::AtomEnum::NONE) || edid_prop.num_items == 0 {
            return Ok(None);
        }
        
        // Parse the EDID data
        Edid::parse(&edid_prop.data).map(Some)
    }
}

/// Call `on_change` from a background thread whenever the X server reports that outputs
/// or the screen configuration changed, e.g. when a monitor is plugged in or removed.
///
/// The listener uses its own X connection, so it never competes with requests made through
/// a `LinuxDisplayManager`. If the connection fails, `on_change` receives the error once and
/// the thread exits.
pub fn watch_display_changes<F>(on_change: F) -> Result<()>
where
    F: Fn(Result<()>) + Send + 'static,
{
    let (conn, screen_num) = x11rb::connect(None)
        .map_err(|e| Error::PlatformSpecific(format!("Failed to connect to X server: {}", e)))?;
    let root = conn.setup().roots.get(screen_num)
        .ok_or_else(|| Error::PlatformSpecific("Failed to get screen".to_string()))?
        .root;

    // Output changes report monitors being connected or removed, property changes report
    // a monitor's EDID becoming available (it can arrive after the connection), and CRTC
    // changes report the layout changing, e.g. a dock resetting it after sleep
    let events = randr::NotifyMask::SCREEN_CHANGE
        | randr::NotifyMask::OUTPUT_CHANGE
        | randr::NotifyMask::OUTPUT_PROPERTY
        | randr::NotifyMask::CRTC_CHANGE;
    conn.randr_select_input(root, events)
        .map_err(|e| Error::PlatformSpecific(format!("Failed to subscribe to display changes: {}", e)))?
        .check()
        .map_err(|e| Error::PlatformSpecific(format!("Failed to subscribe to display changes: {}", e)))?;

    std::thread::spawn(move || {
        loop {
            match conn.wait_for_event() {
                Ok(event) => {
                    log::debug!("X event: {:?}", event);
                    on_change(Ok(()));
                }
                Err(e) => {
                    on_change(Err(Error::PlatformSpecific(format!("Lost connection to X server: {}", e))));
                    return;
                }
            }
        }
    });

    Ok(())
}

impl DisplayManager for LinuxDisplayManager {
    fn get_displays(&self) -> Result<Vec<Display>> {
        let resources = self.screen_resources()?;

        // 0 if no output is marked as primary
        let primary_output = self.conn.randr_get_output_primary(self.root)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get primary output: {}", e)))?
            .reply()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get primary output reply: {}", e)))?
            .output;

        let mut displays = Vec::new();
        let mut mode_map = HashMap::new();
        
        // Create a map of mode ID to mode info
        for mode in &resources.modes {
            mode_map.insert(mode.id, mode);
        }
        
        // Process each output
        for &output in &resources.outputs {
            // Get output info
            let output_info = self.conn.randr_get_output_info(output, resources.config_timestamp)
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get output info: {}", e)))?
                .reply()
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get output info reply: {}", e)))?;
            
            // Skip disconnected outputs
            if output_info.connection != randr::Connection::CONNECTED {
                continue;
            }
            
            // Get the connector name, e.g. "DP-1"
            let output_name = String::from_utf8_lossy(&output_info.name).to_string();

            // Try to get EDID data. A missing or malformed EDID (common with adapters and
            // KVMs) only means this display can't be matched by EDID hash.
            let edid = self.get_edid_from_output(output).unwrap_or_else(|e| {
                log::warn!("Ignoring unreadable EDID for the display on {}: {}", output_name, e);
                None
            });

            // Get crtc info if available
            let (current_resolution, position, orientation, (scaling, scaling_filter), enabled) = if output_info.crtc != 0 {
                let crtc_info = self.conn.randr_get_crtc_info(output_info.crtc, resources.config_timestamp)
                    .map_err(|e| Error::PlatformSpecific(format!("Failed to get CRTC info: {}", e)))?
                    .reply()
                    .map_err(|e| Error::PlatformSpecific(format!("Failed to get CRTC info reply: {}", e)))?;

                let current_mode = mode_map.get(&crtc_info.mode).map(|m| Self::parse_resolution(m));
                let position = (crtc_info.x as i32, crtc_info.y as i32);
                let orientation = Self::parse_orientation(crtc_info.rotation.into());
                let scaling = self.crtc_scaling(output_info.crtc, &output_name);
                let enabled = crtc_info.mode != 0;

                (current_mode, position, orientation, scaling, enabled)
            } else {
                (None, (0, 0), Orientation::Normal, (1.0, String::new()), false)
            };

            // Get available resolutions
            let available_resolutions = output_info.modes.iter()
                .filter_map(|&mode_id| mode_map.get(&mode_id).map(|m| Self::parse_resolution(m)))
                .collect();

            // Create display. The name is assigned once all displays are known.
            let display = Display {
                id: format!("{}", output),
                output: output_name,
                name: String::new(),
                edid,
                current_resolution,
                available_resolutions,
                position,
                orientation,
                scaling,
                scaling_filter,
                primary: output == primary_output,
                enabled,
            };

            displays.push(display);
        }

        assign_display_names(&mut displays);

        Ok(displays)
    }
    
    fn apply_config(&self, displays: &[Display], configs: &[DisplayConfig]) -> Result<()> {
        log::debug!("Applying configuration to {} displays with {} configs", displays.len(), configs.len());

        let resources = self.screen_resources()?;

        // Create a map of display ID to config based on EDID hash or fallback to name
        let mut config_map = HashMap::new();
        for m in match_displays(configs, displays) {
            log::debug!("Config {} matches display {} on {} by {:?}", m.config.name, m.display.name, m.display.output, m.kind);
            config_map.insert(&m.display.id, m.config);
        }
        for config in configs {
            if !config_map.values().any(|c| std::ptr::eq(*c, config)) {
                log::debug!("Config {} matches no connected display", config.name);
            }
        }

        log::debug!("Matched {} displays to configurations", config_map.len());

        // Work out the target state of every enabled display before changing anything, so
        // an invalid config is rejected while the current layout is still intact
        let mut plans = Vec::new();
        for display in displays {
            let config = match config_map.get(&display.id) {
                Some(c) => c,
                None => {
                    log::debug!("No configuration for display {} on {}, it will be disabled", display.name, display.output);
                    continue;
                }
            };

            if !config.enabled.unwrap_or(true) {
                log::debug!("Display {} on {} is configured to be disabled", display.name, display.output);
                continue;
            }

            log::debug!("Planning display: {} on {}", display.name, display.output);

            // Parse output ID
            let output = display.id.parse::<u32>()
                .map_err(|_| Error::DisplayConfig(format!("Invalid output ID: {}", display.id)))?;

            // Get output info
            let output_info = self.conn.randr_get_output_info(output, resources.config_timestamp)
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get output info: {}", e)))?
                .reply()
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get output info reply: {}", e)))?;

            // Skip disconnected outputs
            if output_info.connection != randr::Connection::CONNECTED {
                log::debug!("  Display is no longer connected, skipping");
                continue;
            }

            // The CRTC is assigned once every display has been planned
            if output_info.crtcs.is_empty() {
                return Err(Error::DisplayConfig(format!("No CRTC available for display {}", display.name)));
            }

            // Log available modes for this display
            log::trace!("  Available modes for display {}:", display.name);
            for &mode_id in &output_info.modes {
                if let Some(mode_info) = resources.modes.iter().find(|m| m.id == mode_id) {
                    log::trace!("    Mode {}: {}x{}", mode_id, mode_info.width, mode_info.height);
                }
            }

            // Parse resolution
            let mode = if let Some(resolution_str) = &config.resolution {
                let parts: Vec<&str> = resolution_str.split('x').collect();
                if parts.len() != 2 {
                    return Err(Error::DisplayConfig(format!("Invalid resolution format: {}", resolution_str)));
                }

                let width = parts[0].parse::<u32>()
                    .map_err(|_| Error::DisplayConfig(format!("Invalid width: {}", parts[0])))?;
                let height = parts[1].parse::<u32>()
                    .map_err(|_| Error::DisplayConfig(format!("Invalid height: {}", parts[1])))?;

                log::debug!("  Looking for mode with resolution: {}x{}", width, height);
                let mode_id = Self::find_mode(&output_info, &resources, width, height)
                    .ok_or_else(|| Error::DisplayConfig(format!("Resolution not available for display {}: {}", display.name, resolution_str)))?;

                log::debug!("  Found matching mode: {}", mode_id);
                mode_id
            } else if let Some((width, height)) = display.current_resolution {
                // Keep current resolution
                log::debug!("  Using current resolution: {}x{}", width, height);

                let mode_id = Self::find_mode(&output_info, &resources, width, height)
                    .ok_or_else(|| Error::DisplayConfig("Current resolution not found in available modes".to_string()))?;

                log::debug!("  Found matching mode for current resolution: {}", mode_id);
                mode_id
            } else {
                // Use first available mode
                let mode_id = *output_info.modes.first()
                    .ok_or_else(|| Error::DisplayConfig("No modes available".to_string()))?;

                log::debug!("  Using first available mode: {}", mode_id);
                mode_id
            };

            let mode_info = resources.modes.iter().find(|m| m.id == mode)
                .ok_or_else(|| Error::DisplayConfig(format!("Mode {} not found in screen resources", mode)))?;

            // Parse position
            let position = config.position.unwrap_or(display.position);
            let position = match (i16::try_from(position.0), i16::try_from(position.1)) {
                (Ok(x), Ok(y)) if x >= 0 && y >= 0 => (x, y),
                _ => return Err(Error::DisplayConfig(format!("Invalid position for display {}: {:?}", display.name, position))),
            };

            // Parse orientation
            let orientation = config.orientation.as_deref().map(Orientation::from).unwrap_or(display.orientation);
            let rotation = match orientation {
                Orientation::Normal => randr::Rotation::ROTATE0,
                Orientation::Right => randr::Rotation::ROTATE90,
                Orientation::Inverted => randr::Rotation::ROTATE180,
                Orientation::Left => randr::Rotation::ROTATE270,
            };

            // Parse scaling
            let scaling = config.scaling.unwrap_or(1.0);
            if !(scaling > 0.0) {
                return Err(Error::DisplayConfig(format!("Invalid scaling for display {}: {}", display.name, scaling)));
            }

            // Area this display covers on the screen: rotation swaps the axes, and the
            // scaling transform maps each monitor pixel to `scaling` screen pixels. This uses
            // the same fixed-point matrix the server gets, so both arrive at the same size.
            let (mut width, mut height) = (mode_info.width as u64, mode_info.height as u64);
            if rotation == randr::Rotation::ROTATE90 || rotation == randr::Rotation::ROTATE270 {
                std::mem::swap(&mut width, &mut height);
            }
            let matrix_value = Self::scaling_matrix_value(scaling) as u64;
            let size = (
                (width * matrix_value).div_ceil(65536) as u32,
                (height * matrix_value).div_ceil(65536) as u32,
            );

            plans.push(CrtcPlan {
                display,
                output,
                crtc: 0,
                current_crtc: output_info.crtc,
                possible_crtcs: output_info.crtcs,
                mode,
                mode_size: (mode_info.width, mode_info.height),
                position,
                rotation,
                orientation,
                scaling,
                primary: config.primary.unwrap_or(false),
                size,
            });
        }

        // Keep displays on the CRTC they already use, then give each remaining display a
        // CRTC that no other display in the new layout uses. Picking a CRTC per display in
        // isolation could take one that is driving another display we want to keep on.
        let mut used_crtcs = HashSet::new();
        for plan in &mut plans {
            if plan.current_crtc != 0 && used_crtcs.insert(plan.current_crtc) {
                plan.crtc = plan.current_crtc;
            }
        }
        for plan in plans.iter_mut().filter(|p| p.crtc == 0) {
            plan.crtc = plan.possible_crtcs.iter().copied()
                .find(|crtc| !used_crtcs.contains(crtc))
                .ok_or_else(|| Error::DisplayConfig(format!(
                    "No free CRTC for display {}: the graphics hardware can't drive this many displays at once",
                    plan.display.name
                )))?;
            used_crtcs.insert(plan.crtc);
        }

        // Never leave every display switched off. This happens when no display in the
        // profile matches a connected one, e.g. a profile still using placeholder names.
        if plans.is_empty() {
            return Err(Error::DisplayConfig(
                "Refusing to apply a profile that leaves no display enabled. Check that the display names or EDID hashes in the profile match the output of `disp list --detailed`".to_string()
            ));
        }

        // The screen must be large enough to contain every CRTC
        let screen_size = plans.iter().fold((0, 0), |(w, h), p| {
            (w.max(p.position.0 as u32 + p.size.0), h.max(p.position.1 as u32 + p.size.1))
        });
        let range = self.conn.randr_get_screen_size_range(self.root)
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen size range: {}", e)))?
            .reply()
            .map_err(|e| Error::PlatformSpecific(format!("Failed to get screen size range reply: {}", e)))?;
        if screen_size.0 > range.max_width as u32 || screen_size.1 > range.max_height as u32 {
            return Err(Error::DisplayConfig(format!(
                "Layout needs a {}x{} screen, but the maximum supported size is {}x{}",
                screen_size.0, screen_size.1, range.max_width, range.max_height
            )));
        }
        let screen_size = (screen_size.0.max(range.min_width as u32), screen_size.1.max(range.min_height as u32));
        log::debug!("Layout needs a screen size of {}x{}", screen_size.0, screen_size.1);

        // Track errors but don't fail immediately
        let mut errors = Vec::new();

        // Turn off every active CRTC the new layout doesn't use: unmatched displays,
        // displays configured to be disabled, and CRTCs left on for disconnected outputs.
        // Doing this first frees up CRTCs and screen space for the new layout.
        log::debug!("Disabling displays that are not part of the new layout");
        for &crtc in &resources.crtcs {
            if plans.iter().any(|p| p.crtc == crtc) {
                continue;
            }

            let crtc_info = self.conn.randr_get_crtc_info(crtc, resources.config_timestamp)
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get CRTC info: {}", e)))?
                .reply()
                .map_err(|e| Error::PlatformSpecific(format!("Failed to get CRTC info reply: {}", e)))?;
            if crtc_info.mode == 0 {
                continue;
            }

            let names: Vec<&str> = displays.iter()
                .filter(|d| crtc_info.outputs.iter().any(|o| o.to_string() == d.id))
                .map(|d| d.name.as_str())
                .collect();
            let target = if names.is_empty() { format!("CRTC {}", crtc) } else { names.join(", ") };

            log::info!("{}: off", target);
            if let Err(e) = self.set_crtc(crtc, resources.config_timestamp, (0, 0), 0, randr::Rotation::ROTATE0, &[]) {
                let err = format!("Failed to disable {}: {}", target, e);
                log::error!("  {}", err);
                errors.push(err);
            }
        }

        // Grow the screen first so every CRTC fits while it is being moved, then set
        // the exact size once the new layout is in place
        let current_size = self.current_screen_size()?;
        let interim_size = (current_size.0.max(screen_size.0), current_size.1.max(screen_size.1));
        if interim_size != current_size {
            self.set_screen_size(interim_size.0, interim_size.1)?;
        }

        for plan in &plans {
            let display = plan.display;
            log::debug!("Enabling display {} on {} with mode {} at ({}, {}), rotation {:?}",
                display.name, display.output, plan.mode, plan.position.0, plan.position.1, plan.rotation);

            // Set the scaling transform. This must come before SetCrtcConfig: the server only
            // stores the transform as pending and applies it with the next CRTC configuration.
            // The transform matrix is a 3x3 matrix in row-major order. For scaling, we use
            // the same matrix as `xrandr --scale`:
            // | scale    0     0 |
            // |   0    scale   0 |
            // |   0      0     1 |
            let scale_factor = Self::scaling_matrix_value(plan.scaling);
            let transform = x11rb::protocol::render::Transform {
                matrix11: scale_factor,
                matrix12: 0,
                matrix13: 0,
                matrix21: 0,
                matrix22: scale_factor,
                matrix23: 0,
                matrix31: 0,
                matrix32: 0,
                matrix33: 65536,
            };

            // Scaled displays use the same smoothing filter as `xrandr --scale`. With "nearest",
            // scaling down drops desktop pixels and breaks up thin font strokes.
            let identity = scale_factor == 65536;
            let filter_name = if identity { "nearest" } else { SCALING_FILTER };

            // Without scaling, only reset a transform left behind by an earlier scaled layout.
            // Some servers (e.g. Xvfb) reject transforms entirely, even the identity.
            let needs_transform = !identity || self.conn.randr_get_crtc_transform(plan.crtc).ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|t| t.current_transform != transform || t.pending_transform != transform);

            if needs_transform {
                log::debug!("  Setting transform for scaling factor {} with the {} filter", plan.scaling, filter_name);

                match self.conn.randr_set_crtc_transform(
                    plan.crtc,
                    transform,
                    filter_name.as_bytes(),
                    &[], // No filter parameters
                ) {
                    Ok(cookie) => {
                        match cookie.check() {
                            Ok(_) => log::debug!("  Transform set successfully"),
                            Err(e) => {
                                let err = format!("Failed to apply transform for display {}: {}", display.name, e);
                                log::error!("  {}", err);
                                errors.push(err);
                            }
                        }
                    },
                    Err(e) => {
                        let err = format!("Failed to set transform for display {}: {}", display.name, e);
                        log::error!("  {}", err);
                        errors.push(err);
                    }
                }
            }

            if let Err(e) = self.set_crtc(plan.crtc, resources.config_timestamp, plan.position, plan.mode, plan.rotation, &[plan.output]) {
                let err = format!("Failed to configure display {}: {}", display.name, e);
                log::error!("  {}", err);
                errors.push(err);
                continue;
            }
            log::debug!("  CRTC configuration successful");
            log::info!("{}", plan.summary());

            // Set primary if requested
            if plan.primary {
                log::debug!("  Setting as primary display");

                let result = self.conn.randr_set_output_primary(self.root, plan.output)
                    .map_err(|e| e.to_string())
                    .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
                match result {
                    Ok(_) => log::debug!("  Set primary successful"),
                    Err(e) => {
                        let err = format!("Failed to set primary output for display {}: {}", display.name, e);
                        log::error!("  {}", err);
                        errors.push(err);
                    }
                }
            }
        }

        if screen_size != interim_size
            && let Err(e) = self.set_screen_size(screen_size.0, screen_size.1)
        {
            log::error!("{}", e);
            errors.push(e.to_string());
        }

        // If there were any errors, return them as a combined error
        if !errors.is_empty() {
            let error_msg = errors.join("; ");
            log::warn!("Failed to apply configuration to some displays: {}", error_msg);
            // Return a warning as an error to indicate partial success
            return Err(Error::PlatformSpecific(format!("Partial configuration applied with errors: {}", error_msg)));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A captured scaling factor must reproduce the exact transform, or the screen size
    // computed for the layout drifts from what the server uses
    #[test]
    fn scaling_round_trips_to_the_same_matrix() {
        // From 0.25x (a desktop a quarter of the mode's size) to 4x (four times the size)
        for matrix_value in 16384..=262144 {
            let scaling = LinuxDisplayManager::scaling_from_matrix_value(matrix_value);
            assert_eq!(LinuxDisplayManager::scaling_matrix_value(scaling), matrix_value, "scaling {}", scaling);
        }
    }

    // Same meaning as `xrandr --scale`: a matrix diagonal of 2.0 is scaling 2.0
    #[test]
    fn common_scalings_are_exact() {
        assert_eq!(LinuxDisplayManager::scaling_from_matrix_value(131072), 2.0);
        assert_eq!(LinuxDisplayManager::scaling_from_matrix_value(65536), 1.0);
        assert_eq!(LinuxDisplayManager::scaling_from_matrix_value(32768), 0.5);
    }
}
