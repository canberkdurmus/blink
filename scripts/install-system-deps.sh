#!/usr/bin/env bash
# System packages for building and testing Blink on Debian/Ubuntu (needs root).
# Usage: sudo scripts/install-system-deps.sh
set -euo pipefail
apt-get update
apt-get install -y --no-install-recommends \
  build-essential pkg-config git curl ca-certificates tmux gh \
  libimage-exiftool-perl \
  libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev \
  libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libxcb1-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
  libgl1-mesa-dev libegl1-mesa-dev libvulkan-dev libvulkan1 mesa-vulkan-drivers \
  libgtk-3-dev libdbus-1-dev libfontconfig1-dev libssl-dev \
  xvfb
