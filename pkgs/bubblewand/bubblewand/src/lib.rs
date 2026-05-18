use std::ffi::OsString;
use std::io::{self, Write};
use std::os::unix::io::FromRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use xdg::BaseDirectories;

use clap::Args;

const BWRAP: &str = match option_env!("BWRAP") { Some(s) => s, None => "bwrap" };
const XDG_DBUS_PROXY: &str = match option_env!("XDG_DBUS_PROXY") { Some(s) => s, None => "xdg-dbus-proxy" };
const PASTA: &str = match option_env!("PASTA") { Some(s) => s, None => "pasta" };

// ---------------------------------------------------------------------------
// CLI flags shared by both binaries
// ---------------------------------------------------------------------------

#[derive(Args, Clone, Debug)]
pub struct SandboxArgs {
    /// Enable GUI stack (wayland + audio + fonts + cursors)
    #[arg(long)]
    pub gui: bool,

    /// Enable audio (pulse + pipewire)
    #[arg(long)]
    pub audio: bool,

    #[arg(long)]
    pub network: bool,

    /// Isolated network namespace bridged to the host via pasta (rootless)
    #[arg(long, conflicts_with = "network")]
    pub pasta: bool,

    /// TCP port-forwarding spec passed to `pasta -t` (repeatable)
    #[arg(long = "pasta-tcp", value_name = "SPEC")]
    pub pasta_tcp: Vec<String>,

    /// UDP port-forwarding spec passed to `pasta -u` (repeatable)
    #[arg(long = "pasta-udp", value_name = "SPEC")]
    pub pasta_udp: Vec<String>,

    /// MAC address for the pasta TAP interface (e.g. for stable device fingerprinting)
    #[arg(long = "pasta-mac", value_name = "ADDR")]
    pub pasta_mac: Option<String>,

    #[arg(long)]
    pub gpu: bool,

    #[arg(long)]
    pub wayland: bool,

    #[arg(long)]
    pub pulse: bool,

    #[arg(long)]
    pub pipewire: bool,

    #[arg(long)]
    pub camera: bool,

    #[arg(long = "dbus-talk", value_name = "NAME")]
    pub dbus_talk: Vec<String>,

    #[arg(long = "dbus-own", value_name = "NAME")]
    pub dbus_own: Vec<String>,

    #[arg(long = "persist-home", value_name = "NAME")]
    pub persist_home: Option<String>,

    #[arg(long = "share-tmp", value_name = "NAME")]
    pub share_tmp: Option<String>,

    /// Set env var inside the sandbox (KEY=VALUE)
    #[arg(long = "set-env", value_name = "KEY=VALUE")]
    pub set_env: Vec<String>,

    /// Forward env var from host into sandbox
    #[arg(long = "fwd-env", value_name = "KEY")]
    pub fwd_env: Vec<String>,

    /// Read-only bind mount (HOST:DEST)
    #[arg(long = "ro-bind", value_name = "HOST:DEST")]
    pub ro_bind: Vec<String>,

    /// Read-write bind mount (HOST:DEST)
    #[arg(long = "rw-bind", value_name = "HOST:DEST")]
    pub rw_bind: Vec<String>,

    #[arg(long, value_name = "PATH")]
    pub tmpfs: Vec<String>,

    #[arg(long, default_value = "bubble")]
    pub hostname: String,

    #[arg(long = "new-session")]
    pub new_session: bool,

    /// Inherit the host environment instead of starting with a clean slate
    #[arg(long = "keep-env")]
    pub keep_env: bool,

    /// Override the bwrap binary
    #[arg(long, value_name = "PATH", hide = true)]
    pub bwrap: Option<String>,
}

impl Default for SandboxArgs {
    fn default() -> Self {
        Self {
            hostname: "bubble".into(),
            gui: false, audio: false, network: false, gpu: false,
            wayland: false, pulse: false, pipewire: false, camera: false,
            pasta: false, pasta_tcp: Vec::new(), pasta_udp: Vec::new(),
            pasta_mac: None,
            new_session: false, keep_env: false,
            dbus_talk: Vec::new(), dbus_own: Vec::new(),
            persist_home: None, share_tmp: None, set_env: Vec::new(), fwd_env: Vec::new(),
            ro_bind: Vec::new(), rw_bind: Vec::new(), tmpfs: Vec::new(),
            bwrap: None,
        }
    }
}

