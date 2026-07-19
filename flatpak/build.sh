#!/bin/sh

app_id="com.stremio.StremioNative.Devel"
cwd="flatpak"

# Generate cargo dependencies sources list for offline flatpak builds
python3 $cwd/flatpak-builder-tools/cargo/flatpak-cargo-generator.py Cargo.lock -o $cwd/cargo-sources.json

# Build the flatpak
flatpak-builder --repo=$cwd/repo --force-clean $cwd/build $cwd/$app_id.json

# Create the flatpak bundle
flatpak build-bundle $cwd/repo $cwd/$app_id.flatpak $app_id
