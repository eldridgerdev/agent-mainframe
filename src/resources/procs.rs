//! Small read-mostly process helpers: who is running, who is whose child, and
//! how to end a process tree politely.
//!
//! Everything goes through `ps` (present on Linux and macOS) rather than
//! `/proc`, so the same code answers on both. Signals go through `libc`.

use std::path::Path;
use std::time::{Duration, Instant};

/// One process as `ps` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: i64,
    pub ppid: i64,
    /// Full command line, as `ps -o args` prints it.
    pub args: String,
}

/// Every process on the machine. Empty when `ps` is unavailable — callers
/// must treat that as "cannot tell", never as "nothing is running".
pub fn list_processes() -> Vec<ProcInfo> {
    let output = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,args="])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_ps_output(&String::from_utf8_lossy(&output.stdout))
}

fn parse_ps_output(raw: &str) -> Vec<ProcInfo> {
    raw.lines()
        .filter_map(|line| {
            // `ps` right-aligns the numeric columns, so fields are separated by
            // runs of spaces, not single ones.
            let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
            let (ppid, args) = rest.trim_start().split_once(char::is_whitespace)?;
            Some(ProcInfo {
                pid: pid.trim().parse().ok()?,
                ppid: ppid.trim().parse().ok()?,
                args: args.trim().to_string(),
            })
        })
        .collect()
}

/// Whether a PID currently exists. `kill(pid, 0)` also succeeds for zombies,
/// which is fine here: a zombie is still the process AMF started.
pub fn pid_alive(pid: i64) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs the permission/existence check without
    // delivering anything.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// The command line of one PID, or `None` if it is gone.
pub fn args_for_pid(pid: i64) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let args = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!args.is_empty()).then_some(args)
}

/// When a PID started, as an opaque string (`ps -o lstart=`), whitespace
/// normalized so two readings of the same process compare equal.
///
/// This is the half of process identity that argv cannot provide: argv is
/// whatever a process chooses to look like and can be reproduced exactly by a
/// later process on the same worktree, whereas the start time distinguishes
/// *that* process from the one holding the number now. `None` when the process
/// is gone or `ps` has no `lstart` (some busybox builds) — callers must treat
/// that as "cannot tell", never as a match.
pub fn start_time_for_pid(pid: i64) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(normalize_start_time(&String::from_utf8_lossy(
        &output.stdout,
    )))
    .filter(|s| !s.is_empty())
}

