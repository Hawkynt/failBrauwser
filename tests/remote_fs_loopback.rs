// End-to-end checks for the remote VFS (M7) that need no network beyond loopback:
// a real russh SSH server exporting russh-sftp over a disposable folder, and a
// hand-rolled FTP responder, both on 127.0.0.1 with ephemeral ports. The app's own
// client engine — pool, auth prompts, host-key TOFU, both protocol stacks — is driven
// through its public API exactly the way the panes will use it (Given a local server,
// When browsing/downloading/uploading/renaming, Then the folder on disk follows).
#![cfg(windows)]

use failbrauwser::remote_fs::{self, auth, Target};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A throwaway fixture key (empty passphrase): it serves only this test's SSH server.
/// Regenerate with `ssh-keygen -t ed25519 -N ""` if it ever needs refreshing.
const FIXTURE_HOSTKEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----
b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW
QyNTUxOQAAACBAm3DkPEHz+nx4iuaI/eJFTS5eLd85EZmEYOU1Y4eFjQAAAKh4vBoweLwa
MAAAAAtzc2gtZWQyNTUxOQAAACBAm3DkPEHz+nx4iuaI/eJFTS5eLd85EZmEYOU1Y4eFjQ
AAAECozQjF35uI/iuikkbrbzRGFiHu6IR0lqxa/J7XVLnqdkCbcOQ8QfP6fHiK5oj94kVN
Ll4t3zkRmYRg5TVjh4WNAAAAImZhaWxicmF1d3NlciBsb29wYmFjayB0ZXN0IGZpeHR1cm
UBAgM=
-----END OPENSSH PRIVATE KEY-----
";

#[test]
fn sftp_and_ftp_servers_on_loopback_round_trip() {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let tmp = std::env::temp_dir().join(format!("fb-remote-{stamp}"));
    std::fs::create_dir_all(&tmp).unwrap();

    // The TOFU store honors XDG_CONFIG_HOME (probed for this GLib build in M0a); point
    // it at the fixture so the developer's own config never sees a line.
    let xdg = tmp.join("xdg");
    std::fs::create_dir_all(xdg.join("failbrauwser")).unwrap();

    auth::set_prompter(Some(Box::new(|a| match a {
        auth::Ask::Password { .. } => auth::Answer::Secret("hunter2".into()),
        _ => auth::Answer::Cancel,
    })));

    sftp_round_trip(&tmp, &xdg);
    ftp_round_trip(&tmp);

    auth::set_prompter(None);
    unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    let _ = std::fs::remove_dir_all(&tmp);
}

// -- SFTP ---------------------------------------------------------------------

