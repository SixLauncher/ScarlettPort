//! The host's memory as the runtime may really use it.
//!
//! Under Wine, `GlobalMemoryStatusEx` reports the whole Linux machine: its
//! free RAM, and its free RAM plus free swap as the commit left. A launcher
//! may run the title in a control group with a ceiling of its own
//! (SixLauncher: a systemd scope with `MemoryMax` and `MemorySwapMax`). The
//! runtime then believed it had gigabytes left while its group's swap was
//! full: the checks that cut caches and slow texture loading under memory
//! pressure (`scarlett_gpu::residency`) never fired, and the title paged
//! itself to a crawl.
//!
//! [`global_memory_status`] is `GlobalMemoryStatusEx` with the free memory
//! and the commit left cut to what the process's control group and each of
//! its parents still allow (cgroup v2, read through Wine's `Z:` drive).
//! On Windows, or where no limit is found, it is `GlobalMemoryStatusEx`.

/// One level of a cgroup v2 hierarchy: its ceilings (`None`: `max`) and use,
/// in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Level {
    pub memory_max: Option<u64>,
    pub memory_current: u64,
    pub swap_max: Option<u64>,
    pub swap_current: u64,
}

/// What the levels still allow: (memory, memory and swap together), each
/// `None` when no level limits it. The tightest level wins.
pub fn headroom(levels: &[Level]) -> (Option<u64>, Option<u64>) {
    let mut memory: Option<u64> = None;
    let mut commit: Option<u64> = None;
    let tighter = |a: Option<u64>, b: u64| Some(a.map_or(b, |a| a.min(b)));
    for level in levels {
        let Some(max) = level.memory_max else { continue };
        let left = max.saturating_sub(level.memory_current);
        memory = tighter(memory, left);
        // Commit is bounded only where swap is bounded too.
        if let Some(swap_max) = level.swap_max {
            commit = tighter(commit, left + swap_max.saturating_sub(level.swap_current));
        }
    }
    (memory, commit)
}

/// The tightest memory ceiling of the levels, in bytes.
pub fn ceiling(levels: &[Level]) -> Option<u64> {
    levels.iter().filter_map(|l| l.memory_max).min()
}

/// The group in `/proc/self/cgroup` (cgroup v2's `0::/path` line).
pub fn own_group(text: &str) -> Option<&str> {
    text.lines().find_map(|l| l.strip_prefix("0::")).map(str::trim)
}

/// A cgroup file's number: `None` for `max` or anything unreadable.
pub fn number(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// The group's folder and its parents' (up to, not including, the root),
/// innermost first, under `root` with `/` between parts.
pub fn level_dirs(root: &str, group: &str, separator: char) -> Vec<String> {
    let parts: Vec<&str> = group.split('/').filter(|p| !p.is_empty()).collect();
    (1..=parts.len())
        .rev()
        .map(|n| {
            let mut dir = root.to_owned();
            for part in &parts[..n] {
                dir.push(separator);
                dir.push_str(part);
            }
            dir
        })
        .collect()
}

#[cfg(windows)]
mod host {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    /// Linux's files through Wine's drive for `/`.
    const ROOT: &str = r"Z:\sys\fs\cgroup";
    const SELF_CGROUP: &str = r"Z:\proc\self\cgroup";

    /// The folders of the process's group and its parents, found once
    /// (empty on Windows or without cgroup v2).
    fn dirs() -> &'static [String] {
        static DIRS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        DIRS.get_or_init(|| {
            let Ok(text) = std::fs::read_to_string(SELF_CGROUP) else { return Vec::new() };
            let Some(group) = own_group(&text) else { return Vec::new() };
            let dirs = level_dirs(ROOT, group, '\\');
            dirs.into_iter()
                .filter(|d| {
                    std::fs::read_to_string(format!(r"{d}\memory.max")).ok().as_deref().and_then(number).is_some()
                })
                .collect()
        })
    }

    /// The folders of the control groups whose ceilings count (for the log).
    pub fn limited_groups() -> &'static [String] {
        dirs()
    }

    fn read(dir: &str, file: &str) -> Option<u64> {
        number(&std::fs::read_to_string(format!(r"{dir}\{file}")).ok()?)
    }

    /// The limited levels now; read at most every 100 ms (callers ask often).
    fn levels() -> Vec<Level> {
        static LAST: Mutex<Option<(Instant, Vec<Level>)>> = Mutex::new(None);
        let dirs = dirs();
        if dirs.is_empty() {
            return Vec::new();
        }
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, levels)) = &*last
            && at.elapsed() < Duration::from_millis(100)
        {
            return levels.clone();
        }
        let levels: Vec<Level> = dirs
            .iter()
            .map(|d| Level {
                memory_max: read(d, "memory.max"),
                memory_current: read(d, "memory.current").unwrap_or(0),
                swap_max: read(d, "memory.swap.max"),
                swap_current: read(d, "memory.swap.current").unwrap_or(0),
            })
            .collect();
        *last = Some((Instant::now(), levels.clone()));
        levels
    }

    /// `GlobalMemoryStatusEx`, with the free memory, the commit left and the
    /// total cut to the control group's ceilings.
    pub fn global_memory_status() -> Option<MEMORYSTATUSEX> {
        // SAFETY: plain data the call fills in, its length set.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: a sized structure.
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
            return None;
        }
        let levels = levels();
        let (memory, commit) = headroom(&levels);
        if let Some(memory) = memory {
            status.ullAvailPhys = status.ullAvailPhys.min(memory);
        }
        if let Some(commit) = commit {
            status.ullAvailPageFile = status.ullAvailPageFile.min(commit);
        }
        if let Some(max) = ceiling(&levels) {
            status.ullTotalPhys = status.ullTotalPhys.min(max);
        }
        Some(status)
    }
}

