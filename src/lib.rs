//! LLM-as-DOM: AI browser pilot with cheap LLM + heuristics.
//!
//! A headless browser pilot that compresses web pages to ~100-300 tokens
//! and uses heuristics + a cheap LLM to accomplish goals autonomously.

pub mod a11y;
pub mod audit;
pub mod backend;
pub mod crypto;
pub mod engine;
pub mod error;
pub mod heuristics;
pub mod intent;
pub mod locate;
pub mod network;
pub mod oauth;
pub mod observer;
pub mod pilot;
pub mod playbook;
pub mod profile;
pub mod sanitize;
pub mod selector;
pub mod semantic;
pub mod session;
pub mod watch;

pub use error::Error;