impl SandboxArgs {
    pub fn need_wayland(&self) -> bool { self.wayland || self.gui }
    pub fn need_pulse(&self) -> bool   { self.pulse || self.audio || self.gui }
    pub fn need_pipewire(&self) -> bool { self.pipewire || self.audio || self.gui }
    pub fn need_dbus(&self) -> bool    { !self.dbus_talk.is_empty() || !self.dbus_own.is_empty() }
    pub fn need_network_files(&self) -> bool { self.network || self.pasta }

    /// Serialize back to CLI args for embedding in wrapper scripts.
    pub fn to_cli_args(&self) -> Vec<String> {
        let mut out = Vec::new();

        macro_rules! flag {
            ($field:expr, $name:expr) => {
                if $field { out.push($name.to_string()); }
            };
        }
        macro_rules! opt {
            ($field:expr, $name:expr) => {
                if let Some(ref v) = $field { out.push(format!("{}={}", $name, v)); }
            };
        }
        macro_rules! multi {
            ($field:expr, $name:expr) => {
                for v in &$field { out.push(format!("{}={}", $name, v)); }
            };
        }

        flag!(self.gui,         "--gui");
        flag!(self.audio,       "--audio");
        flag!(self.network,     "--network");
        flag!(self.pasta,       "--pasta");
        flag!(self.gpu,         "--gpu");
        flag!(self.wayland,     "--wayland");
        flag!(self.pulse,       "--pulse");
        flag!(self.pipewire,    "--pipewire");
        flag!(self.camera,      "--camera");
        flag!(self.new_session, "--new-session");
        flag!(self.keep_env,    "--keep-env");

        out.push(format!("--hostname={}", self.hostname));

        opt!(self.persist_home, "--persist-home");
        opt!(self.share_tmp,  "--share-tmp");
        multi!(self.dbus_talk, "--dbus-talk");
        multi!(self.dbus_own,  "--dbus-own");
        multi!(self.pasta_tcp, "--pasta-tcp");
        multi!(self.pasta_udp, "--pasta-udp");
        opt!(self.pasta_mac,         "--pasta-mac");
        multi!(self.set_env,   "--set-env");
        multi!(self.fwd_env,   "--fwd-env");
        multi!(self.ro_bind,   "--ro-bind");
        multi!(self.rw_bind,   "--rw-bind");
        multi!(self.tmpfs,     "--tmpfs");

        opt!(self.bwrap, "--bwrap");

        out
    }
}

// ---------------------------------------------------------------------------
// bwrap argument builder
// ---------------------------------------------------------------------------

struct BwrapArgs(Vec<OsString>);

impl BwrapArgs {
    fn new() -> Self { Self(Vec::new()) }

    fn push(&mut self, s: impl Into<OsString>) { self.0.push(s.into()); }

    fn flag(&mut self, f: &str) { self.push(f); }

    fn ro_bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--ro-bind"); self.push(src); self.push(dst);
    }
    fn ro_bind_try(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--ro-bind-try"); self.push(src); self.push(dst);
    }
    fn bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--bind"); self.push(src); self.push(dst);
    }
    fn bind_try(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--bind-try"); self.push(src); self.push(dst);
    }
    fn dev_bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--dev-bind"); self.push(src); self.push(dst);
    }
    fn proc(&mut self, dst: &str)  { self.push("--proc");  self.push(dst); }
    fn dev(&mut self, dst: &str)   { self.push("--dev");   self.push(dst); }
    fn dir(&mut self, dst: impl Into<OsString>) { self.push("--dir"); self.push(dst); }
    fn tmpfs(&mut self, dst: impl Into<OsString>) { self.push("--tmpfs"); self.push(dst); }
    fn file(&mut self, fd: i32, dst: &str) {
        self.push("--file"); self.push(fd.to_string()); self.push(dst);
    }
    fn setenv(&mut self, key: &str, val: &str) {
        self.push("--setenv"); self.push(key); self.push(val);
    }
    fn hostname(&mut self, name: &str) { self.push("--hostname"); self.push(name); }

    fn clearenv(&mut self)      { self.flag("--clearenv"); }
    fn unshare_all(&mut self)   { self.flag("--unshare-all"); }
    fn share_net(&mut self)     { self.flag("--share-net"); }
    fn die_with_parent(&mut self) { self.flag("--die-with-parent"); }
    fn new_session(&mut self)   { self.flag("--new-session"); }

    fn exec(mut self, exe: &Path, args: &[OsString]) -> Vec<OsString> {
        self.flag("--");
        self.push(exe.as_os_str());
        self.0.extend_from_slice(args);
        self.0
    }
}

