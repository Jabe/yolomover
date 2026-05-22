use clap::Parser;
use yolomover::cli::Cli;
use yolomover::{execute, init_logging};

fn main() {
    let cli = Cli::parse();
    init_logging(cli.log_level);
    if let Err(e) = execute(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
