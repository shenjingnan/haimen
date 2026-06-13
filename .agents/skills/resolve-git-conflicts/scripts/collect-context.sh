#!/bin/bash
# collect-context.sh - 收集 Git 冲突上下文信息，始终返回 0
# 被 resolve-git-conflicts skill 的 !`bash ...` 调用，将结果注入 LLM 上下文

run_check() {
  local name="$1"
  shift
  echo "---[${name}]---"
  "$@" 2>&1 || true
  echo ""
}

run_check "conflicted files" git --no-pager diff --name-only --diff-filter=U
run_check "unmerged entries" git --no-pager ls-files --unmerged
run_check "repository status" git --no-pager status
run_check "current branch" git branch --show-current
run_check "conflict file count" bash -c 'git --no-pager diff --name-only --diff-filter=U 2>/dev/null | wc -l'

echo "---[conflict blocks per file]---"
for f in $(git --no-pager diff --name-only --diff-filter=U 2>/dev/null); do
  [ -z "$f" ] && continue
  count=$(grep -c "^<<<<<<<" "$f" 2>/dev/null || echo 0)
  echo "  $f: ${count} conflict blocks"
done
echo ""
