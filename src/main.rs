use clap::Parser;

fn main() {
    let cli = seeds::Cli::parse();
    let (code, message) = seeds::run(&cli);
    eprintln!("{message}");
    std::process::exit(code);
}
