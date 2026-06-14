// core::playlist — per-monitor image list + cursor

use std::path::{Path, PathBuf};

use rand::seq::SliceRandom;
use rand::Rng;

use crate::model::CycleMode;

/// Supported image file extensions (§image-formats, case-insensitive).
/// webp is explicitly excluded; anything else is ignored.
const SUPPORTED_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "bmp"];

/// Per-monitor image playlist with cursor and shuffle state.
///
/// # Folder scanning
///
/// `Playlist::build` scans each folder **non-recursively** (top-level entries
/// only). Subdirectories and their contents are silently ignored. This is a
/// deliberate design choice: the user places images directly in the configured
/// folder; nested organisation is out of scope (§ Non-goals).
pub struct Playlist {
    /// Sorted by filename (case-insensitive, stable) for Sequential mode.
    images: Vec<PathBuf>,
    /// Cursor used by Sequential and as the position within `shuffle_order`.
    cursor: usize,
    /// Current shuffle permutation (indices into `images`).
    shuffle_order: Vec<usize>,
}

impl Playlist {
    /// Scan all folders, collect supported images, sort by file name
    /// (case-insensitive, stable). Missing or empty folders contribute nothing.
    /// An empty result is valid — `next()` will return `None`.
    ///
    /// Scan is **non-recursive**: only top-level entries in each folder are
    /// considered. Subdirectories and their contents are ignored.
    pub fn build(folders: &[PathBuf]) -> Playlist {
        let mut images: Vec<PathBuf> = Vec::new();

        for folder in folders {
            let entries = match std::fs::read_dir(folder) {
                Ok(e) => e,
                Err(_) => continue, // missing or unreadable folder — skip
            };
            for entry in entries.flatten() {
                let path = entry.path();
                // Non-recursive: skip subdirectories entirely.
                if path.is_dir() {
                    continue;
                }
                if is_supported_extension(&path) {
                    images.push(path);
                }
            }
        }

        // Sort by filename (the last component), case-insensitive, stable.
        images.sort_by(|a, b| {
            let fa = a
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let fb = b
                .file_name()
                .map(|n| n.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            fa.cmp(&fb)
        });

        let len = images.len();
        let shuffle_order: Vec<usize> = (0..len).collect();

        Playlist {
            images,
            cursor: 0,
            shuffle_order,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    /// Advance and return the next image for `mode`, using `rng` for random
    /// modes. Returns `None` iff the playlist is empty.
    ///
    /// - **Sequential**: returns `images[cursor]`, then advances cursor with
    ///   wrap-around: `cursor = (cursor + 1) % len`.
    /// - **Shuffle**: walks `shuffle_order`; when the permutation is exhausted
    ///   it reshuffles and resets the cursor (a fresh pass may begin with the
    ///   same item that ended the previous pass).
    /// - **PureRandom**: picks a uniform random index each call; repeats are
    ///   allowed.
    pub fn next(&mut self, mode: CycleMode, rng: &mut impl Rng) -> Option<PathBuf> {
        if self.images.is_empty() {
            return None;
        }
        let len = self.images.len();

        match mode {
            CycleMode::Sequential => {
                let path = self.images[self.cursor].clone();
                self.cursor = (self.cursor + 1) % len;
                Some(path)
            }
            CycleMode::Shuffle => {
                // If cursor has walked past the end of the permutation, reshuffle.
                if self.cursor >= self.shuffle_order.len() {
                    self.shuffle_order.shuffle(rng);
                    self.cursor = 0;
                }
                let idx = self.shuffle_order[self.cursor];
                self.cursor += 1;
                Some(self.images[idx].clone())
            }
            CycleMode::PureRandom => {
                let idx = rng.gen_range(0..len);
                Some(self.images[idx].clone())
            }
        }
    }

    /// Remove the bad path from the playlist, then return the following
    /// `next()`. Used when `WallpaperSetter` reports a corrupt/unreadable image.
    ///
    /// After removal the cursor and shuffle state are adjusted so that the
    /// playlist remains consistent.
    pub fn skip_and_next(
        &mut self,
        bad: &Path,
        mode: CycleMode,
        rng: &mut impl Rng,
    ) -> Option<PathBuf> {
        if let Some(pos) = self.images.iter().position(|p| p == bad) {
            self.images.remove(pos);

            // Keep the shuffle_order consistent: remove the entry that pointed
            // to `pos` and decrement all entries that pointed above `pos`.
            self.shuffle_order.retain(|&i| i != pos);
            for i in self.shuffle_order.iter_mut() {
                if *i > pos {
                    *i -= 1;
                }
            }

            // Adjust the cursor so it stays valid.
            match mode {
                CycleMode::Sequential => {
                    if self.images.is_empty() {
                        self.cursor = 0;
                    } else if pos < self.cursor {
                        // The removed item was before the cursor; step back one
                        // so the cursor still points to the same logical next
                        // item (now shifted one position earlier).
                        self.cursor = self.cursor.saturating_sub(1);
                    } else if self.cursor >= self.images.len() {
                        self.cursor = 0;
                    }
                }
                CycleMode::Shuffle | CycleMode::PureRandom => {
                    // For Shuffle, the shuffle_order was already patched above.
                    // Clamp cursor in case the permutation got shorter.
                    if self.cursor > self.shuffle_order.len() {
                        self.cursor = self.shuffle_order.len();
                    }
                }
            }
        }

        self.next(mode, rng)
    }
}

fn is_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use tempfile::TempDir;

    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_file(dir: &TempDir, name: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, b"dummy").expect("write test file");
        p
    }

    fn make_subdir_file(dir: &TempDir, sub: &str, name: &str) -> PathBuf {
        let subdir = dir.path().join(sub);
        fs::create_dir_all(&subdir).expect("create subdir");
        let p = subdir.join(name);
        fs::write(&p, b"dummy").expect("write test file");
        p
    }

    fn seeded_rng() -> StdRng {
        StdRng::seed_from_u64(42)
    }

    // -----------------------------------------------------------------------
    // §5.3 — Folder scan filtering
    // -----------------------------------------------------------------------

    /// A temp folder with a.jpg, b.JPEG, c.png, d.bmp, e.webp, f.txt,
    /// sub/g.jpg → only the four supported files are included;
    /// webp, txt, and sub/g.jpg are excluded (non-recursive scan).
    #[test]
    fn scan_filtering_supported_only_and_non_recursive() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");
        make_file(&dir, "b.JPEG");
        make_file(&dir, "c.png");
        make_file(&dir, "d.bmp");
        make_file(&dir, "e.webp"); // excluded — webp not supported
        make_file(&dir, "f.txt"); // excluded — unknown extension
        make_subdir_file(&dir, "sub", "g.jpg"); // excluded — non-recursive

        let pl = Playlist::build(&[dir.path().to_path_buf()]);

        assert_eq!(pl.len(), 4, "expected a.jpg, b.JPEG, c.png, d.bmp only");
        assert!(!pl.is_empty());

        // Verify filenames present (case-insensitive sort: a, b, c, d).
        let names: Vec<String> = pl
            .images
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a.jpg", "b.JPEG", "c.png", "d.bmp"]);
    }

