#!/bin/bash
# run-security-audit.sh - 运行安全审计，捕获输出，始终返回 0
# 被 security-audit skill 的 !`bash ...` 调用，将结果注入 LLM 上下文

run_check() {
  local name="$1"
  shift
  echo "---[${name}]---"
  "$@" 2>&1 || true
  echo ""
}

run_check "cargo audit" cargo audit 2>/dev/null || echo "提示: 请安装 cargo audit (cargo install cargo-audit)"
run_check "cargo deny check" cargo deny check 2>/dev/null || echo "提示: 请安装 cargo deny (cargo install cargo-deny)"
