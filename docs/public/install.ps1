# haimen 一键安装脚本（PowerShell）
# 从 GitHub Releases 下载并执行 cargo-dist 生成的安装器
$Repo = 'shenjingnan/haimen'
$InstallerUrl = "https://github.com/${Repo}/releases/latest/download/installer.ps1"

Write-Host "⬇️  正在下载 haimen 安装器..." -ForegroundColor Cyan
$TempFile = "$env:TEMP\haimen-installer.ps1"
Invoke-WebRequest -Uri $InstallerUrl -OutFile $TempFile
& $TempFile
