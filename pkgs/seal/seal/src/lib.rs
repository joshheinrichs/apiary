use std::ffi::OsString;
use std::io::{self, Write};
use std::os::fd::OwnedFd;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use xdg::BaseDirectories;

use clap::Args;

const BWRAP: &str = match option_env!("BWRAP") {
    Some(s) => s,
    None => "bwrap",
};
const XDG_DBUS_PROXY: &str = match option_env!("XDG_DBUS_PROXY") {
    Some(s) => s,
    None => "xdg-dbus-proxy",
};
const PASTA: &str = match option_env!("PASTA") {
    Some(s) => s,
    None => "pasta",
};
const CAGE: &str = match option_env!("CAGE") {
    Some(s) => s,
    None => "cage",
};
const PIPEWIRE: &str = match option_env!("PIPEWIRE") {
    Some(s) => s,
    None => "pipewire",
};
const WIREPLUMBER: &str = match option_env!("WIREPLUMBER") {
    Some(s) => s,
    None => "wireplumber",
};
const WIREPLUMBER_SHARE: &str = match option_env!("WIREPLUMBER_SHARE") {
    Some(s) => s,
    None => "",
};
const PIPEWIRE_SANDBOX_CONF: &str = match option_env!("PIPEWIRE_SANDBOX_CONF") {
    Some(s) => s,
    None => "pipewire-sandbox.conf",
};
const PIPEWIRE_SANDBOX_CAPTURE_CONF: &str = match option_env!("PIPEWIRE_SANDBOX_CAPTURE_CONF") {
    Some(s) => s,
    None => "pipewire-sandbox-capture.conf",
};

/// Inside a pasta sandbox: pasta forwards DNS sent here to the host's resolver.
const SANDBOX_DNS: &str = "169.254.1.1";
/// Inside a pasta sandbox: the host's loopback, mapped only when the host is reachable.
const SANDBOX_HOST: &str = "169.254.1.2";

// ---------------------------------------------------------------------------
// Network policy
// ---------------------------------------------------------------------------

/// Destinations a sandbox may reach.
///
/// `All` and `Zones` both run in a private netns bridged by pasta; `All`
/// skips the firewall. `Shared` joins the host netns outright.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Net {
    #[default]
    None,
    Shared,
    All,
    Zones(Zones),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Zones {
    pub internet: bool,
    pub lan: bool,
    pub host: bool,
}

impl Net {
    pub fn uses_pasta(&self) -> bool {
        matches!(self, Net::All | Net::Zones(_))
    }
    pub fn reaches_host(&self) -> bool {
        match self {
            Net::All => true,
            Net::Zones(z) => z.host,
            Net::None | Net::Shared => false,
        }
    }
}

impl std::str::FromStr for Net {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "none" => return Ok(Net::None),
            "shared" => return Ok(Net::Shared),
            "all" => return Ok(Net::All),
            _ => {}
        }
        let words: Vec<&str> = s.split(',').collect();
        if let Some(bad) = words
            .iter()
            .find(|w| !matches!(**w, "internet" | "lan" | "host"))
        {
            return Err(match *bad {
                "none" | "shared" | "all" => format!("`{bad}` cannot be combined with other zones"),
                _ => format!(
                    "unknown zone `{bad}` (expected internet, lan, host, all, shared or none)"
                ),
            });
        }
        Ok(Net::Zones(Zones {
            internet: words.contains(&"internet"),
            lan: words.contains(&"lan"),
            host: words.contains(&"host"),
        }))
    }
}

impl std::fmt::Display for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Net::None => f.write_str("none"),
            Net::Shared => f.write_str("shared"),
            Net::All => f.write_str("all"),
            Net::Zones(z) => {
                let words: Vec<&str> = [(z.internet, "internet"), (z.lan, "lan"), (z.host, "host")]
                    .into_iter()
                    .filter_map(|(on, w)| on.then_some(w))
                    .collect();
                f.write_str(&words.join(","))
            }
        }
    }
}

/// An inbound port forward, in pasta's -t/-u SPEC syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Publish {
    Tcp(String),
    Udp(String),
}

impl std::str::FromStr for Publish {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s.split_once(':') {
            Some(("tcp", spec)) => Ok(Publish::Tcp(spec.into())),
            Some(("udp", spec)) => Ok(Publish::Udp(spec.into())),
            _ => Err(format!("expected tcp:SPEC or udp:SPEC, got `{s}`")),
        }
    }
}

impl std::fmt::Display for Publish {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Publish::Tcp(spec) => write!(f, "tcp:{spec}"),
            Publish::Udp(spec) => write!(f, "udp:{spec}"),
        }
    }
}

