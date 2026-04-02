use cat_llm::{Cli, Command, run_cat, run_extract};
use clap::Parser;

fn main() {
    let cli = Cli::parse();
    let mut stdout = std::io::stdout();

    let result = match cli.command {
        Some(Command::Extract(args)) => {
            run_extract(&args, &mut stdout).map(|(written, skipped)| {
                eprintln!("{} file(s) written, {} skipped.", written, skipped);
            })
        }
        Some(Command::Cat(args)) => run_cat(&args, &mut stdout),
        None => run_cat(&cli.cat_args, &mut stdout),
    };

    if let Err(err) = result {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }
}
