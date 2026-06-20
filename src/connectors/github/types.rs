use serde::Deserialize;

/// GitHub IssueComment 事件（从 webhook payload 反序列化）
#[derive(Debug, Clone, Deserialize)]
pub struct IssueCommentEvent {
    pub action: String,
    pub issue: Issue,
    pub comment: Comment,
    pub repository: Option<Repository>,
}

/// Issue 信息
#[derive(Debug, Clone, Deserialize)]
pub struct Issue {
    pub number: i64,
    pub title: String,
    pub body: Option<String>,
    /// API URL 用于获取/创建评论
    pub comments_url: String,
}

/// 评论信息
#[derive(Debug, Clone, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub body: Option<String>,
    pub user: Option<User>,
}

/// GitHub 用户
#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub login: String,
    pub id: i64,
}

/// 仓库信息
#[derive(Debug, Clone, Deserialize)]
pub struct Repository {
    pub full_name: String,
    pub html_url: String,
}
