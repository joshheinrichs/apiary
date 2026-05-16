use std::ffi::OsString;

use clap::Parser;
use bubblewand::SandboxArgs;

#[derive(Parser)]
#[command(name = "bubblewand", about = "Run a program in a bubblewrap sandbox")]
struct Cli {
    #[command(flatten)]
    sandbox: SandboxArgs,

    /// The executable and its arguments
    #[arg(last = true, required = true)]
    command: Vec<OsString>,
}

fn main() {
    let cli = Cli::parse();

    let Some(exe) = cli.command.first() else {
        eprintln!("bubblewand: no executable specified");
        std::process::exit(1);
    };

    let err = bubblewand::run_sandbox(
        &cli.sandbox,
        exe.as_ref(),
        &cli.command[1..],
    );

    eprintln!("bubblewand: exec failed: {}", err);
    std::process::exit(1);
}
