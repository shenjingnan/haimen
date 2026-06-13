/// 日志初始化模块
///
/// 初始化双层日志系统：
/// - 文件日志: 写入 `~/.ai-rust-starter/logs/app.log`（info 级别以上，无 ANSI 颜色）
/// - stderr 日志: 受 `RUST_LOG` 环境变量控制（默认 warn 级别以上）
use std::fs::OpenOptions;
use std::io;
use std::path::PathBuf;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, Registry, fmt};

/// 初始化日志系统
///
/// 使用 `try_init()` 而非 `init()`，以便在测试中多次调用不会 panic。
pub fn init_logging() {
    let log_path = get_log_path();

    // 确保日志目录存在
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    // 文件日志层 — 记录 info+，无 ANSI 颜色
    let file_layer = fmt::layer()
        .with_writer(make_file_writer(log_path.clone()))
        .with_ansi(false)
        .with_target(true)
        .with_filter(EnvFilter::new("info"));

    // stderr 日志层 — 受 RUST_LOG 控制，默认 warn
    let stderr_layer = fmt::layer()
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")));

    let _ = Registry::default()
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
}

/// 创建文件日志 writer（可测试）
fn make_file_writer(log_path: PathBuf) -> impl Fn() -> Box<dyn io::Write> {
    move || -> Box<dyn io::Write> {
        // 确保目录存在（可能被外部删除）
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(file) => Box::new(file),
            Err(_) => Box::new(io::sink()),
        }
    }
}

/// 获取日志文件路径
fn get_log_path() -> PathBuf {
    get_log_dir().join("app.log")
}

/// 获取日志目录路径
fn get_log_dir() -> PathBuf {
    crate::config::settings::get_home_dir().join(".ai-rust-starter/logs")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::run_with_temp_home;
    use std::io::Write;

    #[test]
    fn test_log_path_uses_project_dir() {
        run_with_temp_home(|_home| {
            let log_path = get_log_path();
            assert!(log_path.starts_with(_home.join(".ai-rust-starter/logs")));
            assert_eq!(
                log_path.file_name().unwrap(),
                "app.log",
                "日志文件名应为 app.log"
            );
        });
    }

    #[test]
    fn test_log_dir_is_created() {
        run_with_temp_home(|_home| {
            let log_dir = get_log_dir();
            assert!(!log_dir.exists(), "日志目录应在 init 时创建");

            std::fs::create_dir_all(&log_dir).unwrap();
            assert!(log_dir.exists());
        });
    }

    #[test]
    fn test_file_writer_appends() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");
            std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();

            let mut writer = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            writeln!(writer, "line 1").unwrap();
            drop(writer);

            let mut writer = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            writeln!(writer, "line 2").unwrap();
            drop(writer);

            let content = std::fs::read_to_string(&log_path).unwrap();
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 2);
            assert_eq!(lines[0], "line 1");
            assert_eq!(lines[1], "line 2");
        });
    }

    #[test]
    fn test_make_file_writer_writes_content() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");
            let writer = make_file_writer(log_path.clone());

            {
                let mut w = writer();
                writeln!(w, "hello world").unwrap();
            }

            let content = std::fs::read_to_string(&log_path).unwrap();
            assert!(content.contains("hello world"), "writer 应写入文件内容");
        });
    }

    #[test]
    fn test_make_file_writer_sink_on_error() {
        let writer = make_file_writer(PathBuf::from("/nonexistent/path/app.log"));
        let mut w = writer();
        let result = writeln!(w, "should not panic");
        assert!(result.is_ok(), "回退到 io::sink 应始终成功");
    }

    #[test]
    fn test_init_logging_creates_directory() {
        run_with_temp_home(|home| {
            let log_dir = home.join(".ai-rust-starter/logs");
            assert!(!log_dir.exists(), "测试前日志目录不应存在");
            init_logging();
            assert!(log_dir.exists(), "init_logging 应创建日志目录");
        });
    }

    #[test]
    fn test_init_logging_does_not_panic() {
        run_with_temp_home(|_home| {
            init_logging();
        });
    }

    #[test]
    fn test_tracing_events_written_to_file() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");

            let subscriber = Registry::default().with(
                fmt::layer()
                    .with_writer(make_file_writer(log_path.clone()))
                    .with_ansi(false),
            );

            tracing::subscriber::with_default(subscriber, || {
                tracing::info!("test info message");
                tracing::warn!("test warn message");
                tracing::error!("test error message");
            });

            let content = std::fs::read_to_string(&log_path).expect("日志文件应被创建");
            assert!(
                content.contains("test info message"),
                "日志文件应包含 info 消息"
            );
            assert!(
                content.contains("test warn message"),
                "日志文件应包含 warn 消息"
            );
            assert!(
                content.contains("test error message"),
                "日志文件应包含 error 消息"
            );
        });
    }

    #[test]
    fn test_tracing_events_no_ansi() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");

            let subscriber = Registry::default().with(
                fmt::layer()
                    .with_writer(make_file_writer(log_path.clone()))
                    .with_ansi(false),
            );

            tracing::subscriber::with_default(subscriber, || {
                tracing::info!("plain text message");
            });

            let content = std::fs::read_to_string(&log_path).unwrap();
            assert!(!content.contains('\x1b'), "日志文件不应包含 ANSI 转义字符");
        });
    }

    #[test]
    fn test_tracing_events_filtered_by_level() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");

            let subscriber = Registry::default().with(
                fmt::layer()
                    .with_writer(make_file_writer(log_path.clone()))
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("info")),
            );

            tracing::subscriber::with_default(subscriber, || {
                tracing::debug!("debug message - should not appear");
                tracing::info!("info message - should appear");
                tracing::warn!("warn message - should appear");
            });

            let content = std::fs::read_to_string(&log_path).unwrap();
            assert!(content.contains("info message"), "info 级别应通过过滤");
            assert!(content.contains("warn message"), "warn 级别应通过过滤");
            assert!(!content.contains("debug message"), "debug 级别应被过滤掉");
        });
    }

    #[test]
    fn test_make_file_writer_multiple_calls_produce_unique_writers() {
        run_with_temp_home(|home| {
            let log_path = home.join(".ai-rust-starter/logs/app.log");
            let writer = make_file_writer(log_path.clone());

            {
                let mut w1 = writer();
                writeln!(w1, "from first writer").unwrap();
            }
            {
                let mut w2 = writer();
                writeln!(w2, "from second writer").unwrap();
            }

            let content = std::fs::read_to_string(&log_path).unwrap();
            let lines: Vec<&str> = content.lines().collect();
            assert_eq!(lines.len(), 2, "两个写入器的内容都应存在");
            assert_eq!(lines[0], "from first writer");
            assert_eq!(lines[1], "from second writer");
        });
    }
}
