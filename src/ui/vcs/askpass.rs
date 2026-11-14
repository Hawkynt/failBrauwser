//! Asks for user names, passwords and key passphrases that git or ssh need while a Git
//! command of ours runs: they start this program again (as GIT_ASKPASS / SSH_ASKPASS) with
//! the question, and it prints the answer.

use gtk::prelude::*;

/// Set in the environment of our git commands: the program answers as askpass helper.
pub const ENV: &str = "FB_GIT_ASKPASS";

/// Answers the question in the arguments when started as askpass helper; None otherwise.
pub fn main_if_asked() -> Option<i32> {
    if std::env::var_os(ENV).is_none_or(|v| v.is_empty()) {
        return None;
    }
    let question = std::env::args().nth(1).unwrap_or_else(|| "Password:".into());
    // Tests answer without a window.
    if let Ok(reply) = std::env::var("FB_ASKPASS_REPLY") {
        println!("{reply}");
        return Some(0);
    }
    if gtk::init().is_err() {
        return Some(1);
    }
    let secret = is_secret(&question);
    let d = gtk::Dialog::with_buttons(Some("TortoiseGit"), None::<&gtk::Window>, gtk::DialogFlags::MODAL, &[("OK", gtk::ResponseType::Ok), ("Cancel", gtk::ResponseType::Cancel)]);
    d.set_default_response(gtk::ResponseType::Ok);
    d.set_keep_above(true);
    if let Some(pb) = super::icons::pixbuf("tgit", 32) {
        d.set_icon(Some(&pb));
    }
    let area = d.content_area();
    area.set_border_width(12);
    area.set_spacing(8);
    let l = gtk::Label::new(Some(question.trim()));
    l.set_xalign(0.0);
    l.set_line_wrap(true);
    l.set_max_width_chars(60);
    let e = gtk::Entry::new();
    e.set_visibility(!secret);
    e.set_activates_default(true);
    e.set_width_chars(40);
    area.pack_start(&l, false, false, 0);
    area.pack_start(&e, false, false, 0);
    d.show_all();
    e.grab_focus();
    let r = d.run();
    let text = e.text().to_string();
    unsafe { d.destroy() };
    if r == gtk::ResponseType::Ok {
        println!("{text}");
        Some(0)
    } else {
        Some(1)
    }
}

/// Whether the question asks for something not to show (a password or passphrase).
pub fn is_secret(question: &str) -> bool {
    let q = question.to_lowercase();
    q.contains("password") || q.contains("passphrase") || q.contains("pin")
}

#[cfg(test)]
mod tests {
    #[test]
    fn secrets_are_hidden() {
        assert!(super::is_secret("Password for 'https://ann@example.org': "));
        assert!(super::is_secret("Enter passphrase for key '/home/a/.ssh/id_ed25519': "));
        assert!(!super::is_secret("Username for 'https://example.org': "));
    }
}
