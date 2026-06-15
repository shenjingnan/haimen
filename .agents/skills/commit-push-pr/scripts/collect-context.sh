#!/bin/bash
# 收集 git 状态和模型信息，为 commit-push-pr skill 提供上下文
# 被 commit-push-pr skill 的 !`bash ...` 调用，将结果注入 LLM 上下文

run_check() {
  local name="$1"
  shift
  echo "---[${name}]---"
  "$@" 2>&1 || true
  echo ""
}

run_check "git status" git status
run_check "git diff HEAD" git diff HEAD
run_check "当前分支" git branch --show-current
run_check "当前模型" jq -r '.env.ANTHROPIC_MODEL' ~/.claude/settings.json
