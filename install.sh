#!/bin/bash

# Installation script for disp (Display Configuration Tool)

# Default installation directory
INSTALL_DIR="/usr/local/bin"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --prefix=*)
      PREFIX="${1#*=}"
      INSTALL_DIR="$PREFIX/bin"
      shift
      ;;
    --help)
      echo "Usage: $0 [--prefix=<installation prefix>]"
      echo ""
      echo "Options:"
      echo "  --prefix=<path>    Install to <path>/bin instead of /usr/local/bin"
      echo "  --help             Show this help message"
      exit 0
      ;;
    *)
      echo "Unknown option: $1"
      echo "Use --help for usage information"
      exit 1
      ;;
  esac
done

# Check if the binary exists
BINARY="target/x86_64-unknown-linux-musl/release/disp"
if [ ! -f "$BINARY" ]; then
  echo "Error: Binary not found at $BINARY"
  echo "Please run this script from the project root directory after building with:"
  echo "  cargo build --release --target x86_64-unknown-linux-musl"
  exit 1
fi

# Create installation directory if it doesn't exist
mkdir -p "$INSTALL_DIR"
if [ $? -ne 0 ]; then
  echo "Error: Failed to create installation directory $INSTALL_DIR"
  echo "You may need to run this script with sudo"
  exit 1
fi

# Copy the binary
cp "$BINARY" "$INSTALL_DIR/"
if [ $? -ne 0 ]; then
  echo "Error: Failed to copy binary to $INSTALL_DIR"
  echo "You may need to run this script with sudo"
  exit 1
fi

# Make it executable
chmod +x "$INSTALL_DIR/disp"
if [ $? -ne 0 ]; then
  echo "Error: Failed to make binary executable"
  exit 1
fi

echo "Installation successful!"
echo "The disp binary has been installed to $INSTALL_DIR/disp"
echo ""
echo "You can now run disp with:"
echo "  disp --help"
echo ""
echo "To create a configuration file, run:"
echo "  disp create-config --output /path/to/config.toml"
echo ""
echo "To apply the best matching profile (or a specific one with --profile), run:"
echo "  disp apply --config /path/to/config.toml [--profile <profile-name>]"
