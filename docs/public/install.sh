#!/bin/sh
# haimen 一键安装脚本
# 用法: curl -fsSL https://haimen.dev/install.sh | sh
#
# 注意：此脚本将自动转向 cargo-dist 生成的官方安装器，
# cargo-dist 安装器会自动检测平台、下载二进制归档、验证完整性并配置 PATH。
set -eu

REPO='shenjingnan/haimen'

# 使用 GitHub Releases 上的 cargo-dist 安装器（始终获取最新版本）
INSTALLER_URL="https://github.com/${REPO}/releases/latest/download/haimen-installer.sh"

echo "⬇️  正在下载 haimen 安装器..."
echo ""

if command -v curl > /dev/null 2>&1; then
  curl -fsSL "$INSTALLER_URL" | sh
elif command -v wget > /dev/null 2>&1; then
  wget -qO- "$INSTALLER_URL" | sh
else
  echo "❌ 错误: 未找到 curl 或 wget，请先安装其中之一"
  echo ""
  echo "   或者你可以直接使用 cargo 安装:"
  echo "   cargo install haimen"
  exit 1
fi
