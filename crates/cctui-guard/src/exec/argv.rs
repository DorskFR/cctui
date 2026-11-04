//! An exec's path and argv, read out of the target's memory.

use std::io;

pub const MAX_PATH: usize = 4096;
/// Past these limits the exec is denied as uninspectable.
pub const MAX_ARGS: usize = 65_536;
pub const MAX_ARGV_BYTES: usize = 4 << 20;
pub const MAX_DISPLAY: usize = 240;

const CHUNK: u64 = 4096;

pub trait Memory {
    fn read_at(&self, addr: u64, buf: &mut [u8]) -> io::Result<usize>;
}

impl Memory for std::fs::File {
    fn read_at(&self, addr: u64, buf: &mut [u8]) -> io::Result<usize> {
        std::os::unix::fs::FileExt::read_at(self, buf, addr)
    }
}

/// Reads stop at 4096-byte boundaries so they never fault on an unmapped page
/// past the data they need.
const fn to_boundary(addr: u64) -> u64 {
    CHUNK - addr % CHUNK
}

fn too_large(what: &str) -> io::Error {
    io::Error::other(format!("{what} too large to inspect"))
}

fn read_full(mem: &impl Memory, addr: u64, buf: &mut [u8]) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = mem.read_at(addr + done as u64, &mut buf[done..])?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
    }
    Ok(())
}

