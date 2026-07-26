#!/bin/sh
# haimen 一键安装脚本
# 从 GitHub Releases 下载并执行 cargo-dist 生成的安装器
set -eu

REPO='shenjingnan/haimen'
INSTALLER_URL="https://github.com/${REPO}/releases/latest/download/installer.sh"

echo "⬇️  正在下载 haimen 安装器..."
curl -fsSL "$INSTALLER_URL" | sh
