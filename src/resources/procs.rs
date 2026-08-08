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
    let path = workdir.to_string_lossy();
    !path.is_empty() && args.contains(path.as_ref())
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
        let found = list_processes().into_iter().find(|proc| {
            !before.contains(&proc.pid) && is_vscode_for_workdir(&proc.args, workdir)
        });
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
            ProcInfo { pid: 1, ppid: 0, args: "init".into() },
            ProcInfo { pid: 10, ppid: 1, args: "code".into() },
            ProcInfo { pid: 11, ppid: 10, args: "renderer".into() },
            ProcInfo { pid: 12, ppid: 11, args: "rust-analyzer".into() },
            ProcInfo { pid: 20, ppid: 1, args: "unrelated".into() },
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
        assert!(found.is_none(), "a pre-existing window is not AMF's to kill");

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
        assert!(children.len() > 1, "expected child processes, got {children:?}");

        terminate_tree(root, Duration::from_secs(2));
        let _ = parent.wait();

        for pid in children.iter().skip(1) {
            assert!(!pid_alive(*pid), "pid {pid} survived the tree termination");
        }
    }
}
