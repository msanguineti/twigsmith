//! Native completion observations while the caller retains the unreaped leader.
use rustix::process::Pid;
#[cfg(any(target_os = "macos", test))]
use std::collections::HashSet;
use std::{io, time::Instant};

pub(super) fn ensure_supported() -> io::Result<()> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "managed checks require Linux or macOS process-group observation on Unix",
        ))
    }
}

fn check_deadline(deadline: Instant) -> io::Result<()> {
    if Instant::now() >= deadline {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "check process-group observation exceeded the cleanup deadline",
        ))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Identity {
    pid: i32,
    start: (u64, u64),
}

#[derive(Clone, Copy, Debug)]
struct Member {
    identity: Identity,
    live: bool,
}

#[derive(Default)]
struct Observation {
    members: Vec<Member>,
    changed: bool,
}

impl Observation {
    fn has_live_members(&self, group: Pid, deadline: Instant) -> io::Result<bool> {
        check_deadline(deadline)?;
        let mut leader_seen = false;
        let mut live = self.changed;
        for member in &self.members {
            check_deadline(deadline)?;
            leader_seen |= member.identity.pid == group.as_raw_pid();
            live |= member.live;
        }
        if !leader_seen {
            return Err(io::Error::other(format!(
                "retained check leader {group} missing from process-group observation"
            )));
        }
        Ok(live)
    }
}

/// The caller must retain the unreaped leader and have sent SIGKILL to the group.
/// On Darwin a second complete enumeration closes the list/status fork race:
/// every final identity must have been observed terminal before that list began.
/// The deadline bounds our iteration; a native syscall cannot be preempted here.
pub(super) fn has_live_members(group: Pid, deadline: Instant) -> io::Result<bool> {
    ensure_supported()?;
    check_deadline(deadline)?;
    let first = observe(group, deadline)?;
    #[cfg(not(target_os = "macos"))]
    return first.has_live_members(group, deadline);

    #[cfg(target_os = "macos")]
    {
        if first.has_live_members(group, deadline)? {
            return Ok(true);
        }
        check_deadline(deadline)?;
        let second = observe(group, deadline)?;
        confirm_terminal_members(group, &first, &second, deadline)
    }
}

#[cfg(any(target_os = "macos", test))]
fn confirm_terminal_members(
    group: Pid,
    first: &Observation,
    second: &Observation,
    deadline: Instant,
) -> io::Result<bool> {
    if first.has_live_members(group, deadline)? || second.has_live_members(group, deadline)? {
        return Ok(true);
    }
    let mut terminal = HashSet::new();
    for member in &first.members {
        check_deadline(deadline)?;
        terminal.insert(member.identity);
    }
    for member in &second.members {
        check_deadline(deadline)?;
        if !terminal.contains(&member.identity) {
            return Ok(true);
        }
    }
    check_deadline(deadline)?;
    Ok(false)
}

#[cfg(target_os = "linux")]
fn observe(group: Pid, deadline: Instant) -> io::Result<Observation> {
    linux::observe(std::path::Path::new("/proc"), group, deadline)
}

