#!/usr/bin/env bash
# Install cctui-agent (cctui-shim) on this machine
set -euo pipefail

CCTUI_URL="${CCTUI_URL:-https://cctui.dorsk.dev}"
CCTUI_TOKEN="${CCTUI_TOKEN:-}"
INSTALL_DIR="${HOME}/.cctui/bin"
BINARY_URL="${CCTUI_URL}/api/v1/agent/cctui-agent"

mkdir -p "${INSTALL_DIR}"

if command -v curl &>/dev/null; then
    curl -fsSL -H "Authorization: Bearer ${CCTUI_TOKEN}" -o "${INSTALL_DIR}/cctui-agent" "${BINARY_URL}"
elif command -v wget &>/dev/null; then
    wget -q --header="Authorization: Bearer ${CCTUI_TOKEN}" -O "${INSTALL_DIR}/cctui-agent" "${BINARY_URL}"
else
    echo "Error: curl or wget required" >&2
    exit 1
fi

chmod +x "${INSTALL_DIR}/cctui-agent"
echo "cctui-agent installed to ${INSTALL_DIR}/cctui-agent"
echo "Add ${INSTALL_DIR} to PATH or configure hooks with full path."