// ---------------------------------------------------------------------------
// Runtime entry point
// ---------------------------------------------------------------------------

/// Build bwrap args and exec into the sandboxed process. Never returns on success.
pub fn run_sandbox(args: &SandboxArgs, exe: &Path, exe_args: &[OsString]) -> io::Error {
    let home = env::var("HOME").unwrap_or_else(|_| "/home/user".into());
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let username = env::var("USER").unwrap_or_else(|_| uid.to_string());
    let groupname = unsafe {
        let gr = libc::getgrgid(gid);
        if gr.is_null() { None } else { std::ffi::CStr::from_ptr((*gr).gr_name).to_str().ok().map(|s| s.to_owned()) }
    }.unwrap_or_else(|| gid.to_string());

    let xdg = BaseDirectories::new();
    let xdg_runtime = xdg
        .get_runtime_directory()
        .map(|p: &PathBuf| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("/run/user/{}", uid));

    // bwrap's default uid map makes the sandbox process appear as uid 0.
    // Map uid 0 in passwd/group to the real username so getpwuid(0) returns
    // the correct name and home directory.
    let passwd_fd = write_pipe(format!("{}:x:0:0::{}:/bin/sh\n", username, home));
    let group_fd  = write_pipe(format!("{}:x:0:{}\n", groupname, username));

    let dbus = if args.need_dbus() {
        match spawn_dbus_proxy(args, &xdg_runtime) {
            Ok(d) => Some(d),
            Err(e) => { eprintln!("bubblewand: dbus proxy failed: {}", e); None }
        }
    } else {
        None
    };

    let pasta = if args.pasta {
        match spawn_pasta_orchestrator(args) {
            Ok(p) => Some(p),
            Err(e) => {
                return io::Error::new(e.kind(), format!("pasta orchestrator failed: {}", e));
            }
        }
    } else {
        None
    };

    let mut cmd = BwrapArgs::new();

    // Base filesystem
    cmd.proc("/proc");
    cmd.dev("/dev");
    if let Some(ref name) = args.share_tmp {
        let scoped = PathBuf::from(&xdg_runtime).join("bubblewand").join(name);
        let _ = fs::create_dir_all(&scoped);
        cmd.bind(scoped, "/tmp");
    } else {
        cmd.tmpfs("/tmp");
    }

    // Home: persistent or ephemeral
    if let Some(ref name) = args.persist_home {
        let xdg_bp = BaseDirectories::with_prefix("bubblewand");
        let persist = xdg_bp
            .create_data_directory(format!("{}/home", name))
            .unwrap_or_else(|_| PathBuf::from(&home).join(format!(".local/share/bubblewand/{}/home", name)));
        cmd.bind(persist, &home);
    } else {
        cmd.dir(&home);
    }

    // /etc with fake passwd + group + localtime + hostname
    cmd.tmpfs("/etc");
    if let Some(fd) = passwd_fd {
        cmd.file(fd, "/etc/passwd");
    }
    if let Some(fd) = group_fd {
        cmd.file(fd, "/etc/group");
    }
    if Path::new("/etc/localtime").exists() {
        cmd.ro_bind("/etc/localtime", "/etc/localtime");
    }
    if let Some(fd) = write_pipe(format!("{}\n", args.hostname)) {
        cmd.file(fd, "/etc/hostname");
    }

    // Isolation
    cmd.die_with_parent();
    cmd.unshare_all();

    if args.network {
        cmd.share_net();
    }

    if args.need_network_files() {
        for path in ["/etc/hosts", "/etc/nsswitch.conf"] {
            if Path::new(path).exists() {
                cmd.ro_bind(path, path);
            }
        }

        if Path::new("/etc/resolv.conf").exists() {
            cmd.ro_bind("/etc/resolv.conf", "/etc/resolv.conf");
        }

        // SSL certs: bind /etc/ssl, then also bind the intermediate and final
        // symlink targets so the NixOS chain resolves inside the sandbox.
        // On NixOS: /etc/ssl/certs/ca-*.crt → /etc/static/ssl/… → /nix/store/…
        // bwrap resolves symlinks on the source side, so --ro-bind-try on each
        // hop makes the full chain accessible at its expected path.
        cmd.ro_bind_try("/etc/ssl", "/etc/ssl");
        if Path::new("/etc/static/ssl").exists() {
            cmd.ro_bind_try("/etc/static/ssl", "/etc/static/ssl");
        }
        for cert in ["/etc/ssl/certs/ca-certificates.crt", "/etc/ssl/certs/ca-bundle.crt"] {
            if let Ok(real) = fs::canonicalize(cert) {
                cmd.ro_bind_try(&real, &real);
            }
        }
        for var in ["NIX_SSL_CERT_FILE", "SSL_CERT_FILE", "SSL_CERT_DIR"] {
            if let Ok(val) = env::var(var) {
                if let Ok(real) = fs::canonicalize(&val) {
                    cmd.ro_bind_try(&real, &real);
                }
                cmd.setenv(var, &val);
            }
        }
    }

    if let Some(ref p) = pasta {
        cmd.push("--info-fd"); cmd.push(p.info_fd.to_string());
        cmd.push("--block-fd"); cmd.push(p.block_fd.to_string());
    }

    cmd.hostname(&args.hostname);
    if !args.keep_env {
        cmd.clearenv();
    }
    cmd.setenv("HOME", &home);
    if let Ok(v) = env::var("TERM") { cmd.setenv("TERM", &v); }
    if let Ok(v) = env::var("LANG") { cmd.setenv("LANG", &v); }
    if let Ok(v) = env::var("TZ")   { cmd.setenv("TZ",   &v); }

    // XDG_RUNTIME_DIR — set once, many features need it
    if args.need_wayland() || args.need_pulse() || args.need_pipewire() || args.need_dbus() {
        cmd.setenv("XDG_RUNTIME_DIR", &xdg_runtime);
        cmd.dir(&xdg_runtime);
    }

    // GPU
    if args.gpu {
        if Path::new("/dev/dri").exists() {
            cmd.dev_bind("/dev/dri", "/dev/dri");
        }
        if Path::new("/sys/dev/char").exists() {
            cmd.ro_bind("/sys/dev/char", "/sys/dev/char");
        }
        for path in ["/run/opengl-driver", "/run/opengl-driver-32"] {
            if Path::new(path).exists() {
                cmd.ro_bind(path, path);
            }
        }
        for path in gpu_pci_paths() {
            cmd.ro_bind(&path, &path);
        }
    }

    // Wayland
    if args.need_wayland() {
        let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".into());
        let sock = format!("{}/{}", xdg_runtime, display);
        if Path::new(&sock).exists() {
            cmd.ro_bind(&sock, &sock);
        }
        cmd.setenv("WAYLAND_DISPLAY", &display);
        if let Ok(v) = env::var("XDG_SESSION_TYPE") {
            cmd.setenv("XDG_SESSION_TYPE", &v);
        }
    }

    // PulseAudio
    if args.need_pulse() {
        if Path::new("/run/pulse").exists() {
            cmd.bind_try("/run/pulse", "/run/pulse");
        }
        let pulse_sock = format!("{}/pulse", xdg_runtime);
        cmd.bind_try(&pulse_sock, &pulse_sock);
        if let Ok(v) = env::var("PULSE_SERVER") {
            cmd.setenv("PULSE_SERVER", &v);
        }
    }

    // PipeWire
    if args.need_pipewire() {
        if Path::new("/run/pipewire").exists() {
            cmd.bind_try("/run/pipewire", "/run/pipewire");
        }
        let pw_sock = format!("{}/pipewire-0", xdg_runtime);
        cmd.bind_try(&pw_sock, &pw_sock);
    }

    // GUI extras: fonts, dconf, cursors, XDG_DATA_DIRS
    if args.gui {
        cmd.ro_bind_try("/etc/fonts", "/etc/fonts");

        let dconf = format!("{}/.config/dconf", home);
        cmd.tmpfs(&dconf);
        cmd.ro_bind_try(&dconf, &dconf);

        if let Ok(dirs) = env::var("XDG_DATA_DIRS") {
            let mut resolved = Vec::new();
            for dir in dirs.split(':').filter(|d| !d.is_empty()) {
                if let Ok(real) = fs::canonicalize(dir) {
                    if real.is_dir() {
                        let real_str = real.to_string_lossy().into_owned();
                        if !real_str.starts_with("/nix/") {
                            cmd.ro_bind_try(&real_str, &real_str);
                        }
                        resolved.push(real_str);
                    }
                }
            }
            cmd.setenv("XDG_DATA_DIRS", &resolved.join(":"));
        }
        if let Ok(v) = env::var("XCURSOR_THEME") { cmd.setenv("XCURSOR_THEME", &v); }
        if let Ok(v) = env::var("XCURSOR_SIZE")  { cmd.setenv("XCURSOR_SIZE", &v); }
        if let Ok(paths) = env::var("XCURSOR_PATH") {
            cmd.setenv("XCURSOR_PATH", &paths);
            for dir in paths.split(':').filter(|d| !d.is_empty()) {
                if Path::new(dir).is_dir() {
                    cmd.ro_bind_try(dir, dir);
                }
            }
        }
    }

    // Camera
    if args.camera {
        for i in 0u32..=63 {
            let path = format!("/dev/video{}", i);
            if Path::new(&path).exists() {
                cmd.dev_bind(&path, &path);
            }
        }
    }

    // DBus proxy socket
    if let Some(ref d) = dbus {
        let dest = format!("{}/bus", xdg_runtime);
        cmd.ro_bind(&d.socket, &dest);
        cmd.setenv("DBUS_SESSION_BUS_ADDRESS", &format!("unix:path={}", dest));
        unsafe {
            let flags = libc::fcntl(d.lifetime_fd, libc::F_GETFD);
            libc::fcntl(d.lifetime_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }

    // User-supplied binds and env
    for spec in &args.ro_bind {
        if let Some((src, dst)) = spec.split_once(':') {
            cmd.ro_bind(src, dst);
        }
    }
    for spec in &args.rw_bind {
        if let Some((src, dst)) = spec.split_once(':') {
            cmd.bind(src, dst);
        }
    }
    for path in &args.tmpfs {
        cmd.tmpfs(path.as_str());
    }
    for kv in &args.set_env {
        if let Some((k, v)) = kv.split_once('=') {
            cmd.setenv(k, v);
        }
    }
    for key in &args.fwd_env {
        if let Ok(val) = env::var(key) {
            cmd.setenv(key, &val);
        }
    }

    if args.new_session {
        cmd.new_session();
    }

    // Exec bwrap — replaces this process
    let bwrap_bin = args.bwrap.as_deref().unwrap_or(BWRAP);
    use std::os::unix::process::CommandExt;
    Command::new(bwrap_bin).args(cmd.exec(exe, exe_args)).exec()
}

// ---------------------------------------------------------------------------
// Passwd pipe
// ---------------------------------------------------------------------------

fn write_pipe(content: impl AsRef<[u8]>) -> Option<i32> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let mut f = unsafe { fs::File::from_raw_fd(write_fd) };
    let _ = f.write_all(content.as_ref()); // drops + closes write_fd
    Some(read_fd)
}

// ---------------------------------------------------------------------------
// DBus proxy
// ---------------------------------------------------------------------------

pub struct DbusProxy {
    pub socket: String,
    pub lifetime_fd: i32,
    _child: std::process::Child,
}

fn spawn_dbus_proxy(args: &SandboxArgs, xdg_runtime: &str) -> io::Result<DbusProxy> {
    use std::os::unix::process::CommandExt;

    let dbus_addr = env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| {
        io::Error::new(io::ErrorKind::NotFound, "DBUS_SESSION_BUS_ADDRESS not set")
    })?;

    // Proxy writes a ready byte to the write end; parent keeps the read end
    // open in bwrap. When bwrap exits, POLLHUP fires on the proxy's write end
    // (G_IO_HUP in GLib) and the proxy exits.
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (parent_fd, proxy_fd) = (fds[0], fds[1]);

    let socket = format!("{}/bubblewand-dbus.sock", xdg_runtime);

    let mut cmd = Command::new(XDG_DBUS_PROXY);
    cmd.arg(&dbus_addr)
        .arg(&socket)
        .arg("--filter")
        .arg(format!("--fd={}", proxy_fd));
    for name in &args.dbus_talk {
        cmd.arg(format!("--talk={}", name));
    }
    for name in &args.dbus_own {
        cmd.arg(format!("--own={}", name));
    }

    // Clear CLOEXEC on proxy_fd so the child inherits it
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc::fcntl(proxy_fd, libc::F_GETFD);
            libc::fcntl(proxy_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            Ok(())
        });
    }

    let child = cmd.spawn()?;

    // Close proxy_fd in the parent — only the proxy needs it
    unsafe { libc::close(proxy_fd) };

    // Block until the proxy signals readiness. 5s timeout covers slow startup.
    let mut pfd = libc::pollfd { fd: parent_fd, events: libc::POLLIN | libc::POLLHUP, revents: 0 };
    let ret = unsafe { libc::poll(&mut pfd, 1, 5000) };
    let mut buf = [0u8; 1];
    let n = if ret > 0 {
        unsafe { libc::read(parent_fd, buf.as_mut_ptr() as *mut libc::c_void, 1) }
    } else {
        0
    };
    if n != 1 {
        unsafe { libc::close(parent_fd) };
        return Err(io::Error::new(io::ErrorKind::TimedOut, "dbus proxy did not become ready"));
    }

    Ok(DbusProxy { socket, lifetime_fd: parent_fd, _child: child })
}