/// pasta arguments for a policy, minus the namespace paths only known at runtime.
fn pasta_args(net: Net, publish: &[Publish], mac: Option<&str>) -> Vec<String> {
    let host = if net.reaches_host() {
        vec!["--map-host-loopback".into(), SANDBOX_HOST.into()]
    } else {
        vec!["--no-map-gw".into()]
    };
    let mac = mac
        .map(|m| vec!["--ns-mac-addr".into(), m.into()])
        .unwrap_or_default();
    let forwards = |flag: &str, specs: Vec<&String>| -> Vec<String> {
        if specs.is_empty() {
            return vec![flag.into(), "none".into()];
        }
        specs
            .into_iter()
            .flat_map(|s| [flag.into(), s.clone()])
            .collect()
    };
    let tcp = forwards(
        "-t",
        publish
            .iter()
            .filter_map(|p| match p {
                Publish::Tcp(s) => Some(s),
                Publish::Udp(_) => None,
            })
            .collect(),
    );
    let udp = forwards(
        "-u",
        publish
            .iter()
            .filter_map(|p| match p {
                Publish::Udp(s) => Some(s),
                Publish::Tcp(_) => None,
            })
            .collect(),
    );

    [
        vec![
            "--quiet".into(),
            "--config-net".into(),
            // Host-loopback forwards land on the sandbox's loopback, so apps
            // bound to 127.0.0.1 inside stay reachable through --publish.
            "--host-lo-to-ns-lo".into(),
            // No sandbox -> host port forwards: pasta's default (auto) mirrors
            // every host listener into the sandbox and steals those ports.
            "-T".into(),
            "none".into(),
            "-U".into(),
            "none".into(),
            "--dns-forward".into(),
            SANDBOX_DNS.into(),
        ],
        host,
        mac,
        tcp,
        udp,
    ]
    .concat()
}

/// nftables ruleset enforcing a zone set inside the sandbox netns. Matches
/// run top to bottom; whatever is left over is internet.
fn firewall_ruleset(zones: Zones) -> String {
    let verdict = |allowed: bool| if allowed { "accept" } else { "drop" };
    format!(
        "table inet seal {{
  chain output {{
    type filter hook output priority 0; policy {internet};
    oif lo accept
    ct state established,related accept
    ip daddr {dns_addr} meta l4proto {{ tcp, udp }} th dport 53 {dns}
    ip daddr {host_addr} {host}
    ip daddr {{ 0.0.0.0/8, 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16, 224.0.0.0/4, 255.255.255.255 }} {lan}
    ip6 daddr {{ ::1, fc00::/7, fe80::/10, ff00::/8 }} {lan}
  }}
}}
",
        internet = verdict(zones.internet),
        dns_addr = SANDBOX_DNS,
        // DNS is a tunnel to the internet, so a host-only sandbox gets none.
        dns = verdict(zones.internet || zones.lan),
        host_addr = SANDBOX_HOST,
        host = verdict(zones.host),
        lan = verdict(zones.lan),
    )
}

// ---------------------------------------------------------------------------
// CLI flags shared by both binaries
// ---------------------------------------------------------------------------

#[derive(Args, Clone, Debug)]
pub struct SandboxArgs {
    /// Enable GUI stack (wayland + audio + fonts + cursors)
    #[arg(long)]
    pub gui: bool,

    /// Playback-only audio via restricted PipeWire proxy (implied by --gui)
    #[arg(long)]
    pub audio: bool,

    /// Add microphone access on top of --audio (implies --audio)
    #[arg(long)]
    pub audio_capture: bool,

    /// What the sandbox can reach: none, shared, all, or a comma-separated
    /// set of internet, lan, host
    #[arg(long, value_name = "ZONES", default_value = "none")]
    pub net: Net,

    /// Forward a host port into the sandbox: tcp:SPEC or udp:SPEC, where SPEC
    /// is pasta's -t/-u syntax (repeatable)
    #[arg(long, value_name = "PROTO:SPEC")]
    pub publish: Vec<Publish>,

    /// MAC address for the pasta TAP interface (e.g. for stable device fingerprinting)
    #[arg(long = "pasta-mac", value_name = "ADDR")]
    pub pasta_mac: Option<String>,

    /// Full GPU access including /dev/dri/card* primary nodes (allows screen
    /// capture via DRM). Needed for direct DRM compositors.
    #[arg(long)]
    pub gpu: bool,

    /// Render-node-only GPU access: binds /dev/dri/renderD* but not card*.
    /// Sufficient for hardware-accelerated rendering and video decode/encode;
    /// the sandbox cannot read the host framebuffer via DRM.
    #[arg(long = "gpu-render", conflicts_with = "gpu")]
    pub gpu_render: bool,

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

    /// Pass a device node through at the same path, if it exists (repeatable)
    #[arg(long, value_name = "PATH")]
    pub device: Vec<String>,

    #[arg(long, default_value = "bubble")]
    pub hostname: String,

    #[arg(long = "new-session")]
    pub new_session: bool,

    /// Wrap in cage (nested Wayland compositor) for clipboard/screencopy isolation
    #[arg(long)]
    pub cage: bool,

    /// Share the desktop's X server. X11 has no isolation between clients:
    /// the app can read every other X client's input, windows and clipboard.
    #[arg(long)]
    pub x11: bool,

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
            gui: false,
            audio: false,
            audio_capture: false,
            net: Net::None,
            publish: Vec::new(),
            gpu: false,
            gpu_render: false,
            wayland: false,
            pulse: false,
            pipewire: false,
            camera: false,
            pasta_mac: None,
            new_session: false,
            keep_env: false,
            cage: false,
            x11: false,
            dbus_talk: Vec::new(),
            dbus_own: Vec::new(),
            persist_home: None,
            share_tmp: None,
            set_env: Vec::new(),
            fwd_env: Vec::new(),
            ro_bind: Vec::new(),
            rw_bind: Vec::new(),
            tmpfs: Vec::new(),
            device: Vec::new(),
            bwrap: None,
        }
    }
}

