//! Platform-abstracted available-memory probe.
//!
//! [`probe`] returns `Some(MemorySnapshot)` only when the OS gives a signal
//! worth gating on:
//!
//! - **Linux / WSL**: `/proc/meminfo` (`MemAvailable`, plus swap), and when the
//!   process runs under a cgroup v2 memory limit, the tighter of that limit's
//!   headroom and the host figure — inside a limited container the host's free
//!   memory is not what the process can actually use.
//! - **macOS**: `hw.memsize` for the total and `vm_stat` for the reclaimable
//!   page classes, with `vm.swapusage` for swap.
//! - **Anything else**: `None`, which disables the memory gate entirely.

use std::path::Path;

const KIB_PER_MIB: u64 = 1024;

/// Where a snapshot's numbers came from. Kept on the snapshot so `amf doctor`
/// can say what it measured instead of printing bare numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemorySource {
    /// `/proc/meminfo` only.
    ProcMeminfo,
    /// `/proc/meminfo` for the totals, narrowed by a cgroup v2 memory limit.
    CgroupV2,
    /// macOS `sysctl` + `vm_stat`. Only constructed on macOS; the label and
    /// the parsers behind it are still compiled (and tested) everywhere.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    MacOs,
}

impl MemorySource {
    pub fn label(self) -> &'static str {
        match self {
            MemorySource::ProcMeminfo => "/proc/meminfo",
            MemorySource::CgroupV2 => "cgroup v2 memory limit",
            MemorySource::MacOs => "sysctl/vm_stat",
        }
    }
}

/// A point-in-time read of host memory, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    /// Memory that can be handed to a new process without swapping.
    pub available_mb: u64,
    /// Total memory the process is allowed to use (host RAM, or the cgroup
    /// limit when that is tighter).
    pub total_mb: u64,
    /// Unused swap, when the platform reports any.
    pub swap_free_mb: Option<u64>,
    /// Configured swap, when the platform reports any. `Some(0)` means swap is
    /// genuinely off — distinct from `None`, which means "not reported".
    pub swap_total_mb: Option<u64>,
    pub source: MemorySource,
}

impl MemorySnapshot {
    /// Whether available memory has fallen below `threshold_mb`.
    pub fn is_low(&self, threshold_mb: u64) -> bool {
        self.available_mb < threshold_mb
    }

}

/// Read the current memory state, or `None` on a platform with no usable
/// signal. Callers must treat `None` as "no memory gate" rather than as a
/// failure — see the module docs.
pub fn probe() -> Option<MemorySnapshot> {
    #[cfg(target_os = "linux")]
    {
        probe_linux()
    }
    #[cfg(target_os = "macos")]
    {
        probe_macos()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// ---------------------------------------------------------------- Linux/WSL

#[cfg(target_os = "linux")]
fn probe_linux() -> Option<MemorySnapshot> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut snapshot = parse_proc_meminfo(&meminfo)?;

    // A cgroup limit only ever narrows the picture: inside a limited container
    // the host may look idle while the process is one allocation from the OOM
    // killer. Take the tighter of the two.
    if let Some(cgroup) = probe_cgroup_v2(Path::new("/proc/self/cgroup"), Path::new("/sys/fs/cgroup"))
        && cgroup.available_mb < snapshot.available_mb
    {
        snapshot.available_mb = cgroup.available_mb;
        snapshot.total_mb = cgroup.total_mb;
        snapshot.source = MemorySource::CgroupV2;
    }

    Some(snapshot)
}

/// Parse `/proc/meminfo`. Values there are in KiB.
fn parse_proc_meminfo(raw: &str) -> Option<MemorySnapshot> {
    let mut total = None;
    let mut available = None;
    let mut free = None;
    let mut cached = None;
    let mut swap_total = None;
    let mut swap_free = None;

    for line in raw.lines() {
        let (key, rest) = line.split_once(':')?;
        let kib = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok());
        let Some(kib) = kib else { continue };
        match key {
            "MemTotal" => total = Some(kib),
            "MemAvailable" => available = Some(kib),
            "MemFree" => free = Some(kib),
            "Cached" => cached = Some(kib),
            "SwapTotal" => swap_total = Some(kib),
            "SwapFree" => swap_free = Some(kib),
            _ => {}
        }
    }

    // MemAvailable is the kernel's own estimate and the right number; kernels
    // older than 3.14 lack it, where free + page cache is the usual stand-in.
    let available = available.or_else(|| Some(free? + cached.unwrap_or(0)))?;

    Some(MemorySnapshot {
        available_mb: available / KIB_PER_MIB,
        total_mb: total? / KIB_PER_MIB,
        swap_free_mb: swap_free.map(|kib| kib / KIB_PER_MIB),
        swap_total_mb: swap_total.map(|kib| kib / KIB_PER_MIB),
        source: MemorySource::ProcMeminfo,
    })
}

