#!/bin/bash
# run-checks.sh - 运行所有代码质量检查，捕获输出，始终返回 0
# 被 check skill 的 !`bash ...` 调用，将结果注入 LLM 上下文

run_check() {
  local name="$1"
  shift
  echo "---[${name}]---"
  "$@" 2>&1 || true
  echo ""
}

run_check "cargo fmt --check" cargo fmt --check
run_check "cargo clippy" cargo clippy -- -D warnings
run_check "cargo test" cargo test -- --test-threads=1
run_check "typos" typos .
run_check "cargo build" cargo build
