use clap::{Parser, Args, value_parser};
use std::path::PathBuf;

use crate::config::{Config, Profile};
use crate::display::linux::{LinuxDisplayManager, watch_display_changes};
use crate::display::{Display, DisplayManager, create_display_manager};
use crate::error::{Error, Result};

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Show detailed information about each display
    #[arg(short, long)]
    pub detailed: bool,
}

#[derive(Debug, Args)]
pub struct ApplyArgs {
    /// Path to the configuration file
    #[arg(short, long, value_parser = value_parser!(PathBuf))]
    pub config: PathBuf,
    
    /// Profile name to apply (overrides automatic detection)
    #[arg(short, long)]
    pub profile: Option<String>,
}

#[derive(Debug, Args)]
pub struct CreateConfigArgs {
    /// Path to save the configuration file
    #[arg(short, long, value_parser = value_parser!(PathBuf))]
    pub output: PathBuf,

    /// Name of the profile to create from the current display layout
    #[arg(short, long, default_value = "detected")]
    pub profile: String,

    /// Add the profile to an existing configuration file instead of creating a new one
    #[arg(short, long)]
    pub add: bool,

    /// Overwrite the output file, or with --add replace a profile of the same name
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct DetectArgs {
    /// Path to the configuration file
    #[arg(short, long, value_parser = value_parser!(PathBuf))]
    pub config: PathBuf,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Path to the configuration file
    #[arg(short, long, value_parser = value_parser!(PathBuf))]
    pub config: PathBuf,
}

#[derive(Debug, clap::Parser)]
#[command(name = "disp")]
#[command(about = "Display configuration tool", long_about = None)]
#[command(version)]
pub struct Cli {
    /// Increase verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
    
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    /// List connected displays
    List(ListArgs),
    
    /// Apply a display configuration
    Apply(ApplyArgs),
    
    /// Create a configuration file from the current display layout
    CreateConfig(CreateConfigArgs),
    
    /// Detect the best matching profile for current displays
    Detect(DetectArgs),
    
    /// Watch for display changes and apply configurations automatically
    Watch(WatchArgs),
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    
    // Initialize logger
    let log_level = match cli.verbose {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    };
    
    // What disp did is shown as plain lines under the command's own output; details at
    // higher verbosity, warnings and errors are marked with their level. Other crates
    // only report warnings and errors.
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("disp", log_level)
        .format(|buf, record| {
            use std::io::Write;
            match record.level() {
                log::Level::Info => writeln!(buf, "  {}", record.args()),
                level => writeln!(buf, "{}: {}", level, record.args()),
            }
        })
        .init();
    
