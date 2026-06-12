use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum Provider {
    Openai,
    Gemini,
}


#[derive(Parser, Debug)]
#[command(name="wp_poster", about="Generate articles with OpenAI and save drafts to WordPress")]
pub struct Cli {
    #[arg(long)]
    pub prompt_file: Option<PathBuf>,
    #[arg(long)]
    pub model: Option<String>,
    #[arg(long, default_value_t=100)]
    pub max_concurrency: usize,
    #[arg(long, default_value_t=false)]
    pub dry_run: bool,
    #[arg(long)]
    pub outdir: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = Provider::Openai)]
    pub provider: Provider,
    /// Use a news-style prompt for the initial draft generation only.
    #[arg(long, default_value_t=false)]
    pub news: bool,
}