fn normalize_start_time(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `root` plus every descendant, depth-first, parents before children.
pub fn process_tree(processes: &[ProcInfo], root: i64) -> Vec<i64> {
    let mut tree = vec![root];
    let mut index = 0;
    while index < tree.len() {
        let parent = tree[index];
        for proc in processes {
            if proc.ppid == parent && !tree.contains(&proc.pid) {
                tree.push(proc.pid);
            }
        }
        index += 1;
    }
    tree
}

/// End a process tree: `SIGTERM` to everything, then `SIGKILL` to whatever is
/// still alive after `grace`. Children are signalled before their parents so a
/// supervisor cannot restart them mid-shutdown.
///
/// Returns the PIDs that were gone by the end.
pub fn terminate_tree(root: i64, grace: Duration) -> Vec<i64> {
    let processes = list_processes();
    let mut tree = process_tree(&processes, root);
    tree.reverse();

    for pid in &tree {
        signal(*pid, libc::SIGTERM);
    }

    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if tree.iter().all(|pid| !pid_alive(*pid)) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    for pid in &tree {
        if pid_alive(*pid) {
            signal(*pid, libc::SIGKILL);
        }
    }

    tree.into_iter().filter(|pid| !pid_alive(*pid)).collect()
}

fn signal(pid: i64, sig: libc::c_int) {
    if pid <= 0 {
        return;
    }
    // SAFETY: an invalid or exited PID just returns ESRCH.
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

/// Command names that mean "a VS Code main process" — the long-lived window
/// process, as opposed to the `code` shell wrapper that exits immediately.
const VSCODE_BINARIES: [&str; 6] = [
    "code",
    "code-insiders",
    "codium",
    "vscodium",
    "Code",
    "Code - Insiders",
];

/// Whether `args` looks like a VS Code process opened on `workdir`.
///
/// Both halves matter. The path alone matches shells, agents, and language
/// servers living in the worktree; the binary alone matches every VS Code
/// window on the machine. Renderer/utility subprocesses are excluded — they
/// are reached through the main process's tree, not signalled individually.
pub fn is_vscode_for_workdir(args: &str, workdir: &Path) -> bool {
    let Some(binary) = args.split_whitespace().next() else {
        return false;
    };
    let name = Path::new(binary)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if !VSCODE_BINARIES.contains(&name.as_str()) {
        return false;
    }
    if args.contains("--type=") {
        return false;
    }
    mentions_path(args, workdir.to_string_lossy().as_ref())
}

/// Whether `args` names `path` as a path, rather than merely containing its
/// characters.
///
/// A plain substring test makes `/repo/feature-two` an occurrence of
/// `/repo/feat`, which at kill time means signalling a window the user opened
/// on a neighbouring worktree. The match must therefore start and end on a path
/// boundary; a trailing `/` still counts, so a window opened on a directory
/// *inside* the worktree is still that worktree's window.
fn mentions_path(args: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let bytes = args.as_bytes();
    let mut from = 0;
    while let Some(offset) = args[from..].find(path) {
        let start = from + offset;
        let end = start + path.len();
        let starts_clean =
            start == 0 || matches!(bytes[start - 1], b' ' | b'\t' | b'"' | b'\'' | b'=');
        let ends_clean = match args[end..].chars().next() {
            None => true,
            Some(c) => c.is_whitespace() || c == '/' || c == '"' || c == '\'',
        };
        if starts_clean && ends_clean {
            return true;
        }
        // Step by one character (not one byte) so a multi-byte path stays on
        // char boundaries.
        from = start + args[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// How many editor windows a VS Code main process is currently hosting.
///
/// VS Code is a singleton application: `--new-window` gives AMF its own window,
/// but if that launch was the one that *started* VS Code, every window the user
/// opens afterwards lives in the same process tree. Killing the main process
/// then closes their work too, so ownership of the process is not on its own a
/// licence to end it — the window count is what says whether it is still only
/// AMF's.
///
/// Counted from Chromium renderer subprocesses, one per window (helpers such as
/// the GPU process, utility processes, and the zygote carry a different
/// `--type`). Returns `0` when nothing can be read, which callers must treat as
/// "cannot tell".
pub fn vscode_window_count(processes: &[ProcInfo], root: i64) -> usize {
    let tree = process_tree(processes, root);
    processes
        .iter()
        .filter(|proc| tree.contains(&proc.pid) && proc.args.contains("--type=renderer"))
        .count()
}

/// Find the VS Code window process AMF just opened on `workdir`.
///
/// `before` is the set of matching PIDs captured immediately *before* the
/// launch: the new window is whichever match was not already there. That
/// difference is the ownership proof — without it a launch that merely handed
/// the folder to a running instance would look identical to a fresh window.
///
/// Polls because VS Code takes seconds to come up, and gives up rather than
/// guessing: `None` means "not AMF's to kill".
pub fn find_new_vscode_window(
    workdir: &Path,
    before: &[i64],
    timeout: Duration,
    poll: Duration,
) -> Option<ProcInfo> {
    let deadline = Instant::now() + timeout;
    loop {
        let found = list_processes()
            .into_iter()
            .find(|proc| !before.contains(&proc.pid) && is_vscode_for_workdir(&proc.args, workdir));
        if found.is_some() {
            return found;
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(poll);
    }
}

/// PIDs of VS Code windows already open on `workdir`.
pub fn existing_vscode_windows(workdir: &Path) -> Vec<i64> {
    list_processes()
        .into_iter()
        .filter(|proc| is_vscode_for_workdir(&proc.args, workdir))
        .map(|proc| proc.pid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_ps_output_with_spaces_in_args() {
        let raw = "  123   1 /usr/share/code/code --new-window /home/me/my worktree\n\
                     456 123 /usr/share/code/code --type=renderer\n";
        let procs = parse_ps_output(raw);
        assert_eq!(procs.len(), 2);
        assert_eq!(procs[0].pid, 123);
        assert_eq!(procs[0].ppid, 1);
        assert_eq!(
            procs[0].args,
            "/usr/share/code/code --new-window /home/me/my worktree"
        );
        assert_eq!(procs[1].ppid, 123);
    }

    #[test]
    fn ignores_unparseable_ps_lines() {
        assert!(parse_ps_output("garbage\n\n").is_empty());
    }

    #[test]
    fn process_tree_collects_descendants() {
        let procs = vec![
            ProcInfo {
                pid: 1,
                ppid: 0,
                args: "init".into(),
            },
            ProcInfo {
                pid: 10,
                ppid: 1,
                args: "code".into(),
            },
            ProcInfo {
                pid: 11,
                ppid: 10,
                args: "renderer".into(),
            },
            ProcInfo {
                pid: 12,
                ppid: 11,
                args: "rust-analyzer".into(),
            },
            ProcInfo {
                pid: 20,
                ppid: 1,
                args: "unrelated".into(),
            },
        ];
        let tree = process_tree(&procs, 10);
        assert_eq!(tree, vec![10, 11, 12]);
        assert!(!tree.contains(&20), "siblings are not part of the tree");
    }

    #[test]
    fn process_tree_of_an_unknown_pid_is_just_itself() {
        assert_eq!(process_tree(&[], 999), vec![999]);
    }

    #[test]
    fn recognizes_a_vscode_window_on_the_worktree() {
        let wt = PathBuf::from("/home/me/repo/.worktrees/feat");
        assert!(is_vscode_for_workdir(
            "/usr/share/code/code --new-window /home/me/repo/.worktrees/feat",
            &wt
        ));
        assert!(is_vscode_for_workdir(
            "/opt/vscodium-bin/codium /home/me/repo/.worktrees/feat",
            &wt
        ));
    }

    #[test]
    fn does_not_mistake_neighbours_for_a_vscode_window() {
        let wt = PathBuf::from("/home/me/repo/.worktrees/feat");
        // A window on a different worktree.
        assert!(!is_vscode_for_workdir(
            "/usr/share/code/code /home/me/repo/.worktrees/other",
            &wt
        ));
        // Something else entirely running inside the worktree — a shell, an
        // agent, a language server — must never be taken for the editor.
        assert!(!is_vscode_for_workdir(
            "rust-analyzer /home/me/repo/.worktrees/feat",
            &wt
        ));
        assert!(!is_vscode_for_workdir(
            "/bin/zsh -c cd /home/me/repo/.worktrees/feat",
            &wt
        ));
        // Electron helper processes are reached via the tree, not directly.
        assert!(!is_vscode_for_workdir(
            "/usr/share/code/code --type=renderer /home/me/repo/.worktrees/feat",
            &wt
        ));
    }

    #[test]
    fn a_neighbouring_worktree_is_not_this_worktree() {
        // The prefix case: `/repo/feat` is a substring of `/repo/feature-two`,
        // and a substring test would hand AMF a licence to kill the user's
        // window on the neighbouring worktree.
        let wt = PathBuf::from("/home/me/repo/.worktrees/feat");
        assert!(!is_vscode_for_workdir(
            "/usr/share/code/code /home/me/repo/.worktrees/feature-two",
            &wt
        ));
        // A different repo whose path merely ends with ours.
        assert!(!is_vscode_for_workdir(
            "/usr/share/code/code /mnt/backup/home/me/repo/.worktrees/feat",
            &wt
        ));
        // Quoted, and a directory inside the worktree, both still count.
        assert!(is_vscode_for_workdir(
            "/usr/share/code/code \"/home/me/repo/.worktrees/feat\"",
            &wt
        ));
        assert!(is_vscode_for_workdir(
            "/usr/share/code/code /home/me/repo/.worktrees/feat/src",
            &wt
        ));
    }

    #[test]
    fn counts_the_windows_a_vscode_instance_is_hosting() {
        let procs = vec![
            ProcInfo {
                pid: 10,
                ppid: 1,
                args: "/usr/share/code/code --new-window /wt".into(),
            },
            ProcInfo {
                pid: 11,
                ppid: 10,
                args: "/usr/share/code/code --type=zygote".into(),
            },
            ProcInfo {
                pid: 12,
                ppid: 11,
                args: "/usr/share/code/code --type=renderer --window-id=1".into(),
            },
            ProcInfo {
                pid: 13,
                ppid: 10,
                args: "/usr/share/code/code --type=gpu-process".into(),
            },
            ProcInfo {
                pid: 14,
                ppid: 10,
                args: "/usr/share/code/code --type=utility".into(),
            },
        ];
        assert_eq!(vscode_window_count(&procs, 10), 1);

        // A second window opened by the user in the same instance: renderers
        // are forked from the zygote, so they are descendants either way.
        let mut shared = procs.clone();
        shared.push(ProcInfo {
            pid: 15,
            ppid: 11,
            args: "/usr/share/code/code --type=renderer --window-id=2".into(),
        });
        assert_eq!(vscode_window_count(&shared, 10), 2);
    }

    #[test]
    fn window_count_of_an_unreadable_process_list_is_zero() {
        // "Cannot tell", which callers must not read as "no other windows".
        assert_eq!(vscode_window_count(&[], 10), 0);
    }

    #[test]
    fn start_time_is_stable_and_absent_for_dead_pids() {
        let me = std::process::id() as i64;
        let first = start_time_for_pid(me);
        assert!(first.is_some(), "this process should have a start time");
        assert_eq!(first, start_time_for_pid(me), "start time must not drift");
        assert!(start_time_for_pid(0).is_none());
        assert!(start_time_for_pid(-1).is_none());
    }

    #[test]
    fn start_times_normalize_ps_column_padding() {
        assert_eq!(
            normalize_start_time("  Sat Aug  9 12:00:00 2026\n"),
            "Sat Aug 9 12:00:00 2026"
        );
    }

    #[test]
    fn pid_zero_and_negatives_are_never_alive() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
    }

    #[test]
    fn this_process_is_alive_and_has_args() {
        let me = std::process::id() as i64;
        assert!(pid_alive(me));
        assert!(args_for_pid(me).is_some());
    }

    /// Stand-in for the editor: a copy of `sh` named `code`, launched with a
    /// VS Code-shaped argv, that stays alive like a real window would. Real
    /// VS Code cannot be driven from a test, but the ownership resolution this
    /// exercises is the part that has to be right.
    fn spawn_fake_vscode_window(dir: &Path, workdir: &Path) -> std::process::Child {
        let fake = dir.join("code");
        std::fs::copy("/bin/sh", &fake).expect("copy sh");
        std::process::Command::new(&fake)
            .args([
                "-c".as_ref(),
                "sleep 60".as_ref(),
                "--new-window".as_ref(),
                workdir.as_os_str(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("fake code should launch")
    }

    #[test]
    fn finds_the_window_a_launch_just_opened() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().join("worktree");
        std::fs::create_dir_all(&workdir).unwrap();

        let before = existing_vscode_windows(&workdir);
        let mut child = spawn_fake_vscode_window(tmp.path(), &workdir);

        let found = find_new_vscode_window(
            &workdir,
            &before,
            Duration::from_secs(5),
            Duration::from_millis(50),
        );

        let found = found.expect("the launched window should be attributable");
        assert_eq!(found.pid, child.id() as i64);
        assert!(found.args.contains("--new-window"));

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn does_not_claim_a_window_that_was_already_open() {
        // The reused-instance case: a window on this worktree existed before
        // the launch, and nothing new appeared. Claiming it would mean closing
        // a window the user opened.
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().join("worktree");
        std::fs::create_dir_all(&workdir).unwrap();

        let mut child = spawn_fake_vscode_window(tmp.path(), &workdir);
        // Let it appear in `ps` before the snapshot.
        std::thread::sleep(Duration::from_millis(200));
        let before = existing_vscode_windows(&workdir);
        assert!(
            before.contains(&(child.id() as i64)),
            "the pre-existing window should be in the snapshot"
        );

        let found = find_new_vscode_window(
            &workdir,
            &before,
            Duration::from_millis(400),
            Duration::from_millis(50),
        );
        assert!(
            found.is_none(),
            "a pre-existing window is not AMF's to kill"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn terminate_tree_ends_a_real_process_tree() {
        // A shell that spawns a child and waits: the child is only reachable
        // through the tree walk, which is exactly what has to work for a
        // language server under an editor.
        let mut parent = std::process::Command::new("sh")
            // `& wait` keeps the shell itself alive as the parent instead of
            // exec'ing away, so there is a real tree to walk.
            .args(["-c", "sleep 60 & wait"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("sh should be available");
        let root = parent.id() as i64;
        // Give the shell a moment to fork its children.
        std::thread::sleep(Duration::from_millis(300));
        let children = process_tree(&list_processes(), root);
        assert!(
            children.len() > 1,
            "expected child processes, got {children:?}"
        );

        terminate_tree(root, Duration::from_secs(2));
        let _ = parent.wait();

        for pid in children.iter().skip(1) {
            assert!(!pid_alive(*pid), "pid {pid} survived the tree termination");
        }
    }
}
