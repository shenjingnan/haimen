# haimen 国内镜像一键安装脚本（PowerShell）
# 通过 ghproxy.com 代理下载 GitHub Releases 以加速国内访问
$Repo = 'shenjingnan/haimen'
$ProxyBase = 'https://ghproxy.com/https://github.com'
$InstallerUrl = "${ProxyBase}/${Repo}/releases/latest/download/haimen-installer.ps1"

Write-Host "⬇️  正在通过国内镜像下载 haimen 安装器..." -ForegroundColor Cyan

# 设置代理环境变量，让 cargo-dist 安装器也走 ghproxy.com
$env:HAIMEN_INSTALLER_GITHUB_BASE_URL = $ProxyBase

$TempFile = "$env:TEMP\haimen-installer.ps1"
Invoke-WebRequest -Uri $InstallerUrl -OutFile $TempFile
& $TempFile