#[cfg(windows)]
pub use host::{global_memory_status, limited_groups};

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_tightest_level_says_what_is_left() {
        // The game's scope: 11.5 GB of memory, 3.8 GB of swap, the swap full.
        let scope = Level {
            memory_max: Some(23 * GIB / 2),
            memory_current: 9 * GIB,
            swap_max: Some(4 * GIB),
            swap_current: 4 * GIB,
        };
        let (memory, commit) = headroom(&[scope]);
        assert_eq!(memory, Some(5 * GIB / 2));
        assert_eq!(commit, Some(5 * GIB / 2), "no swap left: commit is the memory left");
        // A parent with no ceiling changes nothing; one tighter wins.
        let open = Level::default();
        assert_eq!(headroom(&[scope, open]), (Some(5 * GIB / 2), Some(5 * GIB / 2)));
        let tight = Level { memory_max: Some(10 * GIB), memory_current: 9 * GIB, ..Level::default() };
        assert_eq!(headroom(&[scope, tight]).0, Some(GIB));
        assert_eq!(headroom(&[scope, tight]).1, Some(5 * GIB / 2), "its swap is not limited");
        assert_eq!(ceiling(&[scope, tight, open]), Some(10 * GIB));
    }

    /// Under Wine in a group with a ceiling, the memory left is the group's
    /// (ignored: it needs a launcher's scope; prints what it saw).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn under_wine_the_groups_ceiling_counts() {
        let m = global_memory_status().unwrap();
        eprintln!(
            "memstat: groups {:?}; total {} MB, free {} MB, commit left {} MB",
            limited_groups(),
            m.ullTotalPhys >> 20,
            m.ullAvailPhys >> 20,
            m.ullAvailPageFile >> 20
        );
    }

    #[test]
    fn no_ceiling_no_limit() {
        assert_eq!(headroom(&[]), (None, None));
        assert_eq!(headroom(&[Level { memory_current: GIB, ..Level::default() }]), (None, None));
        // Over the ceiling (it can be, briefly): nothing left, not a wrap.
        let over = Level { memory_max: Some(GIB), memory_current: 2 * GIB, swap_max: Some(0), swap_current: 0 };
        assert_eq!(headroom(&[over]), (Some(0), Some(0)));
    }

    #[test]
    fn the_cgroup_files_are_read() {
        let text = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/sixlauncher-game-1-0.scope\n";
        let group = own_group(text).unwrap();
        assert_eq!(group, "/user.slice/user-1000.slice/user@1000.service/app.slice/sixlauncher-game-1-0.scope");
        assert_eq!(own_group("1:name=systemd:/x\n"), None, "cgroup v1 only");
        let dirs = level_dirs(r"Z:\sys\fs\cgroup", "/a.slice/b.scope", '\\');
        assert_eq!(dirs, [r"Z:\sys\fs\cgroup\a.slice\b.scope", r"Z:\sys\fs\cgroup\a.slice"]);
        assert_eq!(number("12348030976\n"), Some(12348030976));
        assert_eq!(number("max\n"), None);
    }
}