impl SandboxArgs {
    pub fn need_wayland(&self) -> bool {
        self.wayland || self.gui || self.cage
    }
    pub fn need_pulse(&self) -> bool {
        self.pulse || self.audio || self.audio_capture || self.gui
    }
    pub fn need_pipewire(&self) -> bool {
        self.pipewire || self.audio || self.audio_capture || self.gui
    }
    pub fn need_dbus(&self) -> bool {
        !self.dbus_talk.is_empty() || !self.dbus_own.is_empty()
    }
    pub fn need_network_files(&self) -> bool {
        self.net != Net::None
    }

    /// Serialize back to CLI args for embedding in wrapper scripts.
    pub fn to_cli_args(&self) -> Vec<String> {
        let mut out = Vec::new();

        macro_rules! flag {
            ($field:expr, $name:expr) => {
                if $field {
                    out.push($name.to_string());
                }
            };
        }
        macro_rules! opt {
            ($field:expr, $name:expr) => {
                if let Some(ref v) = $field {
                    out.push(format!("{}={}", $name, v));
                }
            };
        }
        macro_rules! multi {
            ($field:expr, $name:expr) => {
                for v in &$field {
                    out.push(format!("{}={}", $name, v));
                }
            };
        }

        flag!(self.gui, "--gui");
        flag!(self.audio, "--audio");
        flag!(self.audio_capture, "--audio-capture");
        if self.net != Net::None {
            out.push(format!("--net={}", self.net));
        }
        flag!(self.gpu, "--gpu");
        flag!(self.gpu_render, "--gpu-render");
        flag!(self.wayland, "--wayland");
        flag!(self.pulse, "--pulse");
        flag!(self.pipewire, "--pipewire");
        flag!(self.camera, "--camera");
        flag!(self.new_session, "--new-session");
        flag!(self.cage, "--cage");
        flag!(self.x11, "--x11");
        flag!(self.keep_env, "--keep-env");

        out.push(format!("--hostname={}", self.hostname));

        opt!(self.persist_home, "--persist-home");
        opt!(self.share_tmp, "--share-tmp");
        multi!(self.dbus_talk, "--dbus-talk");
        multi!(self.dbus_own, "--dbus-own");
        multi!(self.publish, "--publish");
        opt!(self.pasta_mac, "--pasta-mac");
        multi!(self.set_env, "--set-env");
        multi!(self.fwd_env, "--fwd-env");
        multi!(self.ro_bind, "--ro-bind");
        multi!(self.rw_bind, "--rw-bind");
        multi!(self.tmpfs, "--tmpfs");
        multi!(self.device, "--device");

        opt!(self.bwrap, "--bwrap");

        out
    }
}

// ---------------------------------------------------------------------------
// bwrap argument builder
// ---------------------------------------------------------------------------

struct BwrapArgs(Vec<OsString>);

impl BwrapArgs {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push(&mut self, s: impl Into<OsString>) {
        self.0.push(s.into());
    }

    fn flag(&mut self, f: &str) {
        self.push(f);
    }

