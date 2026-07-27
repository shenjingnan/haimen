#!/bin/sh
# haimen 国内镜像一键安装脚本（Gitee）
# 通过 ghproxy.com 代理下载 GitHub Releases 以加速国内访问
set -eu

echo "⬇️  正在通过国内镜像下载 haimen 安装器..."

# 使用 ghproxy.com 代理 GitHub Releases 以加速国内访问
HAIMEN_INSTALLER_GITHUB_BASE_URL="https://ghproxy.com/https://github.com" \
  curl -fsSL "https://ghproxy.com/https://github.com/shenjingnan/haimen/releases/latest/download/install.sh" | sh
