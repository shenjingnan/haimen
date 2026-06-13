use clap::Parser;
use haimen::cli::{self, Cli};

#[tokio::main]
async fn main() {
    haimen::logging::init_logging();

    let cli = Cli::parse();
    let result = cli::run(cli).await;

    if let Err(err) = result {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}
