#!/bin/bash
set -e

# Target directory for rootfs
ROOTFS_DIR="/home/rustam/dist/helios/rootfs"
TARBALL="alpine-minirootfs-3.19.1-x86_64.tar.gz"
URL="https://dl-cdn.alpinelinux.org/alpine/v3.19/releases/x86_64/$TARBALL"

echo "==============================================================="
# Create and navigate to rootfs directory
mkdir -p "$ROOTFS_DIR"
cd "$ROOTFS_DIR"

if [ -f "bin/sh" ]; then
    echo "Rootfs already structured and exists at $ROOTFS_DIR."
    echo "Skipping download."
    exit 0
fi

echo "Downloading Alpine Mini Rootfs from:"
echo "$URL"
echo "---------------------------------------------------------------"

# Try curl first, fallback to wget
if command -v curl >/dev/null 2>&1; then
    curl -L -O "$URL"
elif command -v wget >/dev/null 2>&1; then
    wget "$URL"
else
    echo "Error: Neither curl nor wget is installed on the host. Cannot download rootfs."
    exit 1
fi

echo "---------------------------------------------------------------"
echo "Extracting rootfs tarball..."
tar -xzf "$TARBALL"

echo "Cleaning up tarball..."
rm -f "$TARBALL"

# Ensure standard device files exist as place-holders
mkdir -p dev
touch dev/null dev/zero dev/urandom dev/random dev/tty

echo "Rootfs successfully initialized at:"
echo "$ROOTFS_DIR"
echo "==============================================================="
