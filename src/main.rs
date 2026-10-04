mod archive;
mod cli;
mod config;
mod deb;
mod desktop;
mod doctor;
mod fsutil;
mod history;
mod inspect;
mod install;
mod naming;
mod paths;
mod portable;
mod probe;
mod state;
mod tui;
mod update;
mod util;

use std::process::exit;
use crate::cli::run;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        exit(1);
    }
}
