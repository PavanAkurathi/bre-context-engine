#!/bin/sh
# BRE Context Engine One-Line Installer
set -e

# Target paths
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="bre-context-engine"

echo "BRE: Detecting system architecture..."
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Darwin)
        PLATFORM="apple-darwin"
        ;;
    Linux)
        PLATFORM="unknown-linux-gnu"
        ;;
    *)
        echo "BRE: Unsupported operating system: $OS"
        exit 1
        ;;
esac

case "$ARCH" in
    x86_64)
        TARGET_ARCH="x86_64"
        ;;
    arm64|aarch64)
        TARGET_ARCH="aarch64"
        ;;
    *)
        echo "BRE: Unsupported architecture: $ARCH"
        exit 1
        ;;
esac

# Release URL mapping (Assumes GitHub release artifacts match architecture)
RELEASE_TAG="v1.0.0"
DOWNLOAD_URL="https://github.com/private-owner/bre-context-engine/releases/download/${RELEASE_TAG}/${BINARY_NAME}-${TARGET_ARCH}-${PLATFORM}.tar.gz"

echo "BRE: Downloading pre-compiled release from GitHub..."
TEMP_DIR="$(mktemp -d)"
curl -fsSL "$DOWNLOAD_URL" -o "${TEMP_DIR}/${BINARY_NAME}.tar.gz"

echo "BRE: Extracting binary..."
tar -xzf "${TEMP_DIR}/${BINARY_NAME}.tar.gz" -C "$TEMP_DIR"

echo "BRE: Moving binary to ${INSTALL_DIR}/${BINARY_NAME}..."
if [ -w "$INSTALL_DIR" ]; then
    mv "${TEMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
else
    sudo mv "${TEMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
fi

chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
rm -rf "$TEMP_DIR"

echo "BRE: Installation complete! Run 'bre-context-engine --version' or 'bre-context-engine --mcp' to begin."
