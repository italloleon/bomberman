//! Server-side buffer of one client's inputs. The client sends one input per tick; the
//! server consumes exactly one per tick. Network jitter makes inputs arrive in bursts,
//! so they wait here briefly. Lives in `core` so tests can pair it with the client's
//! prediction and check both sides agree.

use std::collections::VecDeque;

use crate::sim::Input;

/// More queued than this and we skip ahead, so a burst can't leave a player lagging
/// permanently behind their own keyboard.
pub const MAX_QUEUED: usize = 3;
/// If the client goes quiet (tab hidden, connection hiccup), keep walking in the last
/// direction for this many ticks, then stop.
pub const STARVED_TICKS_BEFORE_STOP: u32 = 3;

#[derive(Debug, Default)]
pub struct InputQueue {
    queue: VecDeque<(u32, Input)>,
    /// Highest seq received; older or duplicate messages are ignored.
    received: u32,
    /// Highest seq applied. Sent back to the client in snapshots.
    pub ack: u32,
    /// Direction from the last applied input, reused while starved.
    last_dir: Option<crate::sim::Dir>,
    starved: u32,
}

impl InputQueue {
    pub fn push(&mut self, seq: u32, input: Input) {
        if seq <= self.received {
            return;
        }
        self.received = seq;
        self.queue.push_back((seq, input));
    }

    /// The input to apply this tick.
    pub fn next(&mut self) -> Input {
        // Skip ahead if too many are waiting, but never lose a bomb press.
        let mut bomb = false;
        while self.queue.len() > MAX_QUEUED {
            let (seq, input) = self.queue.pop_front().unwrap();
            bomb |= input.bomb;
            self.ack = seq;
        }
        match self.queue.pop_front() {
            Some((seq, input)) => {
                self.ack = seq;
                self.last_dir = input.dir;
                self.starved = 0;
                Input { dir: input.dir, bomb: input.bomb || bomb }
            }
            None => {
                self.starved += 1;
                if self.starved > STARVED_TICKS_BEFORE_STOP {
                    self.last_dir = None;
                }
                Input { dir: self.last_dir, bomb }
            }
        }
    }

    /// A new connection took over this seat: its seqs start from 1 again.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Dir;

    fn i(dir: Option<Dir>, bomb: bool) -> Input {
        Input { dir, bomb }
    }

    #[test]
    fn consumes_one_per_tick_in_order() {
        let mut q = InputQueue::default();
        q.push(1, i(Some(Dir::Up), false));
        q.push(2, i(Some(Dir::Left), false));
        assert_eq!(q.next().dir, Some(Dir::Up));
        assert_eq!(q.ack, 1);
        assert_eq!(q.next().dir, Some(Dir::Left));
        assert_eq!(q.ack, 2);
    }

    #[test]
    fn ignores_old_and_duplicate_seqs() {
        let mut q = InputQueue::default();
        q.push(5, i(Some(Dir::Up), false));
        q.push(5, i(Some(Dir::Down), false));
        q.push(3, i(Some(Dir::Down), false));
        assert_eq!(q.next().dir, Some(Dir::Up));
        assert_eq!(q.next(), i(Some(Dir::Up), false), "starved: repeats direction");
    }

    #[test]
    fn starved_repeats_direction_briefly_then_stops() {
        let mut q = InputQueue::default();
        q.push(1, i(Some(Dir::Right), true));
        assert!(q.next().bomb);
        for _ in 0..STARVED_TICKS_BEFORE_STOP {
            assert_eq!(q.next(), i(Some(Dir::Right), false), "bomb is never repeated");
        }
        assert_eq!(q.next(), i(None, false));
    }

    #[test]
    fn burst_skips_ahead_but_keeps_bomb() {
        let mut q = InputQueue::default();
        q.push(1, i(Some(Dir::Up), true));
        for seq in 2..=6 {
            q.push(seq, i(Some(Dir::Up), false));
        }
        let first = q.next();
        assert!(first.bomb, "skipped bomb press is carried over");
        assert_eq!(q.ack, 4, "skipped 1-3, applied 4, left 5-6 queued");
    }
}