    fn ro_bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--ro-bind");
        self.push(src);
        self.push(dst);
    }
    fn ro_bind_try(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--ro-bind-try");
        self.push(src);
        self.push(dst);
    }
    fn bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--bind");
        self.push(src);
        self.push(dst);
    }
    fn bind_try(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--bind-try");
        self.push(src);
        self.push(dst);
    }
    fn dev_bind(&mut self, src: impl Into<OsString>, dst: impl Into<OsString>) {
        self.push("--dev-bind");
        self.push(src);
        self.push(dst);
    }
    fn proc(&mut self, dst: &str) {
        self.push("--proc");
        self.push(dst);
    }
    fn dev(&mut self, dst: &str) {
        self.push("--dev");
        self.push(dst);
    }
    fn dir(&mut self, dst: impl Into<OsString>) {
        self.push("--dir");
        self.push(dst);
    }
    fn tmpfs(&mut self, dst: impl Into<OsString>) {
        self.push("--tmpfs");
        self.push(dst);
    }
    fn file(&mut self, fd: i32, dst: &str) {
        self.push("--file");
        self.push(fd.to_string());
        self.push(dst);
    }
    fn setenv(&mut self, key: &str, val: &str) {
        self.push("--setenv");
        self.push(key);
        self.push(val);
    }
    fn hostname(&mut self, name: &str) {
        self.push("--hostname");
        self.push(name);
    }

    fn clearenv(&mut self) {
        self.flag("--clearenv");
    }
    fn unshare_all(&mut self) {
        self.flag("--unshare-all");
    }
    fn share_net(&mut self) {
        self.flag("--share-net");
    }
    fn die_with_parent(&mut self) {
        self.flag("--die-with-parent");
    }
    fn new_session(&mut self) {
        self.flag("--new-session");
    }

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
    if !args.publish.is_empty() && !args.net.uses_pasta() {
        return io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "--publish needs a pasta network (--net=all or zones), not --net={}",
                args.net
            ),
        );
    }

    let home = env::var("HOME").unwrap_or_else(|_| "/home/user".into());
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let username = env::var("USER").unwrap_or_else(|_| uid.to_string());
    let groupname = unsafe {
        let gr = libc::getgrgid(gid);
        if gr.is_null() {
            None
        } else {
            std::ffi::CStr::from_ptr((*gr).gr_name)
                .to_str()
                .ok()
                .map(|s| s.to_owned())
        }
    }
    .unwrap_or_else(|| gid.to_string());

    let xdg = BaseDirectories::new();
    let xdg_runtime = xdg
        .get_runtime_directory()
        .map(|p: &PathBuf| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| format!("/run/user/{}", uid));

    // bwrap keeps the real uid/gid inside the sandbox. getpwuid(getuid()) has
    // to resolve: Xwayland only admits clients of the named local user.
    let passwd_fd = write_pipe(format!(
        "{}:x:{}:{}::{}:/bin/sh\n",
        username, uid, gid, home
    ));
    let group_fd = write_pipe(format!("{}:x:{}:{}\n", groupname, gid, username));

    // Everything this sandbox's services create lives in one private dir,
    // removed whole on teardown; no service's own cleanup is relied on.
    let seal_pid = std::process::id();
    let run_dir = format!("{}/seal-{}", xdg_runtime, seal_pid);
    let _ = fs::remove_dir_all(&run_dir);
    {
        use std::os::unix::fs::DirBuilderExt;
        if let Err(e) = fs::DirBuilder::new().mode(0o700).create(&run_dir) {
            return io::Error::new(e.kind(), format!("{}: {}", run_dir, e));
        }
    }
    let dbus_socket = args.need_dbus().then(|| format!("{}/dbus", run_dir));
    let pipewire_socket =
        (args.need_pipewire() && !args.pipewire).then(|| format!("{}/pipewire-0", run_dir));
    let cage_dir = args.cage.then(|| format!("{}/cage", run_dir));
    if let Some(dir) = &cage_dir {
        if let Err(e) = fs::create_dir(dir) {
            let _ = fs::remove_dir_all(&run_dir);
            return io::Error::new(e.kind(), format!("{}: {}", dir, e));
        }
    }

    let mut cmd = BwrapArgs::new();

    // Base filesystem
    cmd.proc("/proc");
    cmd.dev("/dev");
    if let Some(ref name) = args.share_tmp {
        let scoped = PathBuf::from(&xdg_runtime).join("seal").join(name);
        let _ = fs::create_dir_all(&scoped);
        cmd.bind(scoped, "/tmp");
    } else {
        cmd.tmpfs("/tmp");
    }
    if cage_dir.is_some() {
        cmd.dir("/tmp/.X11-unix");
        cmd.ro_bind_try("/tmp/.X11-unix/X0", "/tmp/.X11-unix/X0");
    }
    // Only the socket file: the abstract socket lives in the host netns,
    // which a sandbox never shares unless it uses --net=shared.
    if args.x11 {
        let display = env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        let number = display
            .trim_start_matches(':')
            .split('.')
            .next()
            .unwrap_or("0")
            .to_owned();
        let socket = format!("/tmp/.X11-unix/X{}", number);
        cmd.dir("/tmp/.X11-unix");
        cmd.ro_bind_try(&socket, &socket);
    }
    // Home: persistent or ephemeral
    if let Some(ref name) = args.persist_home {
        let xdg_bp = BaseDirectories::with_prefix("seal");
        let persist = xdg_bp
            .create_data_directory(format!("{}/home", name))
            .unwrap_or_else(|_| {
                PathBuf::from(&home).join(format!(".local/share/seal/{}/home", name))
            });
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
    // The container interface's marker: apps that know they're contained stop
    // waiting on host-only signals, e.g. SDL watches /dev/input instead of
    // udev events, which never reach the sandbox's netns.
    cmd.dir("/run/host");
    if let Some(fd) = write_pipe("seal\n") {
        cmd.file(fd, "/run/host/container-manager");
    }

    // Isolation
    cmd.die_with_parent();
    cmd.unshare_all();

    // Under pasta, "shared" is the netns seal prepared in enter_pasta_net.
    if args.net == Net::Shared || args.net.uses_pasta() {
        cmd.share_net();
    }

    if args.need_network_files() {
        for path in ["/etc/hosts", "/etc/nsswitch.conf"] {
            if Path::new(path).exists() {
                cmd.ro_bind(path, path);
            }
        }

        // Under pasta the host's resolvers may sit in a blocked zone, so DNS
        // goes to pasta's forwarder instead.
        if args.net.uses_pasta() {
            if let Some(fd) = write_pipe(format!("nameserver {}\n", SANDBOX_DNS)) {
                cmd.file(fd, "/etc/resolv.conf");
            }
        } else if Path::new("/etc/resolv.conf").exists() {
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
        for cert in [
            "/etc/ssl/certs/ca-certificates.crt",
            "/etc/ssl/certs/ca-bundle.crt",
        ] {
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

    cmd.hostname(&args.hostname);
    if !args.keep_env {
        cmd.clearenv();
    }
    cmd.setenv("HOME", &home);
    if let Ok(v) = env::var("TERM") {
        cmd.setenv("TERM", &v);
    }
    if let Ok(v) = env::var("LANG") {
        cmd.setenv("LANG", &v);
    }
    if let Ok(v) = env::var("TZ") {
        cmd.setenv("TZ", &v);
    }

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
    } else if args.gpu_render {
        // Bind only render nodes (renderD*). Skip primary nodes (card*) and
        // legacy control nodes (controlD*) — those expose scanout via DRM
        // GETFB/GETFB2 and would let the sandboxed app read the host screen.
        for path in render_nodes() {
            cmd.dev_bind(&path, &path);
        }
    }
    if args.gpu || args.gpu_render {
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

    if args.x11 {
        cmd.setenv(
            "DISPLAY",
            &env::var("DISPLAY").unwrap_or_else(|_| ":0".into()),
        );
    }

    // Wayland
    if args.need_wayland() {
        let (sock, display) = if let Some(ref dir) = cage_dir {
            (format!("{}/wayland-0", dir), "wayland-0".to_string())
        } else {
            let d = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".into());
            (format!("{}/{}", xdg_runtime, d), d)
        };
        cmd.ro_bind_try(&sock, &format!("{}/{}", xdg_runtime, display));
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
        if let Ok(v) = env::var("PULSE_SINK") {
            cmd.setenv("PULSE_SINK", &v);
        }
        if let Ok(v) = env::var("PIPEWIRE_PROPS") {
            cmd.setenv("PIPEWIRE_PROPS", &v);
        }
    }

    // PipeWire
    if args.need_pipewire() {
        if let Some(ref socket) = pipewire_socket {
            let dest = format!("{}/pipewire-0", xdg_runtime);
            cmd.bind_try(socket, &dest);
        } else {
            if Path::new("/run/pipewire").exists() {
                cmd.bind_try("/run/pipewire", "/run/pipewire");
            }
            let pw_sock = format!("{}/pipewire-0", xdg_runtime);
            cmd.bind_try(&pw_sock, &pw_sock);
        }
        if let Ok(v) = env::var("PIPEWIRE_PROPS") {
            cmd.setenv("PIPEWIRE_PROPS", &v);
        }
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
        if let Ok(v) = env::var("XCURSOR_THEME") {
            cmd.setenv("XCURSOR_THEME", &v);
        }
        if let Ok(v) = env::var("XCURSOR_SIZE") {
            cmd.setenv("XCURSOR_SIZE", &v);
        }
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
    if let Some(ref socket) = dbus_socket {
        let dest = format!("{}/bus", xdg_runtime);
        cmd.ro_bind(socket, &dest);
        cmd.setenv("DBUS_SESSION_BUS_ADDRESS", &format!("unix:path={}", dest));
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
    for path in &args.device {
        cmd.push("--dev-bind-try");
        cmd.push(path.as_str());
        cmd.push(path.as_str());
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

    let bwrap_args = cmd.exec(exe, exe_args);

    let services = start_services(
        args,
        &xdg_runtime,
        &run_dir,
        dbus_socket.as_deref(),
        pipewire_socket.as_deref(),
    );
    let status = services.and_then(|services| {
        let status = app_command(
            args,
            &xdg_runtime,
            cage_dir.as_deref(),
            &bwrap_args,
            seal_pid,
        )
        .status();
        stop(services);
        status
    });
    let _ = fs::remove_dir_all(&run_dir);

    use std::os::unix::process::ExitStatusExt;
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))),
        Err(e) => e,
    }
}

/// bwrap, or cage running bwrap. Dies with seal like every service.
fn app_command(
    args: &SandboxArgs,
    xdg_runtime: &str,
    cage_dir: Option<&str>,
    bwrap_args: &[OsString],
    seal_pid: u32,
) -> Command {
    use std::os::unix::process::CommandExt;
    let bwrap_bin = args.bwrap.as_deref().unwrap_or(BWRAP);
    let mut cmd = match cage_dir {
        Some(dir) => {
            let host_display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-1".into());
            let host_socket = if host_display.starts_with('/') {
                host_display
            } else {
                format!("{}/{}", xdg_runtime, host_display)
            };
            let mut c = Command::new(CAGE);
            c.env("XDG_RUNTIME_DIR", dir)
                .env("WAYLAND_DISPLAY", host_socket)
                .arg("--")
                .arg(bwrap_bin);
            c
        }
        None => Command::new(bwrap_bin),
    };
    cmd.args(bwrap_args);
    unsafe { cmd.pre_exec(move || die_with_seal(libc::SIGKILL, seal_pid)) };
    cmd
}

/// Start every service at once, then wait for all of them. On any failure the
/// ones already started are stopped and the app never runs.
fn start_services(
    args: &SandboxArgs,
    xdg_runtime: &str,
    run_dir: &str,
    dbus_socket: Option<&str>,
    pipewire_socket: Option<&str>,
) -> io::Result<Vec<Service>> {
    let mut services = Vec::new();
    let started = (|| {
        if let Some(socket) = dbus_socket {
            services.push(spawn_dbus_proxy(args, socket)?);
        }
        if let Some(socket) = pipewire_socket {
            services.extend(spawn_pipewire_proxy(
                xdg_runtime,
                run_dir,
                socket,
                args.audio_capture,
            )?);
        }
        // Last: seal itself moves into the sandbox netns here.
        if args.net.uses_pasta() {
            services.push(spawn_network(args)?);
        }
        await_ready(&services)
    })();
    match started {
        Ok(()) => Ok(services),
        Err(e) => {
            stop(services);
            Err(e)
        }
    }
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
// Services
// ---------------------------------------------------------------------------
//
// The helpers an app needs (dbus proxy, pipewire, network) all start at once,
// and the app starts once every one is ready:
//
//   - ready fd: ready when it yields a byte; EOF, or the process exiting
//     first, is a failure. One poll() waits on all of them.
//   - no ready fd: ready at spawn, because seal created its listening socket.
//
// Every service and the app get PR_SET_PDEATHSIG, so nothing outlives seal.
// That fires when the parent *thread* exits; seal is single-threaded, so that
// is exactly when seal exits. On a normal exit seal kills them itself.

struct Service {
    name: &'static str,
    pid: libc::pid_t,
    pidfd: OwnedFd,
    /// Held open until teardown: xdg-dbus-proxy exits when it closes.
    ready: Option<OwnedFd>,
}

fn pidfd_open(pid: libc::pid_t) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// Runs in a fresh child of seal. The getppid check covers seal dying between
/// fork and prctl, which PDEATHSIG alone would miss.
fn die_with_seal(signal: libc::c_int, seal_pid: u32) -> io::Result<()> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, signal as libc::c_ulong, 0, 0, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::getppid() } as u32 != seal_pid {
        return Err(io::Error::other("seal exited during startup"));
    }
    Ok(())
}

fn spawn_service(
    name: &'static str,
    mut cmd: Command,
    ready: Option<OwnedFd>,
) -> io::Result<Service> {
    use std::os::unix::process::CommandExt;
    let seal_pid = std::process::id();
    unsafe { cmd.pre_exec(move || die_with_seal(libc::SIGKILL, seal_pid)) };
    let child = cmd
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("{}: {}", name, e)))?;
    let pid = child.id() as libc::pid_t;
    Ok(Service {
        name,
        pid,
        pidfd: pidfd_open(pid)?,
        ready,
    })
}