/// Headroom under a cgroup v2 memory limit, if this process is under one.
///
/// A limit can sit on any ancestor cgroup, not just the leaf, so this walks
/// from the process's own cgroup up to the root and keeps the tightest
/// headroom it finds. `None` when the process is unlimited, on cgroup v1, or
/// when the files are unreadable.
fn probe_cgroup_v2(self_cgroup: &Path, mount: &Path) -> Option<MemorySnapshot> {
    let raw = std::fs::read_to_string(self_cgroup).ok()?;
    let rel = parse_cgroup_v2_path(&raw)?;

    let mut dir = mount.join(rel.trim_start_matches('/'));
    let mut tightest: Option<MemorySnapshot> = None;

    loop {
        if let Some(snapshot) = read_cgroup_dir(&dir)
            && tightest.is_none_or(|best| snapshot.available_mb < best.available_mb)
        {
            tightest = Some(snapshot);
        }
        if dir == mount {
            break;
        }
        match dir.parent() {
            Some(parent) if parent.starts_with(mount) => dir = parent.to_path_buf(),
            _ => break,
        }
    }

    tightest
}

/// The unified-hierarchy line of `/proc/self/cgroup` (`0::/some/path`).
/// cgroup v1-only systems have no such line and yield `None`.
fn parse_cgroup_v2_path(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(|path| path.trim().to_string())
}

/// `memory.max` / `memory.current` for one cgroup directory. `None` when the
/// cgroup is unlimited (`max`) or the files are missing.
fn read_cgroup_dir(dir: &Path) -> Option<MemorySnapshot> {
    let max = std::fs::read_to_string(dir.join("memory.max")).ok()?;
    let limit_bytes = parse_cgroup_limit(&max)?;
    let current = std::fs::read_to_string(dir.join("memory.current")).ok()?;
    let used_bytes = current.trim().parse::<u64>().ok()?;

    Some(MemorySnapshot {
        available_mb: limit_bytes.saturating_sub(used_bytes) / (KIB_PER_MIB * KIB_PER_MIB),
        total_mb: limit_bytes / (KIB_PER_MIB * KIB_PER_MIB),
        swap_free_mb: None,
        swap_total_mb: None,
        source: MemorySource::CgroupV2,
    })
}

fn parse_cgroup_limit(raw: &str) -> Option<u64> {
    match raw.trim() {
        "max" => None,
        value => value.parse::<u64>().ok(),
    }
}

// -------------------------------------------------------------------- macOS

#[cfg(target_os = "macos")]
fn probe_macos() -> Option<MemorySnapshot> {
    use std::process::Command;

    let total_bytes = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().ok())?;

    let vm_stat = Command::new("vm_stat")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())?;
    let available_bytes = parse_vm_stat_available_bytes(&vm_stat)?;

    let swap = Command::new("sysctl")
        .args(["-n", "vm.swapusage"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .and_then(|raw| parse_swapusage_mb(&raw));

    Some(MemorySnapshot {
        available_mb: available_bytes / (KIB_PER_MIB * KIB_PER_MIB),
        total_mb: total_bytes / (KIB_PER_MIB * KIB_PER_MIB),
        swap_free_mb: swap.map(|(_, free)| free),
        swap_total_mb: swap.map(|(total, _)| total),
        source: MemorySource::MacOs,
    })
}

/// Reclaimable bytes from `vm_stat` output: free, inactive, speculative, and
/// purgeable pages are all memory a new process can be given without paging
/// anything out. `None` if the page size or the free-page line is missing,
/// which is what makes macOS fall back to the concurrency limit alone.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_vm_stat_available_bytes(raw: &str) -> Option<u64> {
    let page_size = raw
        .lines()
        .next()
        .and_then(|line| line.split("page size of ").nth(1))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())?;

    let pages = |label: &str| -> Option<u64> {
        raw.lines()
            .find(|line| line.starts_with(label))
            .and_then(|line| line.split(':').nth(1))
            .map(|v| v.trim().trim_end_matches('.'))
            .and_then(|v| v.parse::<u64>().ok())
    };

    let free = pages("Pages free")?;
    let inactive = pages("Pages inactive").unwrap_or(0);
    let speculative = pages("Pages speculative").unwrap_or(0);
    let purgeable = pages("Pages purgeable").unwrap_or(0);

    Some((free + inactive + speculative + purgeable) * page_size)
}

