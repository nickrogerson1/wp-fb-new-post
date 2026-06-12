# WP Poster — AI Content Pipeline for WordPress + Social

A Rust CLI (`wp_poster`) that turns a list of topics in a Google Sheet into
fully-researched, source-linked WordPress draft posts plus ready-to-use social
media assets — using either OpenAI or Gemini as the content provider.

## Pipeline

For each pending topic, the tool runs a multi-stage pipeline behind a common
`ContentProvider` trait (implemented separately for OpenAI and Gemini, so the
provider can be swapped with a CLI flag):

1. **Research & draft** — generates a full draft article (title, excerpt, HTML
   content, tags, slug) along with a list of source references, using the
   model's web-research/browsing capability. Seed links from the sheet (the
   configurable seed-links column) are passed in as a starting point. An
   `--news` flag switches to a news-style prompt for time-sensitive topics.
2. **Linkify** — rewrites the draft, turning the cited sources into inline
   links within the article body.
3. **Social assets** — generates a Facebook post snippet and a list of
   suggested image URLs for the article.
4. **Tagging** — normalizes the suggested tags and resolves them to WordPress
   tag IDs via a `TagManager`, creating any tags that don't already exist
   (with an in-memory cache bootstrapped from the site on startup).
5. **Slug generation** — derives a short, URL-safe slug from the article title.
6. **Publish** — posts the article to WordPress as a **draft** via the REST
   API (`/wp-json/wp/v2/posts`), with retries/backoff on rate limits and server
   errors.

Topics are processed concurrently (configurable concurrency limit). After the
run, all generated social assets are written to `social_assets.xlsx` **and**
pushed back into the Google Sheet, and a token-usage/cost summary is printed.

## Google Sheet layout

The sheet column layout is configurable via `.env` (see Setup below):

- **`SHEET_TOPIC_COLUMN`** — the topic/prompt text (a row is processed if this
  is filled in).
- **`SHEET_PROCESSED_COLUMN`** — "already processed" flag; non-empty values
  skip the row.
- **`SHEET_SEED_LINKS_COLUMN`** — optional seed links (one or more reference
  URLs) to ground the research step.
- **`SHEET_OUTPUT_COLUMN`** — first column of the social asset write-back
  block (slug, title, tags, Facebook snippet, image URLs, model), written
  after processing.

## CLI options

```
wp_poster [OPTIONS]

--prompt-file <PATH>     Optional custom prompt file
--model <MODEL>          Override the model (defaults: gpt-5.2 for OpenAI,
                          gemini-3-pro-preview for Gemini)
--max-concurrency <N>    Topics processed in parallel (default: 100)
--dry-run                Generate articles but don't post to WordPress
--outdir <DIR>           Write each generated article as JSON to this folder
--provider <openai|gemini>  Which AI provider to use (default: openai)
--news                   Use a news-style prompt for the initial draft
```

## Setup

Create a `.env` file in the project root:

```env
# Content provider — set whichever matches --provider
OPENAI_API_KEY=...
GEMINI_API_KEY=...

# Google Sheet (topics + results)
GOOGLE_SHEET_ID=your-spreadsheet-id
GOOGLE_SHEET_TAB=Sheet1
GOOGLE_APPLICATION_CREDENTIALS=service_account.json

# Google Sheet column layout (see "Google Sheet layout" above)
SHEET_TOPIC_COLUMN=F
SHEET_PROCESSED_COLUMN=K
SHEET_SEED_LINKS_COLUMN=P
SHEET_OUTPUT_COLUMN=J

# Target WordPress site
KYI_WP_URL=https://your-site.example.com
KYI_WP_USER=your-wp-username
KYI_WP_APP_PASSWORD=your-wp-application-password

# Optional
GOOGLE_IMAGE_URL_LIMIT=5          # max image suggestions per article (default 5)
OPENAI_PRICE_INPUT_PER_1M=...     # for cost estimates
OPENAI_PRICE_OUTPUT_PER_1M=...
OPENAI_WEB_SEARCH_PRICE_PER_1K=...
GEMINI_PRICE_INPUT_PER_1M=...
GEMINI_PRICE_OUTPUT_PER_1M=...
GEMINI_WEB_SEARCH_PRICE_PER_1K=...
```

Google authentication uses Application Default Credentials via `gcp_auth` —
point `GOOGLE_APPLICATION_CREDENTIALS` at a service account JSON key with
access to the target sheet.

`.env`, `service_account.json`, and `social_assets/` are git-ignored.

## Usage

```bash
cargo run --release -- --provider openai --max-concurrency 5 --dry-run
```

Remove `--dry-run` once you're happy with the output to publish drafts to
WordPress.

## Tech stack

Rust, Tokio, `clap`, `reqwest`, `gcp_auth`, `scraper`, `html2text`,
`rust_xlsxwriter`, `async-trait`, `thiserror`.
