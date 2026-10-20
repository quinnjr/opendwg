//! Pure-Rust reader and writer for AutoCAD DWG files: reads AC1015-AC1032
//! (AutoCAD 2000-2018) into [`Drawing`], writes AC1032. See `README.md` for
//! an overview.
//!
//! With the `serde` feature, [`Drawing`] and the public model types
//! implement `Serialize` and `Deserialize`. Values kept only to write
//! unchanged objects back bit for bit, the reference index and the state
//! the writer keeps from the file read are skipped. [`Error`] serializes
//! (only) as `{ kind, message, handles }`, so a Tauri command can return it;
//! the codec tools ([`bits::BitReader`], [`bits::BitWriter`],
//! [`object::ObjReader`], [`object::ObjWriter`], [`object::DecodeCtx`]) are
//! not serializable.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

// The README's examples compile as doc tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub mod bits;
pub mod codec;
pub mod container;
pub mod diag;
pub mod drawing;
pub mod error;
pub mod handle;
pub mod object;
pub mod proxy_graphics;
pub mod section;
pub mod strings;
pub mod version;

pub use bits::Color;
pub use diag::{Diagnostic, Severity};
pub use drawing::Drawing;
pub use error::{Error, Result};
pub use handle::{Handle, HandleRef};
pub use version::Version;