fn sftp_round_trip(tmp: &Path, xdg: &Path) {
    let root = tmp.join("sftp-root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(root.join("emptydir")).unwrap();
    std::fs::write(root.join("hello.txt"), b"remote says hello\n").unwrap();
    std::fs::write(root.join("sub").join("nested.txt"), b"nested").unwrap();

    let key = russh::keys::PrivateKey::from_openssh(FIXTURE_HOSTKEY).unwrap();
    // The wire shows "alg base64" without the PEM's comment — seed the store exactly as
    // the client will see it (trust_host compares the shown form).
    let publine = russh::keys::PublicKey::from(&key).to_string().split(' ').take(2).collect::<Vec<_>>().join(" ");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let store = xdg.join("failbrauwser").join("known_hosts");
    std::fs::write(&store, format!("127.0.0.1:{port} {publine}\nmismatch.example.invalid:22 ssh-ed25519 AAAAreal\n")).unwrap();
    unsafe { std::env::set_var("XDG_CONFIG_HOME", xdg) };

    // A key the seed says is different: the TOFU gate must refuse before signing in.
    let e = auth::trust_host("mismatch.example.invalid", 22, "ssh-ed25519 AAAAtotally-other").unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "a changed host key must not be signed through");

    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<russh::server::RunningServerHandle>();
    let root_for_srv = root.clone();
    let srv = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            let cfg = Arc::new(russh::server::Config { keys: vec![key], auth_rejection_time: std::time::Duration::from_millis(10), ..Default::default() });
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let mut host = SftpHost { root: root_for_srv };
            let running = russh::server::Server::run_on_socket(&mut host, cfg, &listener);
            let _ = ready_tx.send(running.handle());
            // completes once the test asks the server to shut down
            let _ = running.await;
        });
    });
    let sh = ready_rx.recv().unwrap();

    let t = Target::from_uri(&format!("sftp://tester@127.0.0.1:{port}/")).unwrap();
    let at = remote_fs::RemoteAt::Server(t.clone());

    let entries = remote_fs::list(&at, "").expect("listing the fixture root");
    let names: Vec<(&str, bool, u64)> = entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect();
    assert_eq!(names, vec![("emptydir", true, 0), ("hello.txt", false, 18), ("sub", true, 0)], "sorted listing with sizes and types");
    assert!(entries.iter().find(|e| e.name == "hello.txt").unwrap().mtime > 1_700_000_000, "the server's real mtime rides through");

    let down = tmp.join("down.txt");
    assert_eq!(remote_fs::read_to(&at, "/hello.txt", &down).unwrap(), 18);
    assert_eq!(std::fs::read(&down).unwrap(), b"remote says hello\n");

    let up = tmp.join("up.bin");
    std::fs::write(&up, b"uploaded bytes").unwrap();
    assert_eq!(remote_fs::write_from(&at, &up, "/up.txt").unwrap(), 14);
    assert_eq!(std::fs::read(root.join("up.txt")).unwrap(), b"uploaded bytes", "the write landed on the server's disk");

    remote_fs::mkdir(&at, "/newdir").unwrap();
    assert!(root.join("newdir").is_dir());
    remote_fs::rename(&at, "/up.txt", "/moved.txt").unwrap();
    assert!(!root.join("up.txt").exists() && root.join("moved.txt").exists());
    remote_fs::remove(&at, "/moved.txt", false).unwrap();
    remote_fs::remove(&at, "/newdir", true).unwrap();
    assert!(!root.join("moved.txt").exists() && !root.join("newdir").exists());

    assert!(remote_fs::exists(&at, "/sub/nested.txt"));
    assert!(!remote_fs::exists(&at, "/gone.txt"));

    // Searching below a server folder walks it over the same connection: the deep search
    // the pane runs for a remote base (folder of each hit relative to the server root).
    let mut hits = Vec::new();
    let q = failbrauwser::search::Query::new("*.txt");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    assert!(failbrauwser::search::search_listed("", &q, false, &cancel, |dir| remote_fs::list(&at, dir), |dir, e| hits.push(format!("{dir}|{}", e.name))));
    assert_eq!(hits, vec!["|hello.txt", "sub|nested.txt"], "matches of a folder, then of its subfolders");
    let err = remote_fs::list(&at, "/nope").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "a missing folder reads as NotFound");

    // Whole-tree legs of the copy engine: folders walk down to staging and back up,
    // and a name clash is answered through the job's own question (scripted KeepBoth).
    use failbrauwser::ops::job::{Answer, JobCtx, Question, Reply};
    let ctx = JobCtx::with_answers(|q| match q {
        Question::Conflict { .. } => Reply { answer: Answer::KeepBoth, for_all: false },
        _ => Reply { answer: Answer::Cancel, for_all: true },
    });
    let stage = tmp.join("stage");
    std::fs::create_dir_all(&stage).unwrap();
    assert_eq!(remote_fs::fetch_tree(&ctx, &at, "/sub", true, &stage).unwrap(), vec![stage.join("sub")], "a walked folder reports its top entry");
    assert_eq!(std::fs::read(stage.join("sub/nested.txt")).unwrap(), b"nested", "the walk followed the listing down");
    assert_eq!(remote_fs::fetch_tree(&ctx, &at, "/hello.txt", false, &stage).unwrap(), vec![stage.join("hello.txt")]);

    let out = tmp.join("out");
    std::fs::create_dir_all(out.join("deep")).unwrap();
    std::fs::write(out.join("deep/a.txt"), b"ae").unwrap();
    std::fs::write(out.join("b.txt"), b"bee").unwrap();
    std::fs::write(stage.join("hello.txt"), b"local copy of hello\n").unwrap();
    let created = remote_fs::publish_tree(&ctx, &[out.clone(), stage.join("hello.txt")], &at, "").unwrap();
    assert_eq!(created, vec![std::path::PathBuf::from("out"), std::path::PathBuf::from("hello (2).txt")], "the folder lands; KeepBoth renumbers the clash");
    assert_eq!(std::fs::read(root.join("out/deep/a.txt")).unwrap(), b"ae", "nested locals go out over mkdir+write");
    assert_eq!(std::fs::read(root.join("out/b.txt")).unwrap(), b"bee");
    assert_eq!(std::fs::read(root.join("hello (2).txt")).unwrap(), b"local copy of hello\n", "the renumbered write landed");
    assert_eq!(std::fs::read(root.join("hello.txt")).unwrap(), b"remote says hello\n", "KeepBoth left the original alone");

    remote_fs::close_all();
    sh.shutdown("test over".into());
    srv.join().unwrap();
}