#[cfg(target_os = "macos")]
fn observe(group: Pid, deadline: Instant) -> io::Result<Observation> {
    macos::observe(group, deadline)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn observe(_: Pid, _: Instant) -> io::Result<Observation> {
    ensure_supported()?;
    unreachable!("unsupported Unix observer")
}

#[cfg(any(target_os = "linux", test))]
mod linux {
    use super::*;
    use std::{fs, path::Path};

    #[derive(Debug)]
    struct Stat {
        pid: i32,
        group: i32,
        state: u8,
        threads: u64,
        start: u64,
    }

    fn invalid_stat() -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, "invalid /proc process stat")
    }

    fn number<T: std::str::FromStr>(bytes: &[u8]) -> io::Result<T> {
        std::str::from_utf8(bytes)
            .ok()
            .and_then(|value| value.parse().ok())
            .ok_or_else(invalid_stat)
    }

    fn parse_stat(bytes: &[u8]) -> io::Result<Stat> {
        // comm is raw bytes and may itself contain spaces and parentheses.
        // All subsequent fields are numbers or the single-byte state.
        let end = bytes
            .iter()
            .rposition(|&byte| byte == b')')
            .ok_or_else(invalid_stat)?;
        let begin = bytes
            .iter()
            .position(|&byte| byte == b'(')
            .ok_or_else(invalid_stat)?;
        if begin < 2 || end <= begin || bytes.get(begin - 1) != Some(&b' ') {
            return Err(invalid_stat());
        }
        let pid: i32 = number(&bytes[..begin - 1])?;
        if pid <= 0 || bytes.get(end + 1) != Some(&b' ') {
            return Err(invalid_stat());
        }
        let mut fields = bytes[end + 2..]
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty());
        let state = fields.next().ok_or_else(invalid_stat)?;
        if state.len() != 1 {
            return Err(invalid_stat());
        }
        fields.next().ok_or_else(invalid_stat)?; // field 4: ppid
        let group = number(fields.next().ok_or_else(invalid_stat)?)?; // field 5
        let threads = number(fields.nth(14).ok_or_else(invalid_stat)?)?; // field 20
        fields.next().ok_or_else(invalid_stat)?; // field 21: itrealvalue
        let start = number(fields.next().ok_or_else(invalid_stat)?)?; // field 22
        Ok(Stat {
            pid,
            group,
            state: state[0],
            threads,
            start,
        })
    }

    fn parse_stat_read(read: io::Result<Vec<u8>>) -> io::Result<Option<Stat>> {
        match read {
            Ok(bytes) => parse_stat(&bytes).map(Some),
            // A task can disappear after path lookup but before proc_single_show
            // acquires it; Linux reports ESRCH at that read boundary.
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error()) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn observe(root: &Path, group: Pid, deadline: Instant) -> io::Result<Observation> {
        check_deadline(deadline)?;
        let entries = fs::read_dir(root);
        check_deadline(deadline)?;
        let mut entries = entries?;
        let mut observation = Observation::default();
        // Do not stop at a live member: enumeration and retained-leader identity
        // must still be checked. Only a concurrently vanished entry is harmless.
        loop {
            check_deadline(deadline)?;
            let entry = entries.next();
            check_deadline(deadline)?;
            let Some(entry) = entry else { break };
            let entry = entry?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let pid: i32 = name.parse().map_err(|_| invalid_stat())?;
            check_deadline(deadline)?;
            let read = fs::read(entry.path().join("stat"));
            check_deadline(deadline)?;
            let Some(stat) = parse_stat_read(read)? else {
                continue;
            };
            if stat.pid != pid || (pid == group.as_raw_pid() && stat.group != pid) {
                return Err(io::Error::other(
                    "process identity changed during /proc observation",
                ));
            }
            if stat.group == group.as_raw_pid() {
                observation.members.push(Member {
                    identity: Identity {
                        pid,
                        start: (stat.start, 0),
                    },
                    // A zombie main thread can still have executing workers.
                    live: !matches!(stat.state, b'Z' | b'X' | b'x') || stat.threads > 1,
                });
            }
        }
        Ok(observation)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn stat(pid: i32, group: i32, name: &[u8], state: u8, threads: u64) -> Vec<u8> {
            let mut bytes = format!("{pid} (").into_bytes();
            bytes.extend(name);
            bytes.extend(
                format!(
                    ") {} 1 {group} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {threads} 0 42\n",
                    state as char
                )
                .bytes(),
            );
            bytes
        }

        #[test]
        fn parses_raw_names_and_the_final_parenthesis() {
            let parsed = parse_stat(&stat(17, 17, b"a (name) ) \xff", b'Z', 3)).unwrap();
            assert_eq!(
                (
                    parsed.pid,
                    parsed.group,
                    parsed.state,
                    parsed.threads,
                    parsed.start
                ),
                (17, 17, b'Z', 3, 42)
            );
            for bytes in [
                b"17 (broken".as_slice(),
                b"17 (name) Z 1 17",
                b"17 (name) ZZ 1 17",
                b"x (name) Z 1 17",
            ] {
                assert!(parse_stat(bytes).is_err());
            }
        }

        #[test]
        fn vanished_proc_reads_allow_enoent_and_esrch_but_preserve_other_errors() {
            for error in [rustix::io::Errno::NOENT, rustix::io::Errno::SRCH] {
                assert!(parse_stat_read(Err(error.into())).unwrap().is_none());
            }
            let error = parse_stat_read(Err(rustix::io::Errno::ACCESS.into())).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }

        #[test]
        fn expired_budget_refuses_before_enumeration() {
            let root = tempfile::tempdir().unwrap();
            let error = observe(
                &root.path().join("missing"),
                Pid::from_raw(17).unwrap(),
                Instant::now(),
            )
            .err()
            .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }

        #[test]
        fn complete_proc_scan_checks_identity_workers_and_read_errors() {
            let root = tempfile::tempdir().unwrap();
            let leader = Pid::from_raw(17).unwrap();
            fs::create_dir(root.path().join("17")).unwrap();
            fs::create_dir(root.path().join("18")).unwrap(); // vanished stat
            let path = root.path().join("17/stat");
            for state in *b"ZXx" {
                fs::write(&path, stat(17, 17, b"leader", state, 1)).unwrap();
                assert!(
                    !observe(root.path(), leader, test_deadline())
                        .unwrap()
                        .has_live_members(leader, test_deadline())
                        .unwrap()
                );
                fs::write(&path, stat(17, 17, b"leader", state, 2)).unwrap();
                assert!(
                    observe(root.path(), leader, test_deadline())
                        .unwrap()
                        .has_live_members(leader, test_deadline())
                        .unwrap()
                );
            }
            for state in *b"RSDTtI?" {
                fs::write(&path, stat(17, 17, b"leader", state, 1)).unwrap();
                assert!(
                    observe(root.path(), leader, test_deadline())
                        .unwrap()
                        .has_live_members(leader, test_deadline())
                        .unwrap()
                );
            }
            for (pid, group) in [(18, 17), (17, 18)] {
                fs::write(&path, stat(pid, group, b"leader", b'Z', 1)).unwrap();
                assert!(observe(root.path(), leader, test_deadline()).is_err());
            }
            fs::remove_file(&path).unwrap();
            assert!(
                observe(root.path(), leader, test_deadline())
                    .unwrap()
                    .has_live_members(leader, test_deadline())
                    .is_err()
            );
            fs::create_dir(&path).unwrap();
            assert!(observe(root.path(), leader, test_deadline()).is_err());
            assert!(observe(&root.path().join("missing"), leader, test_deadline()).is_err());
        }
    }
}

