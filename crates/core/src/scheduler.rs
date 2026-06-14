// core::scheduler — global tick + advance logic

use std::path::PathBuf;

use rand::Rng;

use crate::model::{AppError, CycleMode, FitMode, MonitorId, WallpaperSetter};
use crate::playlist::Playlist;

pub struct Scheduler<W: WallpaperSetter, R: Rng> {
    setter: W,
    rng: R,
    mode: CycleMode,
    fit: FitMode,
    paused: bool,
    /// Per-monitor (id, playlist).
    monitors: Vec<(MonitorId, Playlist)>,
}

pub struct TickOutcome {
    pub monitor: MonitorId,
    pub result: TickResult,
}

pub enum TickResult {
    Applied(PathBuf),
    SkippedEmpty,
    Failed(String),
}

impl<W: WallpaperSetter, R: Rng> Scheduler<W, R> {
    pub fn new(setter: W, rng: R, mode: CycleMode, fit: FitMode) -> Self {
        Scheduler {
            setter,
            rng,
            mode,
            fit,
            paused: false,
            monitors: Vec::new(),
        }
    }

    /// (Re)build playlists from assignments. Fresh build — cursors are reset.
    pub fn rebuild(&mut self, assignments: &[(MonitorId, Vec<PathBuf>)]) {
        self.monitors = assignments
            .iter()
            .map(|(id, folders)| (id.clone(), Playlist::build(folders)))
            .collect();
    }

    pub fn set_mode(&mut self, mode: CycleMode) {
        self.mode = mode;
    }

    pub fn set_fit(&mut self, fit: FitMode) {
        self.fit = fit;
    }

    pub fn pause(&mut self) {
        self.paused = true;
    }

