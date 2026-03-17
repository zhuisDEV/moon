mod assets;
mod cli;
mod commands;
mod env_loader;
mod error;
mod logging;
mod moon;
mod openclaw;

fn main() {
    if let Err(err) = env_loader::load_dotenv() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }

    if let Err(err) = cli::run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