#[cfg(any(target_os = "macos", test))]
mod macos {
    use super::*;

    const MAX_MEMBERS: usize = 65_536;

    fn list_members(
        deadline: Instant,
        mut list: impl FnMut(&mut [i32]) -> io::Result<usize>,
    ) -> io::Result<Vec<i32>> {
        check_deadline(deadline)?;
        let mut members = vec![0; 32];
        loop {
            check_deadline(deadline)?;
            let count = list(&mut members);
            check_deadline(deadline)?;
            let count = count?;
            if count == 0 || count > members.len() {
                return Err(io::Error::other(
                    "invalid native process-group member count",
                ));
            }
            if count < members.len() {
                members.truncate(count);
                let mut seen = HashSet::new();
                for &pid in &members {
                    check_deadline(deadline)?;
                    if pid <= 0 || !seen.insert(pid) {
                        return Err(io::Error::other("invalid native process-group member list"));
                    }
                }
                return Ok(members);
            }
            if members.len() == MAX_MEMBERS {
                return Err(io::Error::other(
                    "check process-group member list exceeds observation bound",
                ));
            }
            members.resize(members.len() * 2, 0);
        }
    }

    fn member(
        pid: i32,
        group: Pid,
        observed_pid: u32,
        observed_group: u32,
        status: u32,
        zombie: u32,
        start: (u64, u64),
    ) -> io::Result<Member> {
        if observed_pid != pid as u32 || observed_group != group.as_raw_pid() as u32 {
            return Err(io::Error::other(
                "process identity changed during native group observation",
            ));
        }
        Ok(Member {
            identity: Identity { pid, start },
            live: status != zombie,
        })
    }

