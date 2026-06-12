mod app;
mod cli;
mod config;
mod errors;
mod gemini;
mod google;
mod io_utils;
mod models;
mod openai;
mod providers;
mod utils;
mod wordpress;
mod wp_tags;
mod workflow;

use app::run;
use dotenvy::dotenv;

#[tokio::main]
async fn main() {
    dotenv().ok();

    if let Err(err) = run().await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}