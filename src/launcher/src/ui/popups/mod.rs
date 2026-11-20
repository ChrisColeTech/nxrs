//! The authored child window (the family's `28`) and the scaffold it draws: every dialog of the launcher
//! is a modal child window: a real top-level OS window with no native frame, the shared title header
//! with its close button, edges to resize it by, a scrim over the main window while it is open.
pub mod authored;
pub mod child_bounds;
pub mod child_draw;
pub mod child_grip;
pub mod child_scrim;
pub mod child_spec;
pub mod child_window;
pub mod held;
