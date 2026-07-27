use clap::Parser;
use haimen::cli::{self, Cli, Commands};

#[tokio::main]
async fn main() {
    // 先解析 CLI，以便提取日志级别后再初始化日志
    let cli = Cli::parse();

    // 根据命令和 --log-level 参数决定终端日志级别
    // - start 命令默认不输出到终端（仅文件日志）
    // - start --log-level <LEVEL> 按指定级别输出到终端
    // - 其他命令保持原行为（使用 RUST_LOG 或默认 warn）
    let terminal_log_level: Option<&str> = match &cli.command {
        Some(Commands::Start { log_level, .. }) => match log_level {
            Some(level) if !level.is_empty() => Some(level.as_str()),
            _ => Some("off"),
        },
        _ => None,
    };

    haimen::logging::init_logging(terminal_log_level);

    let result = cli::run(cli).await;

    if let Err(err) = result {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}
