use ai_rust_starter::cli::{self, Cli};
use clap::Parser;

#[tokio::main]
async fn main() {
    ai_rust_starter::logging::init_logging();

    let cli = Cli::parse();
    let result = cli::run(cli).await;

    if let Err(err) = result {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}
