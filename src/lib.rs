//! AnyEXE: run Windows executables as native Linux processes.
//!
//! No emulation and no separate runtime process: a PE image is mapped into the process,
//! its imports are bound to Rust reimplementations of the Win32 and C runtime libraries
//! that use the Windows x64 calling convention, and the entry point is called directly.

#![allow(clippy::missing_safety_doc)]
#![allow(unsafe_op_in_unsafe_fn)]

pub mod analyze;
pub mod launch;
pub mod loader;
pub mod pe;
pub mod registry;
pub mod runtime;
pub mod shim;
pub mod win;
