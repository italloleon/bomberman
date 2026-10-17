//! The registry of live rooms: room code -> channel into that room's task.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rand::Rng;
use tokio::sync::mpsc;

use crate::room::{self, RoomCmd};

/// No I/O/0/1 so codes are easy to read aloud and type.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 4;

/// Cheap to clone: every clone points at the same map.
/// A std `Mutex` is fine here because we never hold it across an `.await`.
#[derive(Clone, Default)]
pub struct Rooms(Arc<Mutex<HashMap<String, mpsc::Sender<RoomCmd>>>>);

impl Rooms {
    /// Create a room, start its task, and return its code.
    pub fn create(&self) -> String {
        let mut rng = rand::rng();
        let mut map = self.0.lock().unwrap();
        let code = loop {
            let code: String = (0..CODE_LEN)
                .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
                .collect();
            if !map.contains_key(&code) {
                break code;
            }
        };
        let tx = room::spawn(code.clone(), rng.random(), self.clone());
        map.insert(code.clone(), tx);
        tracing::info!(room = %code, "room created");
        code
    }

    pub fn get(&self, code: &str) -> Option<mpsc::Sender<RoomCmd>> {
        self.0.lock().unwrap().get(code).cloned()
    }

    /// Called by a room's own task when it shuts down.
    pub fn remove(&self, code: &str) {
        self.0.lock().unwrap().remove(code);
        tracing::info!(room = %code, "room closed");
    }
}
