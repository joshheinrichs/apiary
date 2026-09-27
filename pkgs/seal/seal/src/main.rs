use std::ffi::OsString;

use clap::Parser;
use seal::SandboxArgs;

#[derive(Parser)]
#[command(name = "seal", about = "Run a program in a bubblewrap sandbox")]
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
        eprintln!("seal: no executable specified");
        std::process::exit(1);
    };

    let err = seal::run_sandbox(&cli.sandbox, exe.as_ref(), &cli.command[1..]);

    eprintln!("seal: exec failed: {}", err);
    std::process::exit(1);
}