// ---------------------------------------------------------------------------
// Pasta orchestrator
// ---------------------------------------------------------------------------
//
// When --pasta is set, bwrap creates a fresh netns (via --unshare-all without
// --share-net) but it has only `lo`. We bridge it to the host network via
// `pasta`, attached to the netns by PID. Sequence:
//
//   1. Parent (pre-exec) creates two pipes: info (bwrap -> orchestrator) and
//      block (orchestrator -> bwrap).
//   2. Parent forks an orchestrator child.
//   3. Parent clears CLOEXEC on the bwrap-side ends and passes them via
//      --info-fd / --block-fd, then execs into bwrap.
//   4. bwrap sets up namespaces, writes JSON containing child-pid to info-fd,
//      then blocks reading from block-fd.
//   5. Orchestrator reads the PID, spawns pasta against /proc/<pid>/ns/net,
//      writes a byte to block-fd to unblock, and waits on pasta. When the
//      sandbox exits and the netns is torn down, pasta exits, the orchestrator
//      exits.

pub struct PastaOrchestrator {
    pub info_fd: i32,
    pub block_fd: i32,
    pub _child_pid: libc::pid_t,
}

fn spawn_pasta_orchestrator(args: &SandboxArgs) -> io::Result<PastaOrchestrator> {
    let mut info = [0i32; 2];
    let mut block = [0i32; 2];
    if unsafe { libc::pipe2(info.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::pipe2(block.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (info_r, info_w) = (info[0], info[1]);
    let (block_r, block_w) = (block[0], block[1]);

    let tcp = args.pasta_tcp.clone();
    let udp = args.pasta_udp.clone();
    let mac = args.pasta_mac.clone();

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // Orchestrator child. Drop the bwrap-side fds.
        unsafe {
            libc::close(info_w);
            libc::close(block_r);
        }
        let code = orchestrator_main(info_r, block_w, &tcp, &udp, mac.as_deref());
        unsafe { libc::_exit(code) };
    }

    // Parent. Drop the orchestrator-side fds and clear CLOEXEC on the rest so
    // bwrap inherits them across exec.
    unsafe {
        libc::close(info_r);
        libc::close(block_w);
    }
    for fd in [info_w, block_r] {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
        }
    }

    Ok(PastaOrchestrator { info_fd: info_w, block_fd: block_r, _child_pid: pid })
}

fn orchestrator_main(info_fd: i32, block_fd: i32, tcp: &[String], udp: &[String], mac: Option<&str>) -> i32 {
    // Read bwrap's --info-fd JSON until we can extract child-pid.
    let mut buf = Vec::with_capacity(1024);
    let mut tmp = [0u8; 1024];
    let child_pid = loop {
        let n = unsafe {
            libc::read(info_fd, tmp.as_mut_ptr() as *mut libc::c_void, tmp.len())
        };
        if n <= 0 {
            break parse_child_pid(&buf);
        }
        buf.extend_from_slice(&tmp[..n as usize]);
        if let Some(p) = parse_child_pid(&buf) {
            break Some(p);
        }
    };
    let Some(pid) = child_pid else {
        eprintln!("bubblewand: pasta: could not read child PID from bwrap info-fd");
        return 1;
    };
    unsafe { libc::close(info_fd) };

    // Pass both userns and netns: bwrap's netns is owned by bwrap's userns, so
    // pasta needs to enter the userns to get CAP_SYS_ADMIN before it can join
    // the netns.
    //
    // We deliberately do NOT pass --foreground: pasta's default is to set up
    // the netns synchronously, then daemonize. The exit of the spawned
    // (parent) process is itself the readiness signal — when wait() returns,
    // the tap interface and DHCP/NDP responses are configured, and the
    // detached daemon child is running. No pid-file polling, no race.
    let netns = format!("/proc/{}/ns/net", pid);
    let userns = format!("/proc/{}/ns/user", pid);
    let mut cmd = Command::new(PASTA);
    cmd.arg("--quiet")
        .arg("--config-net")
        // Translate host-loopback forwards to namespace loopback so apps
        // bound to 127.0.0.1 inside the sandbox stay reachable via the host
        // loopback forward without per-app config tweaks.
        .arg("--host-lo-to-ns-lo")
        // Disable namespace -> host forwarding entirely. Pasta's defaults
        // (-T auto, -U auto) would bind every host-listening port inside the
        // namespace as a transparent forward, which both leaks host services
        // into the sandbox and steals ports the sandboxed app might want
        // (e.g. syncthing's 22000). For our app-sandbox use case the sandbox
        // should reach the internet, not host localhost services.
        .arg("-T").arg("none")
        .arg("-U").arg("none")
        .arg("--userns").arg(&userns)
        .arg("--netns").arg(&netns);
    if let Some(ref mac) = mac {
        cmd.arg("--ns-mac-addr").arg(mac);
    }
    for spec in tcp {
        cmd.arg("-t").arg(spec);
    }
    for spec in udp {
        cmd.arg("-u").arg(spec);
    }
    if tcp.is_empty() {
        cmd.arg("-t").arg("none");
    }
    if udp.is_empty() {
        cmd.arg("-u").arg("none");
    }

    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("bubblewand: pasta: failed to spawn: {}", e);
            let _ = unsafe { libc::write(block_fd, b"\0".as_ptr() as *const _, 1) };
            return 1;
        }
    };
    if !status.success() {
        eprintln!("bubblewand: pasta: setup failed: {}", status);
        let _ = unsafe { libc::write(block_fd, b"\0".as_ptr() as *const _, 1) };
        return 1;
    }

    // Pasta is up and daemonized; unblock bwrap.
    let _ = unsafe { libc::write(block_fd, b"\0".as_ptr() as *const _, 1) };
    unsafe { libc::close(block_fd) };
    0
}

