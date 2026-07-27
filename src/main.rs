use std::process::ExitCode;

use carnet::{
    catalog::Catalog,
    cli::{Cli, route},
};
use clap::Parser;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let catalog = match Catalog::load() {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("carnet: {error}");
            return ExitCode::from(2);
        }
    };
    match route(cli, &catalog) {
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("carnet: {error}");
            ExitCode::from(2)
        }
    }
}
