use super::bridge::LarkCliBridge;

/// 发送文本消息到飞书聊天
pub async fn send_text(bridge: &LarkCliBridge, chat_id: &str, text: &str) -> Result<(), String> {
    bridge
        .exec(&[
            "im",
            "+messages-send",
            "--as",
            "bot",
            "--chat-id",
            chat_id,
            "--text",
            text,
        ])
        .await?;
    Ok(())
}

/// 发送 Markdown 消息到飞书聊天
pub async fn send_markdown(
    bridge: &LarkCliBridge,
    chat_id: &str,
    markdown: &str,
) -> Result<(), String> {
    bridge
        .exec(&[
            "im",
            "+messages-send",
            "--as",
            "bot",
            "--chat-id",
            chat_id,
            "--markdown",
            markdown,
        ])
        .await?;
    Ok(())
}