    pub fn resume(&mut self) {
        self.paused = false;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Advance every monitor and apply. No-op while paused.
    /// On BadImage error: skip_and_next + retry once.
    /// On other errors: that monitor is Failed, others still applied.
    /// Never panics on a single failure.
    pub fn tick(&mut self) -> Vec<TickOutcome> {
        if self.paused {
            return Vec::new();
        }
        self.advance_and_apply()
    }

    /// Same as tick(), but ignores paused (manual Next).
    pub fn next_now(&mut self) -> Vec<TickOutcome> {
        self.advance_and_apply()
    }

    fn advance_and_apply(&mut self) -> Vec<TickOutcome> {
        let mode = self.mode;
        let fit = self.fit;
        let mut outcomes = Vec::with_capacity(self.monitors.len());

        for (id, playlist) in &mut self.monitors {
            let outcome = match playlist.next(mode, &mut self.rng) {
                None => TickOutcome {
                    monitor: id.clone(),
                    result: TickResult::SkippedEmpty,
                },
                Some(path) => match self.setter.set(id, &path, fit) {
                    Ok(()) => TickOutcome {
                        monitor: id.clone(),
                        result: TickResult::Applied(path),
                    },
                    Err(AppError::BadImage(_)) => {
                        // skip_and_next + retry once
                        match playlist.skip_and_next(&path, mode, &mut self.rng) {
                            None => TickOutcome {
                                monitor: id.clone(),
                                result: TickResult::SkippedEmpty,
                            },
                            Some(next_path) => match self.setter.set(id, &next_path, fit) {
                                Ok(()) => TickOutcome {
                                    monitor: id.clone(),
                                    result: TickResult::Applied(next_path),
                                },
                                Err(e) => TickOutcome {
                                    monitor: id.clone(),
                                    result: TickResult::Failed(format!("{e:?}")),
                                },
                            },
                        }
                    }
                    Err(e) => TickOutcome {
                        monitor: id.clone(),
                        result: TickResult::Failed(format!("{e:?}")),
                    },
                },
            };
            outcomes.push(outcome);
        }

        outcomes
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::{Path, PathBuf};

    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use tempfile::TempDir;

    use super::*;
    use crate::model::{AppError, CycleMode, FitMode, MonitorId, WallpaperSetter};

    // -----------------------------------------------------------------------
    // MockSetter
    // -----------------------------------------------------------------------

    /// What kind of failure the mock should produce (avoids cloning AppError).
    #[derive(Clone, Copy, Debug)]
    enum FailMode {
        BadImage,
        OsError,
    }

    struct MockSetter {
        calls: RefCell<Vec<(MonitorId, PathBuf, FitMode)>>,
        /// If Some, the mock fails every call with this kind of error.
        fail: Option<FailMode>,
    }

    impl MockSetter {
        fn ok() -> Self {
            MockSetter {
                calls: RefCell::new(Vec::new()),
                fail: None,
            }
        }

        fn failing(mode: FailMode) -> Self {
            MockSetter {
                calls: RefCell::new(Vec::new()),
                fail: Some(mode),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.borrow().len()
        }

        fn calls(&self) -> Vec<(MonitorId, PathBuf, FitMode)> {
            self.calls.borrow().clone()
        }
    }

    impl WallpaperSetter for MockSetter {
        fn set(&self, m: &MonitorId, p: &Path, f: FitMode) -> Result<(), AppError> {
            self.calls
                .borrow_mut()
                .push((m.clone(), p.to_path_buf(), f));
            match self.fail {
                None => Ok(()),
                Some(FailMode::BadImage) => {
                    Err(AppError::BadImage(format!("bad: {}", p.display())))
                }
                Some(FailMode::OsError) => Err(AppError::Os("simulated OS error".into())),
            }
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_file(dir: &TempDir, name: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, b"dummy").expect("write test file");
        p
    }

    fn seeded_rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    // -----------------------------------------------------------------------
    // tick calls set once per non-empty monitor with the expected next path
    // -----------------------------------------------------------------------

    #[test]
    fn tick_calls_set_for_each_non_empty_monitor() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let path_a = make_file(&dir_a, "a.jpg");
        let path_b = make_file(&dir_b, "b.jpg");

        let setter = MockSetter::ok();
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[
            ("mon_a".to_string(), vec![dir_a.path().to_path_buf()]),
            ("mon_b".to_string(), vec![dir_b.path().to_path_buf()]),
        ]);

        let outcomes = sched.tick();

        assert_eq!(outcomes.len(), 2);
        assert_eq!(sched.setter.call_count(), 2);

        let calls = sched.setter.calls();
        // mon_a should have been called with a.jpg
        let call_a = calls.iter().find(|(id, _, _)| id == "mon_a").unwrap();
        assert_eq!(call_a.1, path_a);
        assert_eq!(call_a.2, FitMode::Fill);

        // mon_b should have been called with b.jpg
        let call_b = calls.iter().find(|(id, _, _)| id == "mon_b").unwrap();
        assert_eq!(call_b.1, path_b);

        // outcomes are Applied
        for outcome in &outcomes {
            assert!(matches!(outcome.result, TickResult::Applied(_)));
        }
    }

    // -----------------------------------------------------------------------
    // Empty monitors => SkippedEmpty, no set call
    // -----------------------------------------------------------------------

    #[test]
    fn tick_empty_monitor_skipped_no_set_call() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");

        let setter = MockSetter::ok();
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[
            ("mon_full".to_string(), vec![dir.path().to_path_buf()]),
            ("mon_empty".to_string(), vec![]), // no folders → empty playlist
        ]);

        let outcomes = sched.tick();

        assert_eq!(outcomes.len(), 2);

        let empty_outcome = outcomes.iter().find(|o| o.monitor == "mon_empty").unwrap();
        assert!(matches!(empty_outcome.result, TickResult::SkippedEmpty));

        let full_outcome = outcomes.iter().find(|o| o.monitor == "mon_full").unwrap();
        assert!(matches!(full_outcome.result, TickResult::Applied(_)));

        // Only one set call — for the non-empty monitor
        assert_eq!(sched.setter.call_count(), 1);
        let call = &sched.setter.calls()[0];
        assert_eq!(call.0, "mon_full");
    }

    // -----------------------------------------------------------------------
    // paused => tick does no set; next_now still applies
    // -----------------------------------------------------------------------

