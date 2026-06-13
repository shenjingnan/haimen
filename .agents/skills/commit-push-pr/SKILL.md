---
name: commit-push-pr
description: 提交、推送并创建 PR
---

## 上下文获取

以下命令将在技能加载时自动执行，结果将注入到上下文中供分析：

- 全部上下文: !`bash .agents/skills/commit-push-pr/scripts/collect-context.sh`

## 你的任务

根据上述变更：

1. 如果当前在 main 分支，则创建一个新分支
2. 使用中文创建一个包含合适提交信息的 commit，**commit body 中必须包含 ## Attribution 中指定的
   attribution 信息**
3. 将分支推送到 origin
4. 使用 `gh pr create` 创建 Pull Request，**PR 描述中必须包含 ## Attribution 中指定的 attribution
   信息**
5. 你可以在单次响应中调用多个工具。你必须在单条消息中完成上述所有操作。不要使用任何其他工具或执行任何其他操作。除了工具调用之外，不要发送任何其他文本或消息。

## Commit Message 格式要求

**语言要求：Commit subject、body 和 PR 描述均必须使用中文编写。**

Commit message **必须**以以下格式结尾：

```
<commit subject 和 body>

🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: <模型名称> <noreply@<对应主域名>>
```

## Attribution 信息

每次 commit 的 body 和 PR 描述中必须附加以下 attribution 信息：

```
🤖 Generated with [Claude Code](https://claude.com/claude-code)

Co-Authored-By: <当前模型>
```