/// Block until every service with a ready fd has signalled.
fn await_ready(services: &[Service]) -> io::Result<()> {
    let mut waiting: Vec<&Service> = services.iter().filter(|s| s.ready.is_some()).collect();
    while !waiting.is_empty() {
        let mut fds: Vec<libc::pollfd> = waiting
            .iter()
            .flat_map(|s| {
                [
                    s.ready.as_ref().map(|f| f.as_raw_fd()).unwrap_or(-1),
                    s.pidfd.as_raw_fd(),
                ]
                .map(|fd| libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                })
            })
            .collect();
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        let mut still_waiting = Vec::new();
        for (service, pair) in waiting.iter().zip(fds.chunks(2)) {
            // Ready is checked first: a helper may signal and exit together.
            if pair[0].revents != 0 {
                let mut byte = [0u8];
                let n =
                    unsafe { libc::read(pair[0].fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
                if n != 1 {
                    return Err(io::Error::other(format!(
                        "{} failed to start",
                        service.name
                    )));
                }
                continue;
            }
            if pair[1].revents != 0 {
                return Err(io::Error::other(format!(
                    "{} exited before it was ready",
                    service.name
                )));
            }
            still_waiting.push(*service);
        }
        waiting = still_waiting;
    }
    Ok(())
}

fn stop(services: Vec<Service>) {
    for s in &services {
        unsafe { libc::kill(s.pid, libc::SIGKILL) };
    }
    for s in services {
        unsafe { libc::waitpid(s.pid, std::ptr::null_mut(), 0) };
    }
}

fn spawn_dbus_proxy(args: &SandboxArgs, socket: &str) -> io::Result<Service> {
    use std::os::unix::process::CommandExt;

    let dbus_addr = env::var("DBUS_SESSION_BUS_ADDRESS")
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "DBUS_SESSION_BUS_ADDRESS not set"))?;
    let (ready_r, ready_w) = cloexec_pipe()?;
    let proxy_fd = ready_w.as_raw_fd();

    let mut cmd = Command::new(XDG_DBUS_PROXY);
    cmd.arg(&dbus_addr)
        .arg(socket)
        .arg("--filter")
        .arg(format!("--fd={}", proxy_fd))
        .args(args.dbus_talk.iter().map(|n| format!("--talk={}", n)))
        .args(args.dbus_own.iter().map(|n| format!("--own={}", n)));
    unsafe {
        cmd.pre_exec(move || {
            let flags = libc::fcntl(proxy_fd, libc::F_GETFD);
            libc::fcntl(proxy_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            Ok(())
        });
    }
    let service = spawn_service("dbus proxy", cmd, Some(ready_r))?;
    drop(ready_w);
    Ok(service)
}