/// Read a NUL-terminated string of at most `max` bytes at `addr`.
pub fn read_cstring(mem: &impl Memory, addr: u64, max: usize) -> io::Result<Vec<u8>> {
    if addr == 0 {
        return Err(io::Error::other("null string pointer"));
    }
    let mut out = Vec::new();
    let mut cursor = addr;
    loop {
        let want = to_boundary(cursor).min((max + 1 - out.len()) as u64) as usize;
        let mut buf = vec![0u8; want];
        let n = mem.read_at(cursor, &mut buf)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        if let Some(nul) = buf[..n].iter().position(|&b| b == 0) {
            out.extend_from_slice(&buf[..nul]);
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() > max {
            return Err(too_large("string"));
        }
        cursor += n as u64;
    }
}

/// Read a NULL-terminated array of native pointers at `addr`. A null `addr`
/// is an empty array, as the kernel treats a null `argv`.
pub fn read_ptr_array(mem: &impl Memory, addr: u64, max: usize) -> io::Result<Vec<u64>> {
    let mut out = Vec::new();
    if addr == 0 {
        return Ok(out);
    }
    let mut cursor = addr;
    loop {
        let span = (to_boundary(cursor) / 8 * 8).min(512) as usize;
        let len = span.max(8);
        let mut buf = vec![0u8; len];
        read_full(mem, cursor, &mut buf)?;
        for word in buf.chunks_exact(8) {
            let ptr = u64::from_ne_bytes(word.try_into().unwrap_or([0; 8]));
            if ptr == 0 {
                return Ok(out);
            }
            if out.len() == max {
                return Err(too_large("argv"));
            }
            out.push(ptr);
        }
        cursor += len as u64;
    }
}

/// Read the argv array at `addr` into strings (lossy UTF-8).
pub fn read_argv(mem: &impl Memory, addr: u64) -> io::Result<Vec<String>> {
    let ptrs = read_ptr_array(mem, addr, MAX_ARGS)?;
    let mut budget = MAX_ARGV_BYTES;
    let mut argv = Vec::with_capacity(ptrs.len());
    for ptr in ptrs {
        let bytes = read_cstring(mem, ptr, budget)?;
        budget = budget.checked_sub(bytes.len() + 1).ok_or_else(|| too_large("argv"))?;
        argv.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(argv)
}

/// One exec as the kernel saw it: the resolved path and the argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub path: String,
    pub argv: Vec<String>,
}

fn basename(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

/// Keep characters the rule matcher reads as plain words; anything that is
/// shell syntax to it (quotes, `$`, `;`, `|`, `&`, redirections, `#`) becomes a
/// separator, since the kernel's argv carries no shell meaning.
fn sanitize(arg: &str) -> String {
    arg.chars()
        .map(|c| {
            if c.is_alphanumeric() || "-_./=:+,@%^~*?[]{}!".contains(c) { c } else { ' ' }
        })
        .collect()
}

impl ExecRequest {
    /// The command strings to judge. The program is named by the executed
    /// file's basename (argv[0] is caller-chosen); when argv[0] names a
    /// different program, that rendering is judged too.
    #[must_use]
    pub fn match_commands(&self) -> Vec<String> {
        let args: Vec<String> = self.argv.iter().skip(1).map(|a| sanitize(a.as_str())).collect();
        let args = args.join(" ");
        let mut programs = vec![sanitize(basename(&self.path))];
        if let Some(arg0) = self.argv.first() {
            let arg0 = sanitize(basename(arg0));
            if !arg0.trim().is_empty() && arg0 != programs[0] {
                programs.push(arg0);
            }
        }
        programs
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| format!("{} {args}", p.trim()).trim().to_string())
            .collect()
    }

    /// The argv shell-quoted for humans, capped at [`MAX_DISPLAY`] characters.
    #[must_use]
    pub fn display(&self) -> String {
        let joined = shlex::try_join(self.argv.iter().map(String::as_str))
            .unwrap_or_else(|_| self.argv.join(" "));
        let joined = if joined.is_empty() { self.path.clone() } else { joined };
        truncate(&joined, MAX_DISPLAY)
    }
}

/// Cap `s` at `max` characters, marking the cut.
#[must_use]
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// The line written to the denied process's stderr.
#[must_use]
pub fn denial_message(command: Option<&str>, reason: &str) -> String {
    let subject = command.map_or_else(|| "exec".to_string(), |c| format!("`{c}`"));
    format!(
        "guard: denied {subject} — {} (run `cctui-guard why` for recent denials)\n",
        truncate(reason, 400)
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// Sparse fake address space: mapped 4096-byte pages, reads fail past them.
    #[derive(Default)]
    struct FakeMem {
        pages: BTreeMap<u64, Vec<u8>>,
    }

    impl FakeMem {
        fn write(&mut self, addr: u64, bytes: &[u8]) {
            for (i, b) in bytes.iter().enumerate() {
                let a = addr + i as u64;
                let page = self.pages.entry(a / CHUNK).or_insert_with(|| vec![0; CHUNK as usize]);
                page[(a % CHUNK) as usize] = *b;
            }
        }
        fn write_ptrs(&mut self, addr: u64, ptrs: &[u64]) {
            let bytes: Vec<u8> = ptrs.iter().flat_map(|p| p.to_ne_bytes()).collect();
            self.write(addr, &bytes);
        }
    }

    impl Memory for FakeMem {
        fn read_at(&self, addr: u64, buf: &mut [u8]) -> io::Result<usize> {
            let mut n = 0;
            while n < buf.len() {
                let a = addr + n as u64;
                let Some(page) = self.pages.get(&(a / CHUNK)) else {
                    break;
                };
                buf[n] = page[(a % CHUNK) as usize];
                n += 1;
            }
            if n == 0 { Err(io::Error::from_raw_os_error(5)) } else { Ok(n) }
        }
    }

    #[test]
    fn reads_a_string_ending_right_before_an_unmapped_page() {
        let mut mem = FakeMem::default();
        let addr = 2 * CHUNK - 5;
        mem.write(addr, b"git\0");
        assert_eq!(read_cstring(&mem, addr, MAX_PATH).unwrap(), b"git");
    }

    #[test]
    fn reads_a_string_across_pages() {
        let mut mem = FakeMem::default();
        let addr = CHUNK - 2;
        mem.write(addr, b"/usr/bin/git\0");
        assert_eq!(read_cstring(&mem, addr, MAX_PATH).unwrap(), b"/usr/bin/git");
    }

    #[test]
    fn unterminated_or_oversized_strings_fail() {
        let mut mem = FakeMem::default();
        mem.write(CHUNK, &[b'a'; 100]);
        assert!(read_cstring(&mem, CHUNK, 10).is_err());
        assert!(read_cstring(&mem, 0, 10).is_err());
        let mut tail = FakeMem::default();
        tail.write(CHUNK - 3, b"abc");
        tail.pages.retain(|&p, _| p == 0);
        assert!(read_cstring(&tail, CHUNK - 3, MAX_PATH).is_err());
    }

    #[test]
    fn reads_argv_through_pointer_array() {
        let mut mem = FakeMem::default();
        mem.write(0x1_0000, b"git\0push\0--force\0");
        mem.write_ptrs(0x2000 - 16, &[0x1_0000, 0x1_0004, 0x1_0009, 0]);
        let argv = read_argv(&mem, 0x2000 - 16).unwrap();
        assert_eq!(argv, ["git", "push", "--force"]);
    }

    #[test]
    fn null_argv_is_empty() {
        assert!(read_argv(&FakeMem::default(), 0).unwrap().is_empty());
    }

    #[test]
    fn pointer_array_without_terminator_is_capped() {
        let mut mem = FakeMem::default();
        mem.write_ptrs(CHUNK, &[0x10; 64]);
        mem.write_ptrs(CHUNK + 64 * 8, &[0x10; 64]);
        assert!(read_ptr_array(&mem, CHUNK, 100).is_err());
    }

    #[test]
    fn non_utf8_args_are_lossy() {
        let mut mem = FakeMem::default();
        mem.write(0x3000, b"a\xffb\0");
        mem.write_ptrs(0x4000, &[0x3000, 0]);
        assert_eq!(read_argv(&mem, 0x4000).unwrap(), ["a\u{fffd}b"]);
    }

    fn req(path: &str, argv: &[&str]) -> ExecRequest {
        ExecRequest { path: path.into(), argv: argv.iter().map(ToString::to_string).collect() }
    }

    #[test]
    fn match_commands_use_the_executed_basename() {
        assert_eq!(req("/usr/bin/git", &["git", "push"]).match_commands(), ["git push"]);
        assert_eq!(req("/usr/bin/git", &["/usr/bin/git", "push"]).match_commands(), ["git push"]);
    }

    #[test]
    fn a_disguised_argv0_is_judged_under_both_names() {
        assert_eq!(
            req("/usr/bin/git", &["harmless", "push"]).match_commands(),
            ["git push", "harmless push"]
        );
    }

    #[test]
    fn shell_syntax_inside_args_is_neutralised() {
        let cmds = req("/bin/sh", &["sh", "-c", "git push && echo $(id) > /tmp/x; a|b"])
            .match_commands();
        assert_eq!(cmds.len(), 1);
        for bad in ["&", "$", "(", ">", ";", "|", "'", "\""] {
            assert!(!cmds[0].contains(bad), "{bad} survived in {:?}", cmds[0]);
        }
        assert!(cmds[0].starts_with("sh -c git push"), "{:?}", cmds[0]);
    }

    #[test]
    fn display_quotes_and_truncates() {
        let shown = req("/usr/bin/git", &["git", "commit", "-m", "a b"]).display();
        assert_eq!(shown, "git commit -m 'a b'");
        let long = "x".repeat(1000);
        let shown = req("/bin/echo", &["echo", &long]).display();
        assert_eq!(shown.chars().count(), MAX_DISPLAY + 1);
        assert!(shown.ends_with('…'));
        assert_eq!(req("/bin/true", &[]).display(), "/bin/true");
    }

    #[test]
    fn denial_message_names_the_command_and_points_at_why() {
        let reason = "[Step 3] 'git push' is disallowed in this step";
        let msg = denial_message(Some("git push"), reason);
        assert!(msg.starts_with("guard: denied `git push` — [Step 3]"), "{msg}");
        assert!(msg.contains("cctui-guard why"));
        assert!(msg.ends_with('\n'));
        assert!(denial_message(None, "timed out").starts_with("guard: denied exec — timed out"));
    }
}
