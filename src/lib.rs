//! LLM-as-DOM: AI browser pilot with cheap LLM + heuristics.
//!
//! A headless browser pilot that compresses web pages to ~100-300 tokens
//! and uses heuristics + a cheap LLM to accomplish goals autonomously.

pub mod a11y;
pub mod backend;
pub mod error;
pub mod heuristics;
pub mod pilot;
pub mod semantic;

pub use error::Error;
