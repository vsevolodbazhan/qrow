#!/bin/sh
# Install the Ubuntu libraries that the Qrow application builds and links
# against. Native build jobs and the common headless UI job run this script.
set -eu
sudo apt-get update
sudo apt-get install --yes \
    libfontconfig1-dev \
    libvulkan-dev \
    libwayland-dev \
    libx11-dev \
    libxcb-shape0-dev \
    libxcb-xfixes0-dev \
    libxcb1-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-dev