    #[cfg(target_os = "macos")]
    pub(super) fn observe(group: Pid, deadline: Instant) -> io::Result<Observation> {
        let members = list_members(deadline, |buffer| {
            // SAFETY: writable aligned PID array and its checked byte length.
            // libproc returns a PID count, unlike proc_listpids' byte count.
            let count = unsafe {
                libc::proc_listpgrppids(
                    group.as_raw_pid(),
                    buffer.as_mut_ptr().cast(),
                    std::mem::size_of_val(buffer) as i32,
                )
            };
            if count <= 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(count as usize)
        })?;
        let mut observation = Observation::default();
        for pid in members {
            check_deadline(deadline)?;
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
            // SAFETY: correctly sized/aligned output storage; it is read only
            // after an exact-size success. arg=1 includes retained zombies.
            let bytes = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    1,
                    info.as_mut_ptr().cast(),
                    size,
                )
            };
            // Capture errno before another operation, including the clock read.
            let error = if bytes <= 0 {
                Some(io::Error::last_os_error())
            } else {
                None
            };
            check_deadline(deadline)?;
            if let Some(error) = error {
                if error.raw_os_error() == Some(libc::ESRCH) {
                    observation.changed = true;
                    continue;
                }
                return Err(error);
            }
            if bytes != size {
                return Err(io::Error::other("incomplete native process status"));
            }
            // SAFETY: proc_pidinfo filled exactly this structure's byte size.
            let info = unsafe { info.assume_init() };
            observation.members.push(member(
                pid,
                group,
                info.pbi_pid,
                info.pbi_pgid,
                info.pbi_status,
                libc::SZOMB,
                (info.pbi_start_tvsec, info.pbi_start_tvusec),
            )?);
        }
        check_deadline(deadline)?;
        Ok(observation)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn expired_budget_stops_native_list_before_calling_it() {
            let mut called = false;
            let error = list_members(Instant::now(), |_| {
                called = true;
                Ok(0)
            })
            .unwrap_err();
            assert!(!called);
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }

        #[test]
        fn list_growth_never_retries_after_budget_expiration() {
            let deadline = Instant::now() + std::time::Duration::from_millis(5);
            let mut calls = 0;
            let error = list_members(deadline, |buffer| {
                calls += 1;
                if calls == 1 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Ok(buffer.len())
            })
            .unwrap_err();
            assert!(calls <= 1);
            assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        }

        #[test]
        fn member_list_grows_until_complete_and_fails_closed() {
            let mut calls = 0;
            let members = list_members(test_deadline(), |buffer| {
                calls += 1;
                if buffer.len() == 32 {
                    return Ok(32);
                }
                buffer[..2].copy_from_slice(&[17, 18]);
                Ok(2)
            })
            .unwrap();
            assert_eq!((calls, members), (2, vec![17, 18]));
            for count in [0, 33] {
                assert!(list_members(test_deadline(), |_| Ok(count)).is_err());
            }
            assert!(list_members(test_deadline(), |buffer| Ok(buffer.len())).is_err());
            assert!(
                list_members(test_deadline(), |_| Err(io::Error::from(
                    io::ErrorKind::PermissionDenied
                )))
                .is_err()
            );
            for values in [[0, 17], [-1, 17], [17, 17]] {
                assert!(
                    list_members(test_deadline(), |buffer| {
                        buffer[..2].copy_from_slice(&values);
                        Ok(2)
                    })
                    .is_err()
                );
            }
        }

        #[test]
        fn member_status_requires_exact_identity_and_only_zombies_are_terminal() {
            let group = Pid::from_raw(17).unwrap();
            for status in [0, 1, 2, 3, 4, 6, u32::MAX] {
                assert!(member(18, group, 18, 17, status, 5, (1, 2)).unwrap().live);
            }
            assert!(!member(18, group, 18, 17, 5, 5, (1, 2)).unwrap().live);
            assert!(member(18, group, 19, 17, 5, 5, (1, 2)).is_err());
            assert!(member(18, group, 18, 19, 5, 5, (1, 2)).is_err());
        }
    }
}

#[cfg(test)]
fn test_deadline() -> Instant {
    Instant::now() + std::time::Duration::from_secs(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(members: &[(i32, u64, bool)]) -> Observation {
        Observation {
            members: members
                .iter()
                .map(|&(pid, start, live)| Member {
                    identity: Identity {
                        pid,
                        start: (start, 0),
                    },
                    live,
                })
                .collect(),
            changed: false,
        }
    }

    #[test]
    fn completion_requires_two_complete_terminal_identity_observations() {
        let group = Pid::from_raw(17).unwrap();
        let first = observation(&[(17, 1, false), (18, 2, false)]);
        assert!(
            !confirm_terminal_members(
                group,
                &first,
                &observation(&[(17, 1, false)]),
                test_deadline()
            )
            .unwrap()
        );
        for second in [
            observation(&[(17, 1, false), (19, 3, false)]), // newly forked identity
            observation(&[(17, 1, false), (18, 3, false)]), // reused member PID
            observation(&[(17, 1, false), (18, 2, true)]),
            Observation {
                changed: true,
                ..observation(&[(17, 1, false)])
            },
        ] {
            assert!(confirm_terminal_members(group, &first, &second, test_deadline()).unwrap());
        }
        let missing = observation(&[(18, 2, false)]);
        assert!(confirm_terminal_members(group, &missing, &first, test_deadline()).is_err());
        assert!(confirm_terminal_members(group, &first, &missing, test_deadline()).is_err());
        assert!(
            confirm_terminal_members(
                group,
                &observation(&[(17, 1, true)]),
                &first,
                test_deadline()
            )
            .unwrap()
        );
        let error = confirm_terminal_members(group, &first, &first, Instant::now()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }
}
