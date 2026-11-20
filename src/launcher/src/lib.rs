//! The nxrs launcher: the game library, builds through nxrs-build (launcher contract v2, a child
//! process), Play. `constants` holds every size and sentence, `model` the rules (no I/O), `services`
//! all disk, process and OS work, `flows` what each command does, `ui` the egui interface, `cli` the
//! command line.
pub mod cli;
pub mod constants;
pub mod flows;
pub mod model;
pub mod services;
pub mod ui;