/// The russh side: accept tester/hunter2, hand sftp subsystems to a disk-backed folder.
struct SftpHost {
    root: PathBuf,
}

impl russh::server::Server for SftpHost {
    type Handler = SftpLogin;

    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Self::Handler {
        SftpLogin { root: self.root.clone(), channels: std::collections::HashMap::new() }
    }

    fn handle_session_error(&mut self, error: LoginError) {
        eprintln!("loopback ssh session ended: {}", error.0);
    }
}

struct SftpLogin {
    root: PathBuf,
    channels: std::collections::HashMap<russh::ChannelId, russh::Channel<russh::server::Msg>>,
}

#[derive(Debug)]
struct LoginError(russh::Error);

impl From<russh::Error> for LoginError {
    fn from(e: russh::Error) -> Self {
        LoginError(e)
    }
}

impl russh::server::Handler for SftpLogin {
    type Error = LoginError;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<russh::server::Auth, Self::Error> {
        Ok(if user == "tester" && password == "hunter2" { russh::server::Auth::Accept } else { russh::server::Auth::Reject { proceed_with_methods: None, partial_success: false } })
    }

    async fn channel_open_session(&mut self, channel: russh::Channel<russh::server::Msg>, reply: russh::server::ChannelOpenHandle, _session: &mut russh::server::Session) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn subsystem_request(&mut self, channel_id: russh::ChannelId, name: &str, session: &mut russh::server::Session) -> Result<(), Self::Error> {
        let Some(channel) = self.channels.remove(&channel_id) else {
            session.channel_failure(channel_id).map_err(LoginError)?;
            return Ok(());
        };
        session.channel_success(channel_id).map_err(LoginError)?;
        if name == "sftp" {
            let fs = SftpFs { root: self.root.clone(), handles: std::collections::HashMap::new(), next: 0 };
            russh_sftp::server::run(channel.into_stream(), fs).await;
        }
        Ok(())
    }
}

/// A disk-backed sftp handler: enough of the protocol for the app's engine
/// (opendir/readdir, open/read/write/close, mkdir, remove, rename, stat).
struct SftpFs {
    root: PathBuf,
    handles: std::collections::HashMap<String, FsSlot>,
    next: u32,
}

enum FsSlot {
    File(std::fs::File),
    Dir(Vec<russh_sftp::protocol::File>),
}

fn io_status(e: &std::io::Error) -> russh_sftp::protocol::StatusCode {
    use russh_sftp::protocol::StatusCode as S;
    match e.kind() {
        std::io::ErrorKind::NotFound => S::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => S::PermissionDenied,
        _ => S::Failure,
    }
}

fn attrs_of(p: &Path) -> Option<russh_sftp::protocol::FileAttributes> {
    use russh_sftp::protocol::FileAttributes;
    let md = p.metadata().ok()?;
    let mtime = md.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as u32);
    Some(FileAttributes {
        size: Some(md.len()),
        uid: None,
        user: None,
        gid: None,
        group: None,
        permissions: Some(if md.is_dir() { 0o040_755 } else { 0o100_644 }),
        atime: None,
        mtime: Some(mtime.unwrap_or(0)),
    })
}