/// `(total_mb, free_mb)` from `sysctl -n vm.swapusage`, whose values look like
/// `total = 2048.00M  used = 512.00M  free = 1536.00M`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_swapusage_mb(raw: &str) -> Option<(u64, u64)> {
    let field = |label: &str| -> Option<u64> {
        let rest = raw.split(label).nth(1)?;
        let value = rest.split_whitespace().next()?;
        let (number, unit) = value.split_at(value.len().checked_sub(1)?);
        let number: f64 = number.parse().ok()?;
        let mb = match unit {
            "M" => number,
            "G" => number * 1024.0,
            "K" => number / 1024.0,
            _ => return None,
        };
        Some(mb.round() as u64)
    };
    Some((field("total = ")?, field("free = ")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "\
MemTotal:        7736128 kB
MemFree:         5488640 kB
MemAvailable:    6413312 kB
Buffers:           41984 kB
Cached:          1064960 kB
SwapCached:            0 kB
SwapTotal:       2097152 kB
SwapFree:        2097152 kB
";

    #[test]
    fn parses_proc_meminfo_into_mib() {
        let snapshot = parse_proc_meminfo(MEMINFO).unwrap();
        assert_eq!(snapshot.available_mb, 6263);
        assert_eq!(snapshot.total_mb, 7554);
        assert_eq!(snapshot.swap_total_mb, Some(2048));
        assert_eq!(snapshot.swap_free_mb, Some(2048));
        assert_eq!(snapshot.source, MemorySource::ProcMeminfo);
    }

    #[test]
    fn falls_back_to_free_plus_cache_without_mem_available() {
        let raw = "MemTotal:        1048576 kB\nMemFree:          524288 kB\nCached:           262144 kB\n";
        let snapshot = parse_proc_meminfo(raw).unwrap();
        assert_eq!(snapshot.available_mb, 768);
        assert_eq!(snapshot.swap_total_mb, None);
    }

    #[test]
    fn rejects_meminfo_without_a_total() {
        assert!(parse_proc_meminfo("MemAvailable:  100 kB\n").is_none());
    }

    #[test]
    fn is_low_compares_against_threshold() {
        let snapshot = parse_proc_meminfo(MEMINFO).unwrap();
        assert!(!snapshot.is_low(1536));
        assert!(snapshot.is_low(8000));
    }

    #[test]
    fn parses_unified_cgroup_path() {
        let raw = "0::/user.slice/user-1000.slice/session-3.scope\n";
        assert_eq!(
            parse_cgroup_v2_path(raw).unwrap(),
            "/user.slice/user-1000.slice/session-3.scope"
        );
        // cgroup v1 output has no `0::` line.
        assert!(parse_cgroup_v2_path("11:memory:/docker/abc\n").is_none());
    }

    #[test]
    fn unlimited_cgroup_reports_no_limit() {
        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(parse_cgroup_limit("2147483648\n"), Some(2147483648));
    }

    #[test]
    fn cgroup_walk_takes_the_tightest_ancestor_limit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cgroup");
        let leaf = mount.join("user.slice/session.scope");
        std::fs::create_dir_all(&leaf).unwrap();

        // Leaf is unlimited; the parent caps at 1 GiB with 512 MiB used.
        std::fs::write(leaf.join("memory.max"), "max\n").unwrap();
        std::fs::write(leaf.join("memory.current"), "1048576\n").unwrap();
        let parent = mount.join("user.slice");
        std::fs::write(parent.join("memory.max"), "1073741824\n").unwrap();
        std::fs::write(parent.join("memory.current"), "536870912\n").unwrap();

        let self_cgroup = tmp.path().join("self-cgroup");
        std::fs::write(&self_cgroup, "0::/user.slice/session.scope\n").unwrap();

        let snapshot = probe_cgroup_v2(&self_cgroup, &mount).unwrap();
        assert_eq!(snapshot.total_mb, 1024);
        assert_eq!(snapshot.available_mb, 512);
        assert_eq!(snapshot.source, MemorySource::CgroupV2);
    }

    #[test]
    fn cgroup_walk_is_none_when_nothing_is_limited() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mount = tmp.path().join("cgroup");
        let leaf = mount.join("user.slice");
        std::fs::create_dir_all(&leaf).unwrap();
        std::fs::write(leaf.join("memory.max"), "max\n").unwrap();
        std::fs::write(leaf.join("memory.current"), "1024\n").unwrap();

        let self_cgroup = tmp.path().join("self-cgroup");
        std::fs::write(&self_cgroup, "0::/user.slice\n").unwrap();

        assert!(probe_cgroup_v2(&self_cgroup, &mount).is_none());
    }

    #[test]
    fn parses_vm_stat_reclaimable_pages() {
        let raw = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               65536.
Pages active:                            131072.
Pages inactive:                           32768.
Pages speculative:                         8192.
Pages purgeable:                           4096.
";
        let bytes = parse_vm_stat_available_bytes(raw).unwrap();
        assert_eq!(bytes, (65536 + 32768 + 8192 + 4096) * 16384);
    }

    #[test]
    fn vm_stat_without_page_size_is_unusable() {
        assert!(parse_vm_stat_available_bytes("Pages free: 100.\n").is_none());
    }

    #[test]
    fn parses_swapusage() {
        let raw = "total = 2048.00M  used = 512.00M  free = 1536.00M  (encrypted)\n";
        assert_eq!(parse_swapusage_mb(raw), Some((2048, 1536)));
        assert_eq!(parse_swapusage_mb("total = 0.00M\n"), None);
    }

    #[test]
    fn probe_on_this_host_is_self_consistent() {
        let snapshot = probe();
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let snapshot = snapshot.expect("linux and macos must produce a snapshot");
            assert!(snapshot.total_mb > 0);
            assert!(snapshot.available_mb <= snapshot.total_mb);
        } else {
            // No usable signal here, which is what disables the memory gate.
            assert!(snapshot.is_none());
        }
    }
}
