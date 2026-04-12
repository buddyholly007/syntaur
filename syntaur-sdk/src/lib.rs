//! Syntaur SDK — the public interface for building Syntaur modules.
//!
//! This crate defines the traits and types that module authors implement
//! to extend Syntaur with new tools, services, and capabilities.
//!
//! # Module tiers
//!
//! - **Core modules** are compiled into the gateway binary via Cargo feature
//!   flags. They implement [`ModuleTool`] and register via the `inventory`
//!   crate for zero-boilerplate discovery.
//!
//! - **Extension modules** are separate binaries that communicate with the
//!   gateway via the MCP (Model Context Protocol) over stdio. They ship
//!   a `syntaur.module.toml` manifest and are managed by `syntaur-mod`.

pub mod types;
pub mod capabilities;
pub mod tool;
pub mod module;
pub mod manifest;

// Re-export the primary public API at crate root.
pub use capabilities::ToolCapabilities;
pub use manifest::ModuleManifest;
pub use module::{Module, ModuleContext, ModuleHandle, ServiceHandle};
pub use tool::{ModuleTool, ModuleToolContext};
pub use types::{Artifact, Citation, RichToolResult};