    #[test]
    fn paused_tick_does_nothing_next_now_still_applies() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");

        let setter = MockSetter::ok();
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[("mon".to_string(), vec![dir.path().to_path_buf()])]);
        sched.pause();

        assert!(sched.is_paused());

        // tick while paused should be a no-op
        let outcomes = sched.tick();
        assert!(
            outcomes.is_empty(),
            "tick while paused must return empty vec"
        );
        assert_eq!(sched.setter.call_count(), 0, "no set calls while paused");

        // next_now ignores paused
        let outcomes2 = sched.next_now();
        assert_eq!(outcomes2.len(), 1);
        assert!(matches!(outcomes2[0].result, TickResult::Applied(_)));
        assert_eq!(sched.setter.call_count(), 1);
    }

    // -----------------------------------------------------------------------
    // bad-image error => skip_and_next + retry
    // -----------------------------------------------------------------------

    #[test]
    fn bad_image_triggers_skip_and_retry() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "bad.jpg");
        make_file(&dir, "good.jpg");

        // Setter always returns BadImage — the retry will also fail, so we
        // test that skip_and_next is called and the retry happens.
        // To isolate: use a setter that only fails for "bad.jpg".
        struct PartialFail {
            calls: RefCell<Vec<PathBuf>>,
        }
        impl WallpaperSetter for PartialFail {
            fn set(&self, _m: &MonitorId, p: &Path, _f: FitMode) -> Result<(), AppError> {
                self.calls.borrow_mut().push(p.to_path_buf());
                if p.file_name().unwrap() == "bad.jpg" {
                    Err(AppError::BadImage("corrupt".into()))
                } else {
                    Ok(())
                }
            }
        }

        let setter = PartialFail {
            calls: RefCell::new(Vec::new()),
        };
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[("mon".to_string(), vec![dir.path().to_path_buf()])]);

        // First tick: sequential order is [bad.jpg, good.jpg].
        // bad.jpg triggers BadImage → skip_and_next → good.jpg → Applied.
        let outcomes = sched.tick();
        assert_eq!(outcomes.len(), 1);
        assert!(
            matches!(&outcomes[0].result, TickResult::Applied(p) if p.file_name().unwrap() == "good.jpg"),
            "expected Applied(good.jpg)"
        );

        // The setter was called twice: once for bad.jpg (failed) and once for good.jpg (ok).
        let calls = sched.setter.calls.borrow();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].file_name().unwrap(), "bad.jpg");
        assert_eq!(calls[1].file_name().unwrap(), "good.jpg");
    }

    // -----------------------------------------------------------------------
    // generic OS error => that monitor Failed, others still applied
    // -----------------------------------------------------------------------

    #[test]
    fn os_error_one_monitor_fails_others_applied() {
        let dir_ok = TempDir::new().unwrap();
        let dir_bad = TempDir::new().unwrap();
        make_file(&dir_ok, "ok.jpg");
        make_file(&dir_bad, "bad.jpg");

        // Setter fails only for "mon_bad"
        struct SelectiveFail {
            calls: RefCell<Vec<(MonitorId, PathBuf)>>,
            fail_id: String,
        }
        impl WallpaperSetter for SelectiveFail {
            fn set(&self, m: &MonitorId, p: &Path, _f: FitMode) -> Result<(), AppError> {
                self.calls.borrow_mut().push((m.clone(), p.to_path_buf()));
                if m == &self.fail_id {
                    Err(AppError::Os("simulated OS failure".into()))
                } else {
                    Ok(())
                }
            }
        }

        let setter = SelectiveFail {
            calls: RefCell::new(Vec::new()),
            fail_id: "mon_bad".to_string(),
        };
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[
            ("mon_ok".to_string(), vec![dir_ok.path().to_path_buf()]),
            ("mon_bad".to_string(), vec![dir_bad.path().to_path_buf()]),
        ]);

        let outcomes = sched.tick();
        assert_eq!(outcomes.len(), 2);

        let ok_outcome = outcomes.iter().find(|o| o.monitor == "mon_ok").unwrap();
        assert!(
            matches!(ok_outcome.result, TickResult::Applied(_)),
            "mon_ok should be Applied"
        );

        let bad_outcome = outcomes.iter().find(|o| o.monitor == "mon_bad").unwrap();
        assert!(
            matches!(bad_outcome.result, TickResult::Failed(_)),
            "mon_bad should be Failed"
        );

        // Both monitors were attempted (set was called for each)
        let calls = sched.setter.calls.borrow();
        assert_eq!(calls.len(), 2);
    }

    // -----------------------------------------------------------------------
    // set_mode switches advance semantics on next tick
    // -----------------------------------------------------------------------

    #[test]
    fn set_mode_switches_advance_semantics() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");
        make_file(&dir, "b.jpg");
        make_file(&dir, "c.jpg");

        let setter = MockSetter::ok();
        // Start in Sequential
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[("mon".to_string(), vec![dir.path().to_path_buf()])]);

        // Sequential: first tick → a.jpg, second → b.jpg
        let o1 = sched.tick();
        let o2 = sched.tick();
        let p1 = match &o1[0].result {
            TickResult::Applied(p) => p.clone(),
            _ => panic!("expected Applied"),
        };
        let p2 = match &o2[0].result {
            TickResult::Applied(p) => p.clone(),
            _ => panic!("expected Applied"),
        };
        assert_eq!(p1.file_name().unwrap(), "a.jpg");
        assert_eq!(p2.file_name().unwrap(), "b.jpg");

        // Switch to PureRandom; rebuild to reset cursor
        sched.set_mode(CycleMode::PureRandom);
        sched.rebuild(&[("mon".to_string(), vec![dir.path().to_path_buf()])]);

        // With PureRandom the results are from a random pick; just verify they're valid paths
        let o3 = sched.tick();
        let p3 = match &o3[0].result {
            TickResult::Applied(p) => p.clone(),
            _ => panic!("expected Applied after mode switch"),
        };
        let valid_names = ["a.jpg", "b.jpg", "c.jpg"];
        assert!(
            valid_names.contains(&p3.file_name().unwrap().to_str().unwrap()),
            "PureRandom result should be one of the valid images"
        );
    }

    // -----------------------------------------------------------------------
    // All monitors advance on the same tick
    // -----------------------------------------------------------------------

    #[test]
    fn all_monitors_advance_on_same_tick() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        let dir_c = TempDir::new().unwrap();
        make_file(&dir_a, "a1.jpg");
        make_file(&dir_a, "a2.jpg");
        make_file(&dir_b, "b1.jpg");
        make_file(&dir_b, "b2.jpg");
        make_file(&dir_c, "c1.jpg");
        make_file(&dir_c, "c2.jpg");

        let setter = MockSetter::ok();
        let mut sched = Scheduler::new(setter, seeded_rng(), CycleMode::Sequential, FitMode::Fill);
        sched.rebuild(&[
            ("mon_a".to_string(), vec![dir_a.path().to_path_buf()]),
            ("mon_b".to_string(), vec![dir_b.path().to_path_buf()]),
            ("mon_c".to_string(), vec![dir_c.path().to_path_buf()]),
        ]);

        // First tick: all three monitors advance (cursor 0 → 1 for each)
        let outcomes = sched.tick();
        assert_eq!(
            outcomes.len(),
            3,
            "all 3 monitors should produce an outcome"
        );

        let calls = sched.setter.calls();
        assert_eq!(calls.len(), 3, "set called once per monitor");

        // Verify each monitor got its first image
        let call_a = calls.iter().find(|(id, _, _)| id == "mon_a").unwrap();
        let call_b = calls.iter().find(|(id, _, _)| id == "mon_b").unwrap();
        let call_c = calls.iter().find(|(id, _, _)| id == "mon_c").unwrap();
        assert_eq!(call_a.1.file_name().unwrap(), "a1.jpg");
        assert_eq!(call_b.1.file_name().unwrap(), "b1.jpg");
        assert_eq!(call_c.1.file_name().unwrap(), "c1.jpg");
    }
}
