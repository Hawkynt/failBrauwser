//! One program started as the administrator: this program's own file, through the shell's `runas` verb,
//! which is what brings up the consent dialog. The program itself stays non-admin — installed, started and
//! run without rights — and every privileged act asks for its own rights at the moment it needs them
//! (docs\WINDOWS.md). What the run answers, and where it writes that answer, belongs to `fsformat::act`;
//! this module only hands the command to the shell and waits for the process the shell names.

/// Starts this program's own file as the administrator with these arguments and waits for it to end.
/// The arguments are handed over as one parameter string, quoted the way the Windows command line is
/// read, because `ShellExecuteExW` takes the whole tail of the command line as a single string.
pub fn run_self_with(arguments: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("failBrauwser cannot name its own file: {e}"))?;
    let verb = crate::win::err::wide("runas");
    let file = crate::win::err::wide(&exe.to_string_lossy());
    let parameters = crate::win::err::wide(&parameters_text(arguments));
    let mut request = windows::Win32::UI::Shell::SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<windows::Win32::UI::Shell::SHELLEXECUTEINFOW>() as u32,
        // The process handle is what the window waits for: without it the shell would report the start
        // and the answer file would be read before anything was written into it.
        fMask: windows::Win32::UI::Shell::SEE_MASK_NOCLOSEPROCESS,
        lpVerb: crate::win::err::pcw(&verb),
        lpFile: crate::win::err::pcw(&file),
        lpParameters: crate::win::err::pcw(&parameters),
        // The consent dialog is the only window either act shows; what runs afterwards has none of its
        // own, because the answer it gives goes into a file rather than onto a screen.
        nShow: windows::Win32::UI::WindowsAndMessaging::SW_HIDE.0,
        ..Default::default()
    };
    if unsafe { windows::Win32::UI::Shell::ShellExecuteExW(&mut request) }.is_err() {
        let error = crate::win::err::last();
        // 1223 is the machine's own answer for a consent dialog the user closed without agreeing. Nothing
        // ran then, which is the no-op the specification promises.
        if error.raw_os_error() == Some(1223) { return Err("The consent prompt was declined, so nothing was changed.".into()); }
        return Err(format!("Windows refused to start the privileged run: {error}"));
    }
    let process = request.hProcess;
    if process.is_invalid() { return Err("Windows named no process to wait for.".into()); }
    unsafe {
        use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        let _ = WaitForSingleObject(process, INFINITE);
        let _ = windows::Win32::Foundation::CloseHandle(process);
    }
    Ok(())
}

/// The arguments as the single parameter string `ShellExecuteExW` takes: each one quoted where it needs
/// it, in the rule the command line is read by (`CommandLineToArgvW` and the C runtime rule), which a
/// test on this machine round-trips against that very parser.
pub fn parameters_text(arguments: &[String]) -> String {
    arguments.iter().map(|argument| quoted(argument)).collect::<Vec<_>>().join(" ")
}

/// One argument in that rule: bare while it holds nothing the parser splits on, and inside quotes as soon
/// as it does — with every run of backslashes in front of a quote doubled, one more added for the quote
/// itself, and a run at the end of the argument doubled so that it cannot swallow the closing quote.
fn quoted(value: &str) -> String {
    if !value.chars().any(|c| matches!(c, ' ' | '\t' | '"')) { return if value.is_empty() { "\"\"".to_string() } else { value.to_string() }; }
    let mut out = String::from('"');
    let mut backslashes = 0usize;
    for c in value.chars() {
        match c {
            '\\' => { backslashes += 1; out.push('\\'); }
            '"' => { out.extend(std::iter::repeat_n('\\', backslashes + 1)); backslashes = 0; out.push('"'); }
            _ => { backslashes = 0; out.push(c); }
        }
    }
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_argument_is_quoted_only_where_the_command_line_would_split_it() {
        assert_eq!(quoted("F:"), "F:");
        assert_eq!(quoted("/FS:NTFS"), "/FS:NTFS");
        assert_eq!(quoted("/V:Samsung 64 GB"), "\"/V:Samsung 64 GB\"");
        assert_eq!(quoted(""), "\"\"", "an empty argument is still an argument");
        assert_eq!(quoted(r"C:\folder"), r"C:\folder", "backslashes alone are not a reason to quote");
        assert_eq!(quoted(r"C:\folder\"), r"C:\folder\", "nothing follows the backslash, so it needs no quote to protect");
        assert_eq!(quoted(r"C:\folder with spaces\"), r#""C:\folder with spaces\\""#, "a trailing backslash may not swallow the closing quote");
    }

    #[test]
    fn the_command_of_a_format_act_keeps_its_label_with_its_spaces() {
        let text = parameters_text(&["--format".into(), "--outcome".into(), r"C:\Users\me\AppData\Local\Temp\failbrauwser-format-42.out".into(), "F:".into(), "/FS:NTFS".into(), "/V:Samsung 64 GB".into(), "/A:8192".into(), "/Y".into()]);
        assert_eq!(text, r#"--format --outcome C:\Users\me\AppData\Local\Temp\failbrauwser-format-42.out F: /FS:NTFS "/V:Samsung 64 GB" /A:8192 /Y"#);
    }

    /// The rule proven against the parser the elevated run is read by, rather than against my own
    /// reading of the rule: every argument below comes back as it went in, spaces, quotes, backslashes
    /// and emptiness included.
    #[test]
    #[cfg(windows)]
    fn the_machine_s_own_parser_answers_back_exactly_what_the_parameters_gave_it() {
        let cases: [&str; 10] = [
            "F:", "/FS:NTFS", "/V:Samsung 64 GB", "", r"C:\Users\me\AppData\Local\Temp\failbrauwser-format-42.out", r"ends with a backslash \", r#"a "quote" inside"#, r"tab	in here", "/V=64 GB \\\\", r#"a "quote" and a backslash\""#,
        ];
        let parsed = parse(&parameters_text(&cases.map(str::to_string)));
        assert_eq!(parsed, cases, "the parameters round-trip through CommandLineToArgvW");
    }

    /// `CommandLineToArgvW` itself, freed after the answer is taken.
    #[cfg(windows)]
    fn parse(parameters: &str) -> Vec<String> {
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        let wide = crate::win::err::wide(parameters);
        let mut count = 0i32;
        unsafe {
            let argv = CommandLineToArgvW(crate::win::err::pcw(&wide), &raw mut count);
            assert!(!argv.is_null(), "{parameters:?} is a command line the machine can read");
            let out = std::slice::from_raw_parts(argv, count as usize).iter().map(|word| word.to_string().expect("the parser answers a name the machine can spell")).collect::<Vec<_>>();
            // The block the parser allocated is handed back, so the round-trip leaves nothing of its own behind.
            let _ = windows::Win32::Foundation::LocalFree(Some(windows::Win32::Foundation::HLOCAL(argv as *mut core::ffi::c_void)));
            out
        }
    }
}
