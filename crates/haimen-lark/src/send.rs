use crate::bridge::LarkCliBridge;

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
