# disp - Display Configuration Tool

`disp` is a Linux tool for configuring multiple monitors based on their EDID (Extended Display Identification Data). It allows you to define configurations for different combinations of monitors and automatically applies the best matching configuration when monitors are connected or disconnected.

## Features

- Detect connected monitors using EDID data for reliable identification
- Define multiple configuration profiles for different monitor setups
- Configure resolution, orientation, position, and scaling for each monitor
- Set the primary display
- Automatically apply the best matching configuration based on connected monitors
- Watch for display changes and apply configurations automatically

## Supported Platforms

`disp` supports Linux only, on X11 with the RandR extension. Other operating systems are not supported, and the project does not build on them.

## Installation

### Using the Static Binary (Linux)

1. Download the latest static binary from the releases page
2. Make it executable:
   ```
   chmod +x disp
   ```
3. Move it to a directory in your PATH:
   ```
   sudo mv disp /usr/local/bin/
   ```

### Using the Installation Script (Linux)

1. Clone the repository:
   ```
   git clone https://github.com/yourusername/disp.git
   cd disp
   ```

2. Build the static binary:
   ```
   cargo build --release --target x86_64-unknown-linux-musl
   ```

3. Run the installation script:
   ```
   sudo ./install.sh
   ```

   You can also specify a custom installation prefix:
   ```
   sudo ./install.sh --prefix=/opt
   ```

### From Source (Dynamic Build)

1. Clone the repository:
   ```
   git clone https://github.com/yourusername/disp.git
   cd disp
   ```

2. Build the project:
   ```
   cargo build --release
   ```

3. The binary will be available at `target/release/disp`

### Building a Static Binary

To build a fully static binary that doesn't depend on any system libraries:

1. Install the musl target:
   ```
   rustup target add x86_64-unknown-linux-musl
   ```

2. Build the static binary:
   ```
   cargo build --release --target x86_64-unknown-linux-musl
   ```

3. The static binary will be available at `target/x86_64-unknown-linux-musl/release/disp`

## Usage

### List Connected Displays

```
disp list
```

For detailed information about each display:

```
disp list --detailed
```

### Create a Configuration from the Current Layout

Arrange your displays the way you want them (for example with `xrandr` or your desktop's display settings), then run:

```
disp create-config --output config.toml
```

This saves the current layout as a profile named "detected": for every connected display it records its name and EDID hash, resolution, position, orientation, scaling and primary flag. Connected displays that are turned off are saved with `enabled = false`. Use `--profile <name>` to choose a different profile name. An existing file is not overwritten unless you pass `--force`.

### Apply a Configuration

```
disp apply --config config.toml
```

To apply a specific profile:

```
disp apply --config config.toml --profile "docked"
```

### Detect the Best Matching Profile

```
disp detect --config config.toml
```

### Watch for Display Changes

```
disp watch --config config.toml
```

`watch` applies the best matching profile at startup and again whenever a monitor is connected, disconnected or swapped for a different one. It waits for change events from the X server rather than polling, so it uses no CPU while idle. Saving the configuration file reloads it and re-applies the matching profile; this also retries a profile that failed to apply. If the new file is invalid, `watch` reports the error and keeps using the previous configuration.

## Configuration File Format

The configuration file is in TOML format. The easiest way to start is `disp create-config`; here's an example of a laptop with an external monitor:

```toml
# Default profile to use when no profile matches
default_profile = "laptop"

# Available profiles
[[profiles]]
name = "laptop"
description = "Laptop screen only"

  [[profiles.displays]]
  name = "ACM 1A2B"
  edid_hash = "0f1e2d3c4b5a6978" # From `disp list --detailed`
  resolution = "1920x1080"
  orientation = "normal"
  position = [0, 0]
  scaling = 1.0
  primary = true
  enabled = true

[[profiles]]
name = "docked"
description = "External monitor to the right of the closed laptop"

  [[profiles.displays]]
  name = "ACM 1A2B"
  edid_hash = "0f1e2d3c4b5a6978"
  enabled = false

  [[profiles.displays]]
  name = "Contoso C27"
  edid_hash = "8796a5b4c3d2e1f0"
  resolution = "2560x1440"
  orientation = "normal"
  position = [0, 0]
  scaling = 1.0
  primary = true
  enabled = true
```

### Configuration Options

- `default_profile`: The profile to use when no profile matches the connected displays
- `profiles`: A list of configuration profiles
  - `name`: The name of the profile
  - `description`: A description of the profile
  - `displays`: A list of display configurations
    - `name`: What the display is, as shown by `disp list` (e.g. "Contoso C27"). Names come from the EDID, never from the port: monitors of the same model get their serial number added (e.g. "Fabrikam F24 (S/N AB12345C)"), monitors with identical EDIDs are numbered ("#1", "#2"), and displays without an EDID are called "Unknown display"
    - `edid_hash`: The EDID hash that identifies the display (optional). It takes precedence over `name`. When several connected displays share a hash, `name` decides which config goes to which display
    - `resolution`: The display resolution (e.g., "1920x1080")
    - `orientation`: The display orientation ("normal", "left", "right", "inverted")
    - `position`: The display position as [x, y] coordinates
    - `scaling`: The display scaling factor, with the same meaning as `xrandr --scale`. `2.0` doubles the desktop resolution: a 2560x1440 mode covers a 5120x2880 area, so everything looks half as large. `0.5` halves it, so everything looks twice as large. `create-config` records the exact value currently in use
    - `primary`: Whether this is the primary display. At most one display per profile can be primary, and it can't also have `enabled = false`. If no display in a profile is marked primary, the current primary display is left unchanged
    - `enabled`: Whether the display should be enabled. Connected displays that don't match any display in the profile are turned off. A profile that would leave no display enabled is refused

## Displays Are Identified by What They Are

Profiles describe monitors, not ports. A display is matched by its EDID hash (or, without one, by its name from the EDID), so a profile keeps working when you plug a monitor into a different port or swap cables between two monitors: each monitor still gets its own resolution, position and scaling. `watch` notices a monitor moving to another port and applies the layout again.

## Getting EDID Hashes

To get the EDID hash for a display, use the `list` command with the `--detailed` flag:

```
disp list --detailed
```

This will show the EDID hash for each connected display. You can then use this hash in your configuration file to reliably identify displays.

## License

This project is licensed under the MIT License - see the LICENSE file for details.
