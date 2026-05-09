mod auth;
mod cli;
mod tls;
mod upload;

use clap::Parser;

fn main() {
    let _cli = cli::Cli::parse();
    println!("flowcase_upload_server v0");
}