    match &cli.command {
        Commands::List(args) => {
            let display_manager = create_display_manager()?;
            let displays = display_manager.get_displays()?;
            
            println!("Connected displays:");
            for (i, display) in displays.iter().enumerate() {
                println!("{}. {} (on {})", i + 1, display.name, display.output);
                
                if args.detailed {
                    if let Some(edid) = &display.edid {
                        println!("   EDID: {}", edid);
                        println!("   EDID Hash: {}", edid.hash());
                        println!("   Product ID: {:#06x}", edid.product_id);
                        if let Some(serial) = edid.serial() {
                            println!("   Serial Number: {}", serial);
                        }
                        println!("   Manufactured: week {} of {}", edid.manufacture_week, edid.manufacture_year);
                        println!("   EDID Version: {}.{}", edid.version_major, edid.version_minor);
                        if let (Some(width), Some(height)) = (edid.width_cm, edid.height_cm) {
                            println!("   Physical Size: {} x {} cm", width, height);
                        }
                    }

                    if let Some(resolution) = display.current_resolution {
                        println!("   Resolution: {}x{}", resolution.0, resolution.1);
                    }

                    println!("   Position: ({}, {})", display.position.0, display.position.1);
                    println!("   Orientation: {}", display.orientation);
                    println!("   Scaling: {}", display.scaling);
                    println!("   Primary: {}", if display.primary { "Yes" } else { "No" });
                    println!("   Enabled: {}", if display.enabled { "Yes" } else { "No" });
                    
                    if !display.available_resolutions.is_empty() {
                        println!("   Available resolutions:");
                        for res in &display.available_resolutions {
                            println!("     - {}x{}", res.0, res.1);
                        }
                    }
                }
            }
        }
        
        Commands::Apply(args) => {
            let config = Config::from_file(&args.config)?;
            let display_manager = create_display_manager()?;
            let displays = display_manager.get_displays()?;
            
            let profile = if let Some(profile_name) = &args.profile {
                config.profiles.iter()
                    .find(|p| &p.name == profile_name)
                    .ok_or_else(|| crate::error::Error::Config(format!("Profile not found: {}", profile_name)))?
            } else {
                config.find_matching_profile(&displays)
                    .ok_or_else(|| crate::error::Error::NoMatchingConfig)?
            };
            
            println!("Applying profile: {}", profile.name);
            if let Some(desc) = &profile.description {
                println!("Description: {}", desc);
            }

            apply_profile(display_manager.as_ref(), &displays, profile)?;
        }
        
        Commands::CreateConfig(args) => {
            let adding = args.add && args.output.exists();
            if args.output.exists() && !args.add && !args.force {
                return Err(Error::Config(format!(
                    "{} already exists. Use --add to add a profile to it, or --force to overwrite it",
                    args.output.display()
                )));
            }

            let display_manager = create_display_manager()?;
            let displays = display_manager.get_displays()?;
            if displays.is_empty() {
                return Err(Error::DisplayConfig("No connected displays found".to_string()));
            }

            let profile = Profile::from_displays(&args.profile, &displays);
            let mut config = if adding {
                Config::from_file(&args.output)?
            } else {
                Config { default_profile: None, profiles: Vec::new() }
            };

            match config.profiles.iter().position(|p| p.name == args.profile) {
                Some(_) if !args.force => {
                    return Err(Error::Config(format!(
                        "{} already has a profile named \"{}\". Use --force to replace it, or --profile to choose another name",
                        args.output.display(), args.profile
                    )));
                }
                Some(index) => config.profiles[index] = profile,
                None => config.profiles.push(profile),
            }

            // Two profiles for the same monitors can't both be chosen automatically
            let monitors = |p: &Profile| {
                let mut hashes: Vec<Option<String>> = p.displays.iter().map(|d| d.edid_hash.clone()).collect();
                hashes.sort();
                hashes
            };
            let saved = config.profiles.iter().find(|p| p.name == args.profile).expect("profile was just added");
            for other in config.profiles.iter().filter(|p| p.name != args.profile) {
                if monitors(other) == monitors(saved) {
                    eprintln!(
                        "Warning: profile \"{}\" covers the same displays, so only one of the two will ever be chosen automatically",
                        other.name
                    );
                }
            }

            config.save_to_file(&args.output)?;

            let action = if adding { "Added" } else { "Saved" };
            println!("{} the current layout as profile \"{}\" to {}", action, args.profile, args.output.display());
            for (display, detected) in saved.displays.iter().zip(&displays) {
                let state = match (&display.resolution, display.position, display.enabled) {
                    (_, _, Some(false)) => "disabled".to_string(),
                    (Some(resolution), Some((x, y)), _) => format!("{} at ({}, {})", resolution, x, y),
                    _ => "enabled".to_string(),
                };
                let primary = if display.primary == Some(true) { ", primary" } else { "" };
                println!("  {} (currently on {}): {}{}", display.name, detected.output, state, primary);
            }
        }

        Commands::Detect(args) => {
            let config = Config::from_file(&args.config)?;
            let display_manager = create_display_manager()?;
            let displays = display_manager.get_displays()?;
            
            if let Some(profile) = config.find_matching_profile(&displays) {
                println!("Best matching profile: {}", profile.name);
                if let Some(desc) = &profile.description {
                    println!("Description: {}", desc);
                }

                let differences = profile.layout_differences(&displays);
                if differences.is_empty() {
                    println!("The current layout matches this profile");
                } else {
                    println!("The current layout differs from this profile:");
                    for difference in &differences {
                        println!("  - {}", difference);
                    }
                }

                println!("\nDisplays in profile:");
                for display_config in &profile.displays {
                    println!("- {}", display_config.name);
                    if let Some(res) = &display_config.resolution {
                        println!("  Resolution: {}", res);
                    }
                    if let Some(pos) = &display_config.position {
                        println!("  Position: ({}, {})", pos.0, pos.1);
                    }
                    if let Some(orientation) = &display_config.orientation {
                        println!("  Orientation: {}", orientation);
                    }
                    if let Some(primary) = &display_config.primary {
                        println!("  Primary: {}", if *primary { "Yes" } else { "No" });
                    }
                }
            } else {
                println!("No matching profile found");
            }
        }
        
        Commands::Watch(args) => {
            use notify::{EventKind, RecursiveMode, Watcher};
            use std::sync::mpsc::{RecvTimeoutError, channel};
            use std::time::Duration;

            enum WatchEvent {
                DisplaysChanged,
                ConfigChanged,
                Failed(Error),
            }

            let mut config = Config::from_file(&args.config)?;

            // The server reports hotplugs through RandR events, so there is no need to make it
            // probe every output on each read
            let display_manager = LinuxDisplayManager::new()?.without_probing();

            let (tx, rx) = channel();

            // Watch the directory rather than the file itself: editors often save by writing a
            // new file and renaming it over the old one, which would end a watch on the file
            let config_path = args.config.canonicalize()?;
            let config_dir = config_path.parent()
                .ok_or_else(|| Error::Config(format!("Invalid config path: {}", config_path.display())))?
                .to_path_buf();
            let config_name = config_path.file_name().map(|name| name.to_owned());

            let config_tx = tx.clone();
            let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                match res {
                    // Reading the config ourselves produces access events, which must not trigger a reload
                    Ok(event) if matches!(event.kind, EventKind::Access(_)) => {}
                    Ok(event) => {
                        if event.paths.iter().any(|path| path.file_name() == config_name.as_deref()) {
                            let _ = config_tx.send(WatchEvent::ConfigChanged);
                        }
                    }
                    Err(e) => {
                        let _ = config_tx.send(WatchEvent::Failed(Error::Config(format!("Config watcher failed: {}", e))));
                    }
                }
            })
            .map_err(|e| Error::Config(format!("Failed to create watcher: {}", e)))?;