/// pipewire + wireplumber. pipewire is socket-activated: seal binds the
/// listening socket, so it is connectable before pipewire even runs.
fn spawn_pipewire_proxy(
    xdg_runtime: &str,
    run_dir: &str,
    socket: &str,
    capture: bool,
) -> io::Result<Vec<Service>> {
    use std::os::unix::process::CommandExt;

    let name = Path::new(socket)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    let conf = if capture {
        PIPEWIRE_SANDBOX_CAPTURE_CONF
    } else {
        PIPEWIRE_SANDBOX_CONF
    };

    let listener = std::os::unix::net::UnixListener::bind(socket)?;
    let listen_fd = listener.as_raw_fd();

    let mut pw = Command::new(PIPEWIRE);
    pw.arg("-c")
        .arg(conf)
        .env("PIPEWIRE_CORE", &name)
        .env("XDG_RUNTIME_DIR", run_dir)
        .env("PULSE_SERVER", format!("unix:{}/pulse/native", xdg_runtime))
        .env("LISTEN_FDS", "1");
    unsafe {
        pw.pre_exec(move || {
            // Socket activation hands fds over starting at 3.
            if listen_fd == 3 {
                let flags = libc::fcntl(3, libc::F_GETFD);
                libc::fcntl(3, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
            } else if libc::dup2(listen_fd, 3) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let pipewire = spawn_service("pipewire", pw, None)?;
    drop(listener);

    let mut wp = Command::new(WIREPLUMBER);
    wp.arg("--profile")
        .arg("policy")
        .env("PIPEWIRE_REMOTE", &name)
        .env("XDG_RUNTIME_DIR", run_dir);
    if !WIREPLUMBER_SHARE.is_empty() {
        wp.env("XDG_DATA_DIRS", WIREPLUMBER_SHARE);
    }
    let wireplumber = spawn_service("wireplumber", wp, None)?;

    Ok(vec![pipewire, wireplumber])
}

// ---------------------------------------------------------------------------
// Pasta network
// ---------------------------------------------------------------------------
//
// For `--net=all` and zone sets, seal builds the sandbox's network before bwrap
// exists, then runs bwrap inside it with --share-net:
//
//   1. Fork a helper. It stays in the host netns, which pasta needs for its
//      outbound sockets.
//   2. seal unshares a user + net namespace and maps its own uid into it.
//   3. The helper loads the zone firewall into that netns, then attaches
//      pasta, then signals ready. Firewall first, so the link never comes up
//      unfiltered.
//   4. bwrap's payload lands in a user namespace nested below the netns
//      owner and so holds no capabilities over the firewall.
//
// Acting on bwrap's own child instead races it: bwrap reports the PID right
// after clone(), before writing its uid map, and later moves into a nested
// userns for devpts.

/// Move seal into a fresh user + net namespace and start the helper that
/// firewalls and bridges it. Must be the last service spawned: everything
/// after it inherits the new netns.
fn spawn_network(args: &SandboxArgs) -> io::Result<Service> {
    let pasta = pasta_args(args.net, &args.publish, args.pasta_mac.as_deref());
    let firewall = match args.net {
        Net::Zones(zones) => Some(firewall_ruleset(zones)),
        Net::None | Net::Shared | Net::All => None,
    };
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let seal_pid = std::process::id();

    let (entered_r, entered_w) = cloexec_pipe()?;
    let (ready_r, ready_w) = cloexec_pipe()?;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        drop(entered_w);
        drop(ready_r);
        let mut byte = [0u8];
        let entered = die_with_seal(libc::SIGKILL, seal_pid).is_ok()
            && unsafe {
                libc::read(
                    entered_r.as_raw_fd(),
                    byte.as_mut_ptr() as *mut libc::c_void,
                    1,
                )
            } == 1;
        let ok = entered
            && setup_pasta_net(seal_pid, &pasta, firewall.as_deref())
                .map_err(|e| eprintln!("seal: network: {}", e))
                .is_ok();
        if ok {
            unsafe {
                libc::write(
                    ready_w.as_raw_fd(),
                    [1u8].as_ptr() as *const libc::c_void,
                    1,
                )
            };
        }
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    drop(entered_r);
    drop(ready_w);
    let service = Service {
        name: "network",
        pid,
        pidfd: pidfd_open(pid)?,
        ready: Some(ready_r),
    };

    // On failure the helper reads EOF and exits; await_ready never runs.
    enter_user_net_namespace(uid, gid)?;
    unsafe {
        libc::write(
            entered_w.as_raw_fd(),
            [1u8].as_ptr() as *const libc::c_void,
            1,
        )
    };
    Ok(service)
}

fn cloexec_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

fn enter_user_net_namespace(uid: libc::uid_t, gid: libc::gid_t) -> io::Result<()> {
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) } != 0 {
        return Err(io::Error::last_os_error());
    }
    fs::write("/proc/self/setgroups", "deny")?;
    fs::write("/proc/self/uid_map", format!("{uid} {uid} 1"))?;
    fs::write("/proc/self/gid_map", format!("{gid} {gid} 1"))?;
    Ok(())
}

/// Runs in the helper, in the host netns, once seal is in its new namespaces.
fn setup_pasta_net(seal_pid: u32, pasta: &[String], firewall: Option<&str>) -> io::Result<()> {
    let userns = format!("/proc/{}/ns/user", seal_pid);
    let netns = format!("/proc/{}/ns/net", seal_pid);

    if let Some(rules) = firewall {
        load_firewall(&userns, &netns, rules)?;
    }

    // pasta configures the netns, then daemonizes; its exit is the readiness
    // signal. The daemon quits when the netns goes away.
    let status = Command::new(PASTA)
        .args(pasta)
        .arg("--userns")
        .arg(&userns)
        .arg("--netns")
        .arg(&netns)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("pasta exited with {}", status)));
    }
    Ok(())
}

