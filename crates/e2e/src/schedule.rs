//! Running checks side by side: each waits for the checks it builds on (and
//! is skipped if one didn't pass), and a check that must run alone waits for
//! the running ones to finish while holding new ones back.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Skipped,
}

/// Checks done so far, for those waiting on them.
#[derive(Default)]
struct Board {
    done: std::collections::HashMap<&'static str, Outcome>,
}

/// Lets ordinary checks overlap, and a check that must run alone wait for the
/// running ones to finish while holding new ones back.
#[derive(Default)]
struct Gate {
    running: usize,
    alone_waiting: usize,
    alone_running: bool,
}

#[derive(Default)]
pub struct Scheduler {
    board: std::sync::Mutex<Board>,
    board_changed: std::sync::Condvar,
    gate: std::sync::Mutex<Gate>,
    gate_changed: std::sync::Condvar,
}

impl Scheduler {
    /// Wait until every prerequisite has run; whether they all passed.
    pub fn prerequisites_passed(&self, needs: &[&str]) -> bool {
        let mut board = self.board.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if needs.iter().all(|n| board.done.contains_key(n)) {
                return needs.iter().all(|n| board.done[n] == Outcome::Passed);
            }
            board = self
                .board_changed
                .wait(board)
                .unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn finish(&self, name: &'static str, outcome: Outcome) {
        let mut board = self.board.lock().unwrap_or_else(|p| p.into_inner());
        board.done.insert(name, outcome);
        self.board_changed.notify_all();
    }

    pub fn enter(&self, alone: bool) {
        let mut gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        if alone {
            gate.alone_waiting += 1;
            while gate.running > 0 || gate.alone_running {
                gate = self
                    .gate_changed
                    .wait(gate)
                    .unwrap_or_else(|p| p.into_inner());
            }
            gate.alone_waiting -= 1;
            gate.alone_running = true;
        } else {
            while gate.alone_waiting > 0 || gate.alone_running {
                gate = self
                    .gate_changed
                    .wait(gate)
                    .unwrap_or_else(|p| p.into_inner());
            }
            gate.running += 1;
        }
    }

    pub fn leave(&self, alone: bool) {
        let mut gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        if alone {
            gate.alone_running = false;
        } else {
            gate.running -= 1;
        }
        self.gate_changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_check_runs_only_after_what_it_builds_on_passed() {
        let s = Arc::new(Scheduler::default());
        let waiting = {
            let s = s.clone();
            std::thread::spawn(move || s.prerequisites_passed(&["a", "b"]))
        };
        s.finish("a", Outcome::Passed);
        std::thread::sleep(Duration::from_millis(50));
        assert!(!waiting.is_finished(), "still waiting for b");
        s.finish("b", Outcome::Failed);
        assert!(!waiting.join().unwrap(), "b failed: skip");
        assert!(s.prerequisites_passed(&["a"]));
        assert!(s.prerequisites_passed(&[]));
    }

    #[test]
    fn a_check_that_runs_alone_waits_for_the_rest_and_holds_new_ones() {
        let s = Arc::new(Scheduler::default());
        s.enter(false);
        let alone = {
            let s = s.clone();
            std::thread::spawn(move || {
                s.enter(true);
                s.leave(true);
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!alone.is_finished(), "waits for the running check");
        let newcomer = {
            let s = s.clone();
            std::thread::spawn(move || {
                s.enter(false);
                s.leave(false);
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            !newcomer.is_finished(),
            "held back while one waits to run alone"
        );
        s.leave(false);
        alone.join().unwrap();
        newcomer.join().unwrap();
    }
}