    /// Case-insensitive extension matching: .JPG, .Png, .BMP all accepted.
    #[test]
    fn scan_case_insensitive_extensions() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "x.JPG");
        make_file(&dir, "y.Png");
        make_file(&dir, "z.BMP");

        let pl = Playlist::build(&[dir.path().to_path_buf()]);
        assert_eq!(pl.len(), 3);
    }

    // -----------------------------------------------------------------------
    // §5.3 — Empty handling
    // -----------------------------------------------------------------------

    #[test]
    fn empty_build_no_folders() {
        let pl = Playlist::build(&[]);
        assert!(pl.is_empty());
        assert_eq!(pl.len(), 0);
    }

    #[test]
    fn empty_build_missing_folder() {
        let pl = Playlist::build(&[PathBuf::from("/nonexistent/path/abc123")]);
        assert!(pl.is_empty());
        assert_eq!(pl.len(), 0);
    }

    #[test]
    fn next_returns_none_when_empty() {
        let mut pl = Playlist::build(&[]);
        let mut rng = seeded_rng();
        assert_eq!(pl.next(CycleMode::Sequential, &mut rng), None);
        assert_eq!(pl.next(CycleMode::Shuffle, &mut rng), None);
        assert_eq!(pl.next(CycleMode::PureRandom, &mut rng), None);
    }

    // -----------------------------------------------------------------------
    // §5.3 — Sequential mode
    // -----------------------------------------------------------------------

    /// Sequential returns images in sorted-by-filename order and wraps around.
    #[test]
    fn sequential_sorted_order_and_wraparound() {
        let dir = TempDir::new().unwrap();
        // Create in reverse order to confirm sort is applied.
        make_file(&dir, "c.jpg");
        make_file(&dir, "a.jpg");
        make_file(&dir, "b.jpg");

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        let r0 = pl.next(CycleMode::Sequential, &mut rng).unwrap();
        let r1 = pl.next(CycleMode::Sequential, &mut rng).unwrap();
        let r2 = pl.next(CycleMode::Sequential, &mut rng).unwrap();
        // Wraparound: the 4th call returns the 1st image again.
        let r3 = pl.next(CycleMode::Sequential, &mut rng).unwrap();

        assert_eq!(r0.file_name().unwrap(), "a.jpg");
        assert_eq!(r1.file_name().unwrap(), "b.jpg");
        assert_eq!(r2.file_name().unwrap(), "c.jpg");
        assert_eq!(r3.file_name().unwrap(), "a.jpg", "expected wrap-around");
    }

    // -----------------------------------------------------------------------
    // §5.3 — Shuffle no-repeat invariant
    // -----------------------------------------------------------------------

    /// Across one full pass of N items, all N distinct paths appear exactly once.
    #[test]
    fn shuffle_no_repeat_within_pass() {
        let dir = TempDir::new().unwrap();
        let n = 6usize;
        for i in 0..n {
            make_file(&dir, &format!("{i}.jpg"));
        }

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for _ in 0..n {
            let p = pl.next(CycleMode::Shuffle, &mut rng).unwrap();
            assert!(
                seen.insert(p.clone()),
                "duplicate image in single shuffle pass: {:?}",
                p
            );
        }
        assert_eq!(seen.len(), n, "all {} images must appear exactly once", n);
    }

    /// After one full pass, the next pass also contains all N images exactly
    /// once (reshuffle on exhaustion).
    #[test]
    fn shuffle_reshuffles_after_exhaustion() {
        let dir = TempDir::new().unwrap();
        let n = 4usize;
        for i in 0..n {
            make_file(&dir, &format!("{i}.jpg"));
        }

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        // Consume first pass.
        for _ in 0..n {
            pl.next(CycleMode::Shuffle, &mut rng).unwrap();
        }
        // Second pass: all N distinct.
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for _ in 0..n {
            let p = pl.next(CycleMode::Shuffle, &mut rng).unwrap();
            seen.insert(p);
        }
        assert_eq!(seen.len(), n);
    }

    // -----------------------------------------------------------------------
    // §5.3 — PureRandom mode
    // -----------------------------------------------------------------------

    /// Every returned index must be in range; with a fixed seed the sequence is
    /// reproducible; repeats are permitted (no uniqueness assertion).
    #[test]
    fn pure_random_in_range_and_reproducible() {
        let dir = TempDir::new().unwrap();
        let n = 5usize;
        for i in 0..n {
            make_file(&dir, &format!("{i}.jpg"));
        }

        let folders = vec![dir.path().to_path_buf()];

        // Run with seed 99 twice; results must be identical.
        let results_a: Vec<PathBuf> = {
            let mut pl = Playlist::build(&folders);
            let mut rng = StdRng::seed_from_u64(99);
            (0..10)
                .map(|_| pl.next(CycleMode::PureRandom, &mut rng).unwrap())
                .collect()
        };
        let results_b: Vec<PathBuf> = {
            let mut pl = Playlist::build(&folders);
            let mut rng = StdRng::seed_from_u64(99);
            (0..10)
                .map(|_| pl.next(CycleMode::PureRandom, &mut rng).unwrap())
                .collect()
        };

        assert_eq!(results_a, results_b, "same seed must yield same sequence");

        // All returned paths must be in the images vec.
        let images: std::collections::HashSet<PathBuf> =
            Playlist::build(&folders).images.into_iter().collect();
        for p in &results_a {
            assert!(
                images.contains(p),
                "returned path not in image set: {:?}",
                p
            );
        }
    }

    // -----------------------------------------------------------------------
    // §5.3 — skip_and_next
    // -----------------------------------------------------------------------

    /// Removes the bad path and returns the following image (or None when that
    /// empties the list).
    #[test]
    fn skip_and_next_removes_bad_and_returns_valid() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");
        make_file(&dir, "b.jpg");
        make_file(&dir, "c.jpg");

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        // Advance to a.jpg so cursor points at b.jpg next.
        let first = pl.next(CycleMode::Sequential, &mut rng).unwrap();
        assert_eq!(first.file_name().unwrap(), "a.jpg");

        // Pretend a.jpg was corrupt; skip_and_next should:
        // 1. Remove a.jpg from the list (now [b.jpg, c.jpg]).
        // 2. Return the next item after the removal.
        let after = pl.skip_and_next(&first, CycleMode::Sequential, &mut rng);
        assert!(after.is_some(), "expected a valid next image");
        // After removing a.jpg the list is [b.jpg, c.jpg].
        // Cursor was at 1 (pointing to b.jpg), removal shifts it → still b.jpg.
        let name = after.unwrap();
        let name_str = name.file_name().unwrap().to_str().unwrap();
        assert!(
            name_str == "b.jpg" || name_str == "c.jpg",
            "unexpected image: {name_str}"
        );
        assert_eq!(pl.len(), 2, "bad image should be removed");
    }

    #[test]
    fn skip_and_next_single_image_returns_none() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "only.jpg");

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        let only = pl.images[0].clone();
        let result = pl.skip_and_next(&only, CycleMode::Sequential, &mut rng);
        assert_eq!(result, None, "removing the only image should return None");
        assert!(pl.is_empty());
    }

    #[test]
    fn skip_and_next_unknown_path_is_noop() {
        let dir = TempDir::new().unwrap();
        make_file(&dir, "a.jpg");
        make_file(&dir, "b.jpg");

        let mut pl = Playlist::build(&[dir.path().to_path_buf()]);
        let mut rng = seeded_rng();

        // Pass a path not in the playlist — should not panic, just call next().
        let ghost = PathBuf::from("/no/such/image.jpg");
        let result = pl.skip_and_next(&ghost, CycleMode::Sequential, &mut rng);
        assert!(
            result.is_some(),
            "should return next even when bad not found"
        );
        assert_eq!(pl.len(), 2, "no removal should have occurred");
    }
}
