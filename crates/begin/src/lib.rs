//! The begin front-end library: the classic ANSI console UI, the command
//! parser, and the resumable [`session::Session`] that drives single-player
//! identically on the native console and in the browser (begin-web).

pub mod commands;
pub mod fighters;
pub mod session;
pub mod ui;