fn ok_status(id: u32) -> russh_sftp::protocol::Status {
    russh_sftp::protocol::Status { id, status_code: russh_sftp::protocol::StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

impl russh_sftp::server::Handler for SftpFs {
    type Error = russh_sftp::protocol::StatusCode;

    fn unimplemented(&self) -> Self::Error {
        russh_sftp::protocol::StatusCode::OpUnsupported
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<russh_sftp::protocol::Name, Self::Error> {
        Ok(russh_sftp::protocol::Name { id, files: vec![russh_sftp::protocol::File::dummy(path)] })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        attrs_of(&self.map(&path)).map(|attrs| russh_sftp::protocol::Attrs { id, attrs }).ok_or(russh_sftp::protocol::StatusCode::NoSuchFile)
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<russh_sftp::protocol::Handle, Self::Error> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(self.map(&path)).map_err(|e| io_status(&e))? {
            let e = e.map_err(|e| io_status(&e))?;
            if let Some(a) = attrs_of(&e.path()) {
                files.push(russh_sftp::protocol::File::new(e.file_name().to_string_lossy().into_owned(), a));
            }
        }
        files.sort_by(|a, b| a.filename.cmp(&b.filename));
        let h = self.slot();
        self.handles.insert(h.clone(), FsSlot::Dir(files));
        Ok(russh_sftp::protocol::Handle { id, handle: h })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<russh_sftp::protocol::Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(FsSlot::Dir(files)) if !files.is_empty() => Ok(russh_sftp::protocol::Name { id, files: std::mem::take(files) }),
            Some(FsSlot::Dir(_)) => Err(russh_sftp::protocol::StatusCode::Eof),
            _ => Err(russh_sftp::protocol::StatusCode::BadMessage),
        }
    }

    async fn open(&mut self, id: u32, filename: String, pflags: russh_sftp::protocol::OpenFlags, _attrs: russh_sftp::protocol::FileAttributes) -> Result<russh_sftp::protocol::Handle, Self::Error> {
        use russh_sftp::protocol::OpenFlags as F;
        let mut o = std::fs::OpenOptions::new();
        let write = pflags.contains(F::WRITE);
        o.read(!write || pflags.contains(F::READ));
        if write {
            o.write(true);
            if pflags.contains(F::APPEND) { o.append(true); } else { o.truncate(pflags.contains(F::TRUNCATE)); }
        }
        o.create(pflags.contains(F::CREATE));
        let file = o.open(self.map(&filename)).map_err(|e| io_status(&e))?;
        let h = self.slot();
        self.handles.insert(h.clone(), FsSlot::File(file));
        Ok(russh_sftp::protocol::Handle { id, handle: h })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<russh_sftp::protocol::Data, Self::Error> {
        use std::io::Seek;
        let FsSlot::File(f) = self.handles.get_mut(&handle).ok_or(russh_sftp::protocol::StatusCode::BadMessage)? else {
            return Err(russh_sftp::protocol::StatusCode::BadMessage);
        };
        f.seek(std::io::SeekFrom::Start(offset)).map_err(|e| io_status(&e))?;
        let mut buf = vec![0u8; len.min(32 * 1024) as usize];
        let n = f.read(&mut buf).map_err(|e| io_status(&e))?;
        if n == 0 {
            return Err(russh_sftp::protocol::StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(russh_sftp::protocol::Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<russh_sftp::protocol::Status, Self::Error> {
        use std::io::Seek;
        let FsSlot::File(f) = self.handles.get_mut(&handle).ok_or(russh_sftp::protocol::StatusCode::BadMessage)? else {
            return Err(russh_sftp::protocol::StatusCode::BadMessage);
        };
        f.seek(std::io::SeekFrom::Start(offset)).map_err(|e| io_status(&e))?;
        f.write_all(&data).map_err(|e| io_status(&e))?;
        Ok(ok_status(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<russh_sftp::protocol::Status, Self::Error> {
        self.handles.remove(&handle).ok_or(russh_sftp::protocol::StatusCode::BadMessage)?;
        Ok(ok_status(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: russh_sftp::protocol::FileAttributes) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::create_dir(self.map(&path)).map_err(|e| io_status(&e))?;
        Ok(ok_status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::remove_dir(self.map(&path)).map_err(|e| io_status(&e))?;
        Ok(ok_status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::remove_file(self.map(&filename)).map_err(|e| io_status(&e))?;
        Ok(ok_status(id))
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::rename(self.map(&oldpath), self.map(&newpath)).map_err(|e| io_status(&e))?;
        Ok(ok_status(id))
    }
}

impl SftpFs {
    fn slot(&mut self) -> String {
        self.next += 1;
        format!("h{}", self.next)
    }

    fn map(&self, p: &str) -> PathBuf {
        let rel = p.trim_start_matches('/');
        if rel.is_empty() || rel == "." { self.root.clone() } else { self.root.join(rel) }
    }
}

// -- FTP ----------------------------------------------------------------------

/// A minimal but faithful responder: 220/331/230 sign-in, PASV handshakes, LIST,
/// RETR and STOR transfers, 221 farewell — the sequence suppaftp's sync client drives.
fn ftp_round_trip(tmp: &Path) {
    let root = tmp.join("ftp-root");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("hello.txt"), b"ftp says hello!").unwrap();
    let listening = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listening.local_addr().unwrap().port();
    let root2 = root.clone();
    let srv = std::thread::spawn(move || ftp_serve(&listening, &root2).unwrap());

    let t = Target::from_uri(&format!("ftp://tester@127.0.0.1:{port}")).unwrap();
    let at = remote_fs::RemoteAt::Server(t.clone());

    let entries = remote_fs::list(&at, "/").expect("the LIST round trip");
    // the wire order, with the un*x line, the DOS line and the noise line all handled
    assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect::<Vec<_>>(), vec![("photos", true, 0), ("reports", true, 0), ("hello.txt", false, 15)], "both LIST dialects parse off the wire");
    assert_eq!(entries[0].mtime, 1_673_740_800, "the un*x line's dated year is read");
    assert_eq!(entries[1].mtime, 1_762_333_200, "the DOS line's date and AM time are read");

    let down = tmp.join("ftp-down.txt");
    assert_eq!(remote_fs::read_to(&at, "/hello.txt", &down).unwrap(), 15);
    assert_eq!(std::fs::read(&down).unwrap(), b"ftp says hello!");

    let up = tmp.join("ftp-up.bin");
    std::fs::write(&up, b"ftp uploaded bytes").unwrap();
    assert_eq!(remote_fs::write_from(&at, &up, "/up.txt").unwrap(), 18);
    assert_eq!(std::fs::read(root.join("stored-up.txt")).unwrap(), b"ftp uploaded bytes", "STOR landed on disk");

    assert!(remote_fs::exists(&at, "/hello.txt"));
    assert!(!remote_fs::exists(&at, "/gone.txt"));

    remote_fs::close_all();
    srv.join().unwrap();
}

fn ftp_serve(listening: &TcpListener, root: &Path) -> std::io::Result<()> {
    let (mut ctl, _) = listening.accept()?;
    let mut rd = BufReader::new(ctl.try_clone()?);
    let say = |wr: &mut TcpStream, line: &str| -> std::io::Result<()> {
        wr.write_all(format!("{line}\r\n").as_bytes())?;
        wr.flush()
    };
    say(&mut ctl, "220 failBrauwser loopback ftp")?;
    // The listener the next PASV sets up; one transfer per command, as suppaftp asks.
    let mut pending: Option<TcpListener> = None;
    let mut line = String::new();
    loop {
        line.clear();
        if rd.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let cmd = line.trim_end_matches(['\r', '\n']).to_string();
        let (verb, arg) = cmd.split_once(' ').unwrap_or((cmd.as_str(), ""));
        match verb {
            "USER" => say(&mut ctl, "331 Please specify the password.")?,
            "PASS" => say(&mut ctl, if arg == "hunter2" { "230 Logged in" } else { "530 Not allowed" })?,
            "SYST" => say(&mut ctl, "215 UNIX Type: L8")?,
            "TYPE" | "CWD" | "PWD" | "NOOP" => say(&mut ctl, "200 ok")?,
            "PASV" => {
                let data = TcpListener::bind("127.0.0.1:0")?;
                let p = data.local_addr()?.port();
                pending = Some(data);
                say(&mut ctl, &format!("227 Entering Passive Mode (127,0,0,1,{},{}).", p / 256, p % 256))?;
            }
            "LIST" => {
                let (mut d, _) = pending.take().expect("LIST after PASV").accept()?;
                say(&mut ctl, "125 Here comes the directory listing")?;
                // The two dialects the parser knows, one line each, plus noise it must drop.
                d.write_all(b"total 40\r\ndrwxr-xr-x  2 owner group    4096 Jan 15  2023 photos\r\n11-05-25  09:00AM       <DIR>          reports\r\n-rw-r--r--  1 owner group      15 Nov  5 07:00 hello.txt\r\n")?;
                d.flush()?;
                drop(d);
                say(&mut ctl, "226 Transfer complete")?;
            }
            "RETR" => {
                let (mut d, _) = pending.take().expect("RETR after PASV").accept()?;
                say(&mut ctl, "125 Opening ASCII mode data connection for file transfer")?;
                d.write_all(&std::fs::read(root.join(arg.trim_start_matches('/').rsplit('/').next().unwrap_or_default()))?)?;
                d.flush()?;
                drop(d);
                say(&mut ctl, "226 Transfer complete")?;
            }
            "STOR" => {
                let (mut d, _) = pending.take().expect("STOR after PASV").accept()?;
                say(&mut ctl, "125 Data connection already open; starting transfer")?;
                let mut buf = Vec::new();
                d.read_to_end(&mut buf)?;
                drop(d);
                std::fs::write(root.join("stored-up.txt"), buf)?;
                say(&mut ctl, "226 Transfer complete")?;
            }
            "QUIT" => {
                say(&mut ctl, "221 Goodbye")?;
                return Ok(());
            }
            _ => say(&mut ctl, "502 not implemented")?,
        }
    }
}