            watcher.watch(&config_dir, RecursiveMode::NonRecursive)
                .map_err(|e| Error::Config(format!("Failed to watch path: {}", e)))?;

            watch_display_changes(move |res| {
                let _ = tx.send(match res {
                    Ok(()) => WatchEvent::DisplaysChanged,
                    Err(e) => WatchEvent::Failed(e),
                });
            })?;

            println!("Watching for display and configuration changes...");
            println!("Press Ctrl+C to exit");

            // Applies in a row that may leave the layout different from the profile before watch
            // gives up until the displays or the configuration change. This keeps a layout the
            // server can't reach, e.g. an unsupported scaling, from causing an endless loop.
            const MAX_ATTEMPTS: u32 = 3;

            // The connected displays, by port and EDID hash, that were last checked. The port is
            // part of this on purpose: a monitor moved to another port needs its layout again.
            let mut checked_for: Option<Vec<(String, Option<String>)>> = None;
            let mut attempts = 0;
            let mut gave_up = false;

            // Check the layout once at startup
            let mut displays_changed = true;
            let mut config_changed = false;

            loop {
                if config_changed {
                    match Config::from_file(&config_path) {
                        Ok(new_config) => {
                            config = new_config;
                            println!("Configuration reloaded");
                            // Start over, which also retries a profile that failed to apply
                            checked_for = None;
                        }
                        Err(e) => {
                            eprintln!("Error reloading configuration, keeping the previous one: {}", e);
                        }
                    }
                }

                if displays_changed || config_changed {
                    match display_manager.get_displays() {
                        Ok(displays) => {
                            let connected: Vec<(String, Option<String>)> = displays.iter()
                                .map(|d| (d.output.clone(), d.edid.as_ref().map(|e| e.hash())))
                                .collect();

                            let new_displays = checked_for.as_ref() != Some(&connected);
                            if new_displays {
                                attempts = 0;
                                gave_up = false;
                                let names: Vec<String> = displays.iter()
                                    .map(|d| format!("{} on {}", d.name, d.output))
                                    .collect();
                                println!("Connected displays: {}", names.join(", "));
                            }

                            // Compare the actual layout with the profile on every change, not only
                            // when monitors come and go: after sleep a dock can bring the same
                            // monitors back on the same ports with their layout reset
                            match config.find_matching_profile(&displays) {
                                Some(profile) => {
                                    let differences = profile.layout_differences(&displays);
                                    if differences.is_empty() {
                                        if new_displays {
                                            println!("Profile {} is already applied", profile.name);
                                        }
                                        attempts = 0;
                                        gave_up = false;
                                    } else if attempts < MAX_ATTEMPTS {
                                        let reason = if new_displays { "Layout differs from" } else { "Layout no longer matches" };
                                        println!("{} profile {}: {}", reason, profile.name, differences.join("; "));
                                        println!("Applying profile: {}", profile.name);
                                        if let Err(e) = apply_profile(&display_manager, &displays, profile) {
                                            eprintln!("Error applying configuration: {}", e);
                                        }
                                        attempts += 1;
                                    } else if !gave_up {
                                        eprintln!(
                                            "The layout still differs from profile {} after {} attempts: {}",
                                            profile.name, MAX_ATTEMPTS, differences.join("; ")
                                        );
                                        eprintln!("Not retrying until the displays or the configuration change");
                                        gave_up = true;
                                    }
                                }
                                None => {
                                    if new_displays {
                                        println!("No matching profile found");
                                    }
                                }
                            }

                            checked_for = Some(connected);
                        }
                        Err(e) => {
                            eprintln!("Error getting displays: {}", e);
                        }
                    }
                }

                // Wait for the next change, then keep collecting events until none arrive for a
                // second: a dock brings its monitors up one after another, and a config save or a
                // hotplug produces a burst of events, which should all lead to a single apply
                displays_changed = false;
                config_changed = false;

                let mut next = rx.recv().map_err(|_| RecvTimeoutError::Disconnected);
                loop {
                    match next {
                        Ok(WatchEvent::DisplaysChanged) => displays_changed = true,
                        Ok(WatchEvent::ConfigChanged) => config_changed = true,
                        Ok(WatchEvent::Failed(e)) => return Err(e),
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            return Err(Error::PlatformSpecific("Change listeners stopped unexpectedly".to_string()));
                        }
                    }
                    next = rx.recv_timeout(Duration::from_millis(1000));
                }
            }
        }
    }

    Ok(())
}

/// Apply a profile and print the outcome. A partially applied profile is reported, not returned as an error.
fn apply_profile(display_manager: &dyn DisplayManager, displays: &[Display], profile: &Profile) -> Result<()> {
    match display_manager.apply_config(displays, &profile.displays) {
        Ok(_) => println!("Configuration applied successfully"),
        Err(Error::PlatformSpecific(msg)) if msg.starts_with("Partial configuration applied with errors:") => {
            println!("{}", msg);
        }
        Err(e) => return Err(e),
    }

    Ok(())
}
