use cat_llm::{Args, run};
use clap::Parser;

fn main() {
    let args = Args::parse();
    let mut stdout = std::io::stdout();

    if let Err(err) = run(&args, &mut stdout) {
        eprintln!("Error executing cat_llm: {}", err);
        std::process::exit(1);
    }
}
