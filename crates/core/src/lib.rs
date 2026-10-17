//! Deterministic game simulation and wire protocol, shared by the server (native)
//! and the browser client (wasm32). No floats, no wall-clock time, no OS randomness:
//! given the same inputs, every machine computes exactly the same world.

pub mod config;
pub mod game;
pub mod input_queue;
pub mod map;
pub mod protocol;
pub mod rng;
pub mod sim;