#[link(name = "nftables")]
unsafe extern "C" {
    fn nft_ctx_new(flags: u32) -> *mut libc::c_void;
    fn nft_run_cmd_from_buffer(ctx: *mut libc::c_void, buf: *const libc::c_char) -> libc::c_int;
}

/// Load an nftables ruleset into a netns from a child that joins the netns's
/// owning userns. libnftables runs in-process: an exec would drop the
/// capabilities setns just granted.
fn load_firewall(userns: &str, netns: &str, rules: &str) -> io::Result<()> {
    let user = fs::File::open(userns)?;
    let net = fs::File::open(netns)?;
    let rules = std::ffi::CString::new(rules).map_err(io::Error::other)?;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        let loaded = unsafe {
            libc::setns(user.as_raw_fd(), libc::CLONE_NEWUSER) == 0
                && libc::setns(net.as_raw_fd(), libc::CLONE_NEWNET) == 0
                && {
                    let ctx = nft_ctx_new(0);
                    !ctx.is_null() && nft_run_cmd_from_buffer(ctx, rules.as_ptr()) == 0
                }
        };
        unsafe { libc::_exit(if loaded { 0 } else { 1 }) };
    }

    let mut status = 0i32;
    unsafe { libc::waitpid(pid, &mut status, 0) };
    if !(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0) {
        return Err(io::Error::other("firewall could not be loaded"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Device discovery
// ---------------------------------------------------------------------------

fn render_nodes() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir("/dev/dri") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("renderD"))
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect()
}

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
    fn need_wayland_direct() {
        assert!(sa(|a| a.wayland = true).need_wayland());
    }
    #[test]
    fn need_wayland_via_gui() {
        assert!(sa(|a| a.gui = true).need_wayland());
    }
    #[test]
    fn need_wayland_via_cage() {
        assert!(sa(|a| a.cage = true).need_wayland());
    }
    #[test]
    fn need_wayland_off() {
        assert!(!SandboxArgs::default().need_wayland());
    }

    #[test]
    fn need_pulse_direct() {
        assert!(sa(|a| a.pulse = true).need_pulse());
    }
    #[test]
    fn need_pulse_via_audio() {
        assert!(sa(|a| a.audio = true).need_pulse());
    }
    #[test]
    fn need_pulse_via_gui() {
        assert!(sa(|a| a.gui = true).need_pulse());
    }

    #[test]
    fn need_pipewire_direct() {
        assert!(sa(|a| a.pipewire = true).need_pipewire());
    }
    #[test]
    fn need_pipewire_via_audio() {
        assert!(sa(|a| a.audio = true).need_pipewire());
    }
    #[test]
    fn need_pipewire_via_audio_capture() {
        assert!(sa(|a| a.audio_capture = true).need_pipewire());
    }
    #[test]
    fn need_pipewire_via_gui() {
        assert!(sa(|a| a.gui = true).need_pipewire());
    }
    #[test]
    fn need_pulse_via_audio_capture() {
        assert!(sa(|a| a.audio_capture = true).need_pulse());
    }

    #[test]
    fn need_dbus_talk() {
        assert!(sa(|a| a.dbus_talk.push("org.foo".into())).need_dbus());
    }
    #[test]
    fn need_dbus_own() {
        assert!(sa(|a| a.dbus_own.push("org.bar".into())).need_dbus());
    }
    #[test]
    fn need_dbus_empty() {
        assert!(!SandboxArgs::default().need_dbus());
    }

    // -- to_cli_args --

    #[test]
    fn cli_args_default_only_hostname() {
        assert_eq!(
            SandboxArgs::default().to_cli_args(),
            vec!["--hostname=bubble"]
        );
    }

    #[test]
    fn cli_args_flags() {
        let out = sa(|a| {
            a.gui = true;
            a.cage = true;
        })
        .to_cli_args();
        assert!(out.contains(&"--gui".into()));
        assert!(out.contains(&"--cage".into()));
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
        })
        .to_cli_args();
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
    fn cli_args_cage() {
        let out = sa(|a| a.cage = true).to_cli_args();
        assert!(out.contains(&"--cage".into()));
    }

    #[test]
    fn cli_args_audio_capture() {
        let out = sa(|a| a.audio_capture = true).to_cli_args();
        assert!(out.contains(&"--audio-capture".into()));
    }

    #[test]
    fn cli_args_net_and_publish() {
        let out = sa(|a| {
            a.net = "internet,lan".parse().unwrap();
            a.publish.push("tcp:127.0.0.1/8384".parse().unwrap());
        })
        .to_cli_args();
        assert!(out.contains(&"--net=internet,lan".into()));
        assert!(out.contains(&"--publish=tcp:127.0.0.1/8384".into()));
    }

    #[test]
    fn net_parse_round_trips() {
        for s in [
            "none",
            "shared",
            "all",
            "internet",
            "lan,host",
            "internet,lan,host",
        ] {
            assert_eq!(s.parse::<Net>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn net_parse_rejects_mixed_and_unknown() {
        assert!("all,lan".parse::<Net>().is_err());
        assert!("internet,shared".parse::<Net>().is_err());
        assert!("wan".parse::<Net>().is_err());
    }

    #[test]
    fn need_network_files_by_net() {
        assert!(sa(|a| a.net = Net::Shared).need_network_files());
        assert!(sa(|a| a.net = Net::All).need_network_files());
        assert!(!SandboxArgs::default().need_network_files());
    }
}