fn parse_child_pid(buf: &[u8]) -> Option<u32> {
    let s = std::str::from_utf8(buf).ok()?;
    let i = s.find("\"child-pid\"")?;
    let rest = &s[i + "\"child-pid\"".len()..];
    let after_colon = rest.find(':')?;
    let digits: String = rest[after_colon + 1..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

fn gpu_pci_paths() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir("/sys/bus/pci/devices") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().join("drm").exists())
        .filter_map(|e| fs::canonicalize(e.path()).ok())
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sa(f: impl FnOnce(&mut SandboxArgs)) -> SandboxArgs {
        let mut a = SandboxArgs::default();
        f(&mut a);
        a
    }

    // -- need_* helpers --

    #[test]
    fn need_wayland_direct() { assert!(sa(|a| a.wayland = true).need_wayland()); }
    #[test]
    fn need_wayland_via_gui() { assert!(sa(|a| a.gui = true).need_wayland()); }
    #[test]
    fn need_wayland_off() { assert!(!SandboxArgs::default().need_wayland()); }

    #[test]
    fn need_pulse_direct() { assert!(sa(|a| a.pulse = true).need_pulse()); }
    #[test]
    fn need_pulse_via_audio() { assert!(sa(|a| a.audio = true).need_pulse()); }
    #[test]
    fn need_pulse_via_gui() { assert!(sa(|a| a.gui = true).need_pulse()); }

    #[test]
    fn need_pipewire_direct() { assert!(sa(|a| a.pipewire = true).need_pipewire()); }
    #[test]
    fn need_pipewire_via_audio() { assert!(sa(|a| a.audio = true).need_pipewire()); }
    #[test]
    fn need_pipewire_via_gui() { assert!(sa(|a| a.gui = true).need_pipewire()); }

    #[test]
    fn need_dbus_talk() { assert!(sa(|a| a.dbus_talk.push("org.foo".into())).need_dbus()); }
    #[test]
    fn need_dbus_own() { assert!(sa(|a| a.dbus_own.push("org.bar".into())).need_dbus()); }
    #[test]
    fn need_dbus_empty() { assert!(!SandboxArgs::default().need_dbus()); }

    // -- to_cli_args --

    #[test]
    fn cli_args_default_only_hostname() {
        assert_eq!(SandboxArgs::default().to_cli_args(), vec!["--hostname=bubble"]);
    }

    #[test]
    fn cli_args_flags() {
        let out = sa(|a| { a.gui = true; a.network = true; }).to_cli_args();
        assert!(out.contains(&"--gui".into()));
        assert!(out.contains(&"--network".into()));
    }

    #[test]
    fn cli_args_hostname_custom() {
        let out = sa(|a| a.hostname = "mybox".into()).to_cli_args();
        assert!(out.contains(&"--hostname=mybox".into()));
    }

    #[test]
    fn cli_args_multi_repeatable() {
        let out = sa(|a| {
            a.dbus_talk.push("org.foo".into());
            a.dbus_talk.push("org.bar".into());
        }).to_cli_args();
        assert!(out.contains(&"--dbus-talk=org.foo".into()));
        assert!(out.contains(&"--dbus-talk=org.bar".into()));
    }

    #[test]
    fn cli_args_persist_home() {
        let out = sa(|a| a.persist_home = Some("myapp".into())).to_cli_args();
        assert!(out.contains(&"--persist-home=myapp".into()));
    }

    #[test]
    fn cli_args_set_env() {
        let out = sa(|a| a.set_env.push("FOO=bar".into())).to_cli_args();
        assert!(out.contains(&"--set-env=FOO=bar".into()));
    }

    #[test]
    fn cli_args_ro_bind() {
        let out = sa(|a| a.ro_bind.push("/src:/dst".into())).to_cli_args();
        assert!(out.contains(&"--ro-bind=/src:/dst".into()));
    }

    #[test]
    fn cli_args_pasta_flag() {
        let out = sa(|a| a.pasta = true).to_cli_args();
        assert!(out.contains(&"--pasta".into()));
    }

    #[test]
    fn cli_args_pasta_tcp() {
        let out = sa(|a| {
            a.pasta = true;
            a.pasta_tcp.push("127.0.0.1/8384".into());
        }).to_cli_args();
        assert!(out.contains(&"--pasta-tcp=127.0.0.1/8384".into()));
    }

    #[test]
    fn cli_args_pasta_udp() {
        let out = sa(|a| {
            a.pasta = true;
            a.pasta_udp.push("21027".into());
        }).to_cli_args();
        assert!(out.contains(&"--pasta-udp=21027".into()));
    }

    #[test]
    fn need_network_files_via_network() {
        assert!(sa(|a| a.network = true).need_network_files());
    }

    #[test]
    fn need_network_files_via_pasta() {
        assert!(sa(|a| a.pasta = true).need_network_files());
    }

    #[test]
    fn need_network_files_off() {
        assert!(!SandboxArgs::default().need_network_files());
    }

    #[test]
    fn parse_child_pid_basic() {
        assert_eq!(parse_child_pid(b"{\"child-pid\": 12345}"), Some(12345));
    }

    #[test]
    fn parse_child_pid_no_space() {
        assert_eq!(parse_child_pid(b"{\"child-pid\":42}"), Some(42));
    }

    #[test]
    fn parse_child_pid_with_other_fields() {
        let buf = b"{\"child-pid\": 99, \"uid-map\": \"...\"}";
        assert_eq!(parse_child_pid(buf), Some(99));
    }

    #[test]
    fn parse_child_pid_partial() {
        assert_eq!(parse_child_pid(b"{\"child-pid\":"), None);
    }

    #[test]
    fn parse_child_pid_missing() {
        assert_eq!(parse_child_pid(b"{\"foo\": 1}"), None);
    }

}
