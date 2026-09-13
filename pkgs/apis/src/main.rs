//! apis — evaluate/build/run/repl rooted at the git working tree you're standing
//! in, sandboxed (pure eval) so it can only ever see that folder.
//!
//! Model: one irreducible impure step snapshots the source — the git-filtered
//! working tree (dirty TRACKED files in, untracked/gitignored out), or the tree
//! of a commit with `--ref` — grabs its narHash, and reads the host system.
//! Everything after that is pure eval with `self` locked to that snapshot and
//! the host system injected — pure eval has no `currentSystem`, so apis
//! supplies it.
//!
//! Structure: the pure core (below) only builds Nix expression strings and makes
//! decisions — no IO, no Nix calls. The effectful layer evaluates/builds/links.
//! `main` is the only place that sequences side effects.

use anyhow::{bail, Context as _, Result};
use clap::{Parser, Subcommand};
use nix_bindings_expr::eval_state::{gc_register_my_thread, init, EvalState};
use nix_bindings_expr::value::{Value, ValueType};
use nix_bindings_store::store::Store;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "apis", about = "Pure, sandboxed eval/build/run/repl rooted at the repo you're in")]
struct Cli {
    /// Git revision to evaluate instead of the working tree: a branch, tag, sha,
    /// or anything `git rev-parse` takes (`HEAD~3`, `v1.2`).
    #[arg(long = "ref", global = true, value_name = "REF")]
    git_ref: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Evaluate and print an attribute (or --expr) from the repo.
    Eval(Target),
    /// Build a derivation attribute (or --expr) and link ./result.
    Build(Target),
    /// Instantiate a derivation attribute (or --expr) and print its `.drv` path.
    Instantiate(Target),
    /// Build then run a derivation attribute (or --expr).
    Run {
        #[command(flatten)]
        target: Target,
        /// Arguments passed to the built program.
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Open a Nix repl with the repo attrs and `self` in scope.
    Repl,
}

#[derive(clap::Args)]
struct Target {
    /// Attribute path into the repo, e.g. `desktop-home-applicator` or `pkgs.foo.bar`.
    attr: Option<String>,
    /// Raw Nix expression evaluated with `self` (the repo source) in scope.
    #[arg(short = 'e', long = "expr", conflicts_with = "attr")]
    expr: Option<String>,
}

// ---------------------------------------------------------------------------
// Pure core: data + Nix expression construction. No IO, no Nix calls.
// ---------------------------------------------------------------------------

/// The source, pinned to a content hash so pure eval will accept it.
enum Locked {
    /// The working-tree snapshot already sitting in the store, pinned by the
    /// narHash we measured for it.
    Snapshot { store_path: String, nar_hash: String },
    /// A revision, pinned by rev + narHash — enough for `fetchGit` to reproduce
    /// it under pure eval, and what makes it materialize on first read.
    Revision { root: String, rev: String, nar_hash: String },
}

/// Escape a Nix string literal: backslash, quote, and the `${` interpolation
/// opener are the only sequences that can break out.
fn escape_nix_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace("${", "\\${")
}

/// Every path the snapshot must admit: the files themselves plus each ancestor
/// directory, since `builtins.path`'s filter is asked about directories too and
/// a rejected directory prunes everything under it.
fn allowed_paths(files: &[String]) -> BTreeSet<String> {
    let mut allowed = BTreeSet::new();
    for f in files {
        let mut acc = String::new();
        for part in f.split('/') {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            allowed.insert(acc.clone());
        }
    }
    allowed
}

/// Where `self` comes from: the working tree as it is on disk, or a committed
/// revision — the only way apis reaches a version not in the checkout.
enum Source {
    WorkingTree(Vec<String>),
    Revision(String),
}

/// The expression that puts the source in the store.
///
/// Working tree: admit exactly `files` (tracked ∪ untracked-not-ignored). The
/// filter set is a lookup attrset, so deciding a path is one hash probe rather
/// than a scan.
///
/// Revision: hand it to `fetchGit`, which exports that commit's tree — tracked
/// files only, no `.git`, the dirty checkout ignored — and caches it per rev.
/// `allRefs` so a rev off the checked-out branch still resolves.
fn source_expr(root: &str, source: &Source) -> String {
    let root = escape_nix_string(root);
    let files = match source {
        Source::Revision(rev) => {
            return format!(
                "(builtins.fetchGit {{ url = \"{root}\"; rev = \"{rev}\"; allRefs = true; }})"
            )
        }
        Source::WorkingTree(files) => files,
    };
    let names = allowed_paths(files)
        .iter()
        .map(|p| format!("\"{}\"", escape_nix_string(p)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "(let root = \"{root}\"; \
            allowed = builtins.listToAttrs (map (n: {{ name = n; value = true; }}) [ {names} ]); \
          in builtins.path {{ \
            name = \"source\"; \
            path = root; \
            filter = path: _type: \
              builtins.hasAttr \
                (builtins.substring (builtins.stringLength root + 1) (builtins.stringLength path) path) \
                allowed; \
          }})"
    )
}

/// The one impure expression: produce the source's pin and read the host system
/// (unavailable later under pure eval). `pin` is whatever identifies the source
/// afterwards — the snapshot's store path, which we hash next, or the narHash
/// `fetchGit` already computed.
fn probe_expr(root: &str, source: &Source) -> String {
    let src = source_expr(root, source);
    let pin = match source {
        Source::WorkingTree(_) => format!("\"${{{src}}}\""),
        Source::Revision(_) => format!("{src}.narHash"),
    };
    format!("{{ pin = {pin}; s = builtins.currentSystem; }}")
}

/// `self` as a pure-eval-safe value, locked. A revision stays a `fetchGit` call
/// rather than a store path: its store path is not materialized until something
/// reads it, and `builtins.path` would read the filesystem instead of fetching.
fn self_expr(locked: &Locked) -> String {
    match locked {
        Locked::Snapshot { store_path, nar_hash } => format!(
            "builtins.path {{ name = \"source\"; path = \"{store_path}\"; sha256 = \"{nar_hash}\"; }}"
        ),
        Locked::Revision { root, rev, nar_hash } => format!(
            "builtins.fetchGit {{ url = \"{root}\"; rev = \"{rev}\"; \
               narHash = \"{nar_hash}\"; allRefs = true; }}",
            root = escape_nix_string(root)
        ),
    }
}

/// Import the repo. If the entrypoint is a function, apply it with the host
/// system — pure eval has no `builtins.currentSystem`, so the entry point cannot
/// read it itself. `intersectAttrs` filters to the args it actually declares, so
/// a no-arg function or a plain attrset entry still works.
fn import_repo(system: &str) -> String {
    format!(
        "(let r = import self; in \
           if builtins.isFunction r \
           then r (builtins.intersectAttrs (builtins.functionArgs r) {{ system = \"{system}\"; }}) \
           else r)"
    )
}

/// The expression body, given a scope where `self` is bound. Attribute form
/// selects the attrpath from the imported repo; `--expr` is used verbatim;
/// neither yields the whole repo.
fn body_expr(attr: Option<&str>, expr: Option<&str>, system: &str) -> String {
    match (attr, expr) {
        (_, Some(e)) => format!("({e})"),
        (Some(a), None) => format!("({}).{a}", import_repo(system)),
        (None, None) => import_repo(system),
    }
}

/// Wrap a body so it is evaluated with `self` as a function argument.
fn with_self(body: &str) -> String {
    format!("self: {body}")
}

/// Expression loading the repo attrs and `self` into a standalone repl scope
/// (used by the `nix repl` subprocess, so `self` is inlined).
fn repl_expr(locked: &Locked, system: &str) -> String {
    format!(
        "let self = {}; in {} // {{ inherit self; }}",
        self_expr(locked),
        import_repo(system)
    )
}

/// Last component of an attrpath, used as the default program name for `run`.
fn last_component(attr: &str) -> &str {
    attr.rsplit('.').next().unwrap_or(attr)
}

/// Choose which executable in `bin/` to run: a name match if given, else the
/// sole entry. Pure: operates on an already-listed set of paths.
fn pick_bin(entries: &[PathBuf], name: Option<&str>) -> Result<PathBuf> {
    if let Some(name) = name {
        if let Some(p) = entries.iter().find(|p| file_name(p) == Some(name)) {
            return Ok(p.clone());
        }
    }
    match entries {
        [one] => Ok(one.clone()),
        [] => bail!("no executables in the output's bin/"),
        _ => bail!(
            "multiple programs in bin/; set meta.mainProgram on the package, \
             or name the attribute after the binary"
        ),
    }
}

fn file_name(p: &Path) -> Option<&str> {
    p.file_name().and_then(|n| n.to_str())
}

// ---------------------------------------------------------------------------
// Effectful layer: Nix evaluation.
// ---------------------------------------------------------------------------

fn open_eval() -> Result<EvalState> {
    let store = Store::open(None, []).context("opening the Nix store")?;
    EvalState::new(store, []).context("creating an eval state")
}

/// The working tree's file set, in one `git` call: tracked ∪ untracked, with
/// gitignored paths excluded by `--exclude-standard`. Symlinks into the store
/// are dropped — `./result*` links are untracked and unignored here, and a NAR
/// records a symlink by its target, so letting them in would change the
/// snapshot hash on every build and invalidate `self` against itself.
fn git_files(root: &str) -> Result<Vec<String>> {
    let out = std::process::Command::new(git_bin())
        .args(["-C", root, "ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .context("running git ls-files")?;
    if !out.status.success() {
        bail!("git ls-files failed in {root} — not a git repository?");
    }
    let listing = String::from_utf8(out.stdout).context("git ls-files emitted non-UTF-8 paths")?;
    Ok(listing
        .split('\0')
        .filter(|p| !p.is_empty())
        .filter(|p| !points_into_store(Path::new(root).join(p)))
        .map(str::to_string)
        .collect())
}

/// Resolve whatever the user typed into a full commit sha: `fetchGit` accepts
/// nothing shorter, and this is where a typo'd ref gets a readable error.
fn resolve_rev(root: &str, git_ref: &str) -> Result<String> {
    let out = std::process::Command::new(git_bin())
        .args(["-C", root, "rev-parse", "--verify", "--quiet", &format!("{git_ref}^{{commit}}")])
        .output()
        .context("running git rev-parse")?;
    if !out.status.success() {
        bail!("no such revision in {root}: {git_ref}");
    }
    let rev = String::from_utf8(out.stdout).context("git rev-parse emitted non-UTF-8")?;
    let rev = rev.trim().to_string();
    if rev.len() != 40 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("git rev-parse returned something that is not a commit sha: {rev}");
    }
    Ok(rev)
}

fn points_into_store(path: PathBuf) -> bool {
    std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
        && std::fs::read_link(&path).is_ok_and(|t| t.starts_with("/nix/store"))
}

/// The snapshot's NAR hash, which locks it for pure eval. There is no
/// `builtins.hashPath`, so this is the one place we ask the `nix` CLI.
fn nar_hash(store_path: &str) -> Result<String> {
    let out = std::process::Command::new(nix_bin())
        .args(["hash", "path", "--type", "sha256", "--sri", store_path])
        .output()
        .context("running nix hash path")?;
    if !out.status.success() {
        bail!("nix hash path failed for {store_path}");
    }
    Ok(String::from_utf8(out.stdout)
        .context("nix hash path emitted non-UTF-8")?
        .trim()
        .to_string())
}

/// The one impure step: realise the source, pin it, and read the host system.
fn probe(es: &mut EvalState, root: &str, source: &Source) -> Result<(Locked, String)> {
    let v = es.eval_from_string(&probe_expr(root, source), root)?;
    let pin = es.require_attrs_select(&v, "pin")?;
    let s = es.require_attrs_select(&v, "s")?;
    let pin = es.require_string(&pin)?;
    let locked = match source {
        Source::WorkingTree(_) => {
            Locked::Snapshot { nar_hash: nar_hash(&pin)?, store_path: pin }
        }
        Source::Revision(rev) => {
            Locked::Revision { root: root.to_string(), rev: rev.clone(), nar_hash: pin }
        }
    };
    Ok((locked, es.require_string(&s)?))
}

/// Evaluate `body` with `self` bound to the locked source (pure).
fn eval_in_repo(es: &mut EvalState, root: &str, locked: &Locked, body: &str) -> Result<Value> {
    let self_v = es.eval_from_string(&self_expr(locked), root)?;
    let func = es.eval_from_string(&with_self(body), root)?;
    es.call(func, self_v)
}

/// Instantiate a derivation value: force it enough to write its `.drv` to the
/// store and return its (outPath, drvPath). Both are known at eval time, before
/// any building — so apis owns the output path and lets `nix build` realise it.
fn instantiate(es: &mut EvalState, v: &Value) -> Result<(String, String)> {
    if es.value_type(v)? != ValueType::AttrSet {
        bail!("value is not a derivation");
    }
    let out = es
        .require_attrs_select_opt(v, "outPath")?
        .context("value has no `outPath` — is it a derivation?")?;
    let drv = es
        .require_attrs_select_opt(v, "drvPath")?
        .context("value has no `drvPath` — is it a derivation?")?;
    Ok((es.require_string(&out)?, es.require_string(&drv)?))
}

/// Render a value for `eval` output.
fn render(es: &mut EvalState, v: &Value) -> Result<String> {
    Ok(match es.value_type(v)? {
        ValueType::String => es.require_string(v)?,
        ValueType::Int => es.require_int(v)?.to_string(),
        ValueType::Bool => es.require_bool(v)?.to_string(),
        ValueType::Null => "null".to_string(),
        ValueType::Function => "<lambda>".to_string(),
        ValueType::External => "<external>".to_string(),
        ValueType::Unknown => "<unknown>".to_string(),
        ValueType::AttrSet if is_derivation(es, v)? => {
            let out = es.require_attrs_select(v, "outPath")?;
            es.require_string(&out)?
        }
        // List, Path, Float, plain AttrSet → let Nix serialize it.
        _ => to_json(es, v)?,
    })
}

/// `meta.mainProgram`, the package's own declaration of which binary in `bin/`
/// is *the* program. This is what `nix run` consults, and it is the only
/// authority that survives a package having several executables.
fn main_program(es: &mut EvalState, v: &Value) -> Result<Option<String>> {
    let Some(meta) = es.require_attrs_select_opt(v, "meta")? else {
        return Ok(None);
    };
    let Some(main) = es.require_attrs_select_opt(&meta, "mainProgram")? else {
        return Ok(None);
    };
    Ok(Some(es.require_string(&main)?))
}

fn is_derivation(es: &mut EvalState, v: &Value) -> Result<bool> {
    match es.require_attrs_select_opt(v, "type")? {
        Some(t) if es.value_type(&t)? == ValueType::String => {
            Ok(es.require_string(&t)? == "derivation")
        }
        _ => Ok(false),
    }
}

fn to_json(es: &mut EvalState, v: &Value) -> Result<String> {
    let f = es.eval_from_string("builtins.toJSON", ".")?;
    let j = es.call(f, v.clone())?;
    es.require_string(&j)
}

// ---------------------------------------------------------------------------
// Effectful layer: filesystem + process.
// ---------------------------------------------------------------------------

/// Walk up from cwd to the nearest directory containing `default.nix`.
fn find_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join("default.nix").is_file() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!("no default.nix found in any parent directory");
        }
    }
}

/// Point `./result` at a built store path, like `nix build`.
fn link_result(path: &str) -> Result<()> {
    let link = Path::new("result");
    if link.is_symlink() || link.exists() {
        std::fs::remove_file(link).context("removing existing ./result")?;
    }
    std::os::unix::fs::symlink(path, link).context("creating ./result symlink")
}

/// List the files in a built output's `bin/`.
fn read_bins(out: &str) -> Result<Vec<PathBuf>> {
    let bin = Path::new(out).join("bin");
    std::fs::read_dir(&bin)
        .with_context(|| format!("no bin/ in {out}"))?
        .map(|e| Ok(e?.path()))
        .collect()
}

fn exec(exe: &Path, args: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let err = std::process::Command::new(exe).args(args).exec();
    Err(err).with_context(|| format!("exec {}", exe.display()))
}

/// Absolute paths to the tools apis shells out to, baked in at build time (no
/// PATH wrapper needed). Falls back to a PATH lookup for plain `cargo` builds.
fn nix_bin() -> &'static str {
    option_env!("APIS_NIX").unwrap_or("nix")
}
fn nom_bin() -> &'static str {
    option_env!("APIS_NOM").unwrap_or("nom")
}
fn git_bin() -> &'static str {
    option_env!("APIS_GIT").unwrap_or("git")
}

/// Realise a derivation, streaming `nix build`'s internal-json log through
/// `nom` (nix-output-monitor) for a live build tree. apis already knows the
/// output path, so nom doesn't need to report anything back.
fn realise(drv_path: &str) -> Result<()> {
    use std::process::Stdio;
    let mut nix = std::process::Command::new(nix_bin())
        .args([
            "build",
            &format!("{drv_path}^*"),
            "--no-link",
            "--extra-experimental-features",
            "nix-command",
            "--log-format",
            "internal-json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped()) // nix emits the json log on stderr
        .spawn()
        .context("spawning nix build")?;
    let logs = nix.stderr.take().expect("stderr was piped");
    let mut nom = std::process::Command::new(nom_bin())
        .arg("--json")
        .stdin(Stdio::from(logs))
        .spawn()
        .context("spawning nom")?;
    let status = nix.wait().context("waiting for nix build")?;
    let _ = nom.wait();
    if !status.success() {
        bail!("build failed");
    }
    Ok(())
}

fn exec_repl(expr: &str) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    let err = std::process::Command::new(nix_bin())
        .args(["repl", "--extra-experimental-features", "nix-command", "--pure-eval", "--expr", expr])
        .exec();
    Err(err).context("exec nix repl")
}

// ---------------------------------------------------------------------------
// main: the only orchestrator of side effects.
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    init().context("initializing the Nix library")?;
    let _guard = gc_register_my_thread()?;

    let root = find_root().context("locating repo root (looking for default.nix)")?;
    let root = root.to_str().context("repo path is not valid UTF-8")?;

    // The one impure step. `--ref` swaps the working tree for a committed tree;
    // everything downstream sees the same locked `self` either way.
    let source = match &cli.git_ref {
        Some(r) => Source::Revision(resolve_rev(root, r)?),
        None => Source::WorkingTree(git_files(root)?),
    };
    let mut es = open_eval()?;
    let (locked, system) = probe(&mut es, root, &source).context("snapshotting the repo")?;

    // Everything below is pure: `self` is locked to the snapshot and the system
    // is injected. pure-eval is an eval setting (not in the libstore settings
    // registry reachable via the C API settings calls), so we pass it through
    // NIX_CONFIG, which a freshly built EvalState loads from the environment.
    std::env::set_var("NIX_CONFIG", "pure-eval = true");
    let mut es = open_eval()?;

    match cli.cmd {
        Cmd::Eval(t) => {
            let body = body_expr(t.attr.as_deref(), t.expr.as_deref(), &system);
            let v = eval_in_repo(&mut es, root, &locked, &body)?;
            println!("{}", render(&mut es, &v)?);
        }
        Cmd::Instantiate(t) => {
            let body = body_expr(t.attr.as_deref(), t.expr.as_deref(), &system);
            let v = eval_in_repo(&mut es, root, &locked, &body)?;
            let (_, drv) = instantiate(&mut es, &v)?;
            println!("{drv}");
        }
        Cmd::Build(t) => {
            let body = body_expr(t.attr.as_deref(), t.expr.as_deref(), &system);
            let v = eval_in_repo(&mut es, root, &locked, &body)?;
            let (out, drv) = instantiate(&mut es, &v)?;
            realise(&drv)?;
            link_result(&out)?;
            println!("{out}");
        }
        Cmd::Run { target, args } => {
            let body = body_expr(target.attr.as_deref(), target.expr.as_deref(), &system);
            let v = eval_in_repo(&mut es, root, &locked, &body)?;
            let (out, drv) = instantiate(&mut es, &v)?;
            // The package's own mainProgram wins; the attrpath is only a guess.
            let declared = main_program(&mut es, &v)?;
            let name = declared
                .as_deref()
                .or_else(|| target.attr.as_deref().map(last_component));
            realise(&drv)?;
            let bins = read_bins(&out)?;
            let exe = pick_bin(&bins, name)?;
            exec(&exe, &args)?;
        }
        Cmd::Repl => exec_repl(&repl_expr(&locked, &system))?,
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests for the pure core.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_paths_includes_every_ancestor_directory() {
        let files = vec!["pkgs/apis/src/main.rs".to_string(), "README.md".to_string()];
        let allowed = allowed_paths(&files);
        for want in ["pkgs", "pkgs/apis", "pkgs/apis/src", "pkgs/apis/src/main.rs", "README.md"] {
            assert!(allowed.contains(want), "missing {want}");
        }
        assert!(!allowed.contains("pkgs/other"));
    }

    #[test]
    fn escaping_closes_the_string_and_interpolation_holes() {
        assert_eq!(escape_nix_string(r#"a"b"#), r#"a\"b"#);
        assert_eq!(escape_nix_string(r"a\b"), r"a\\b");
        assert_eq!(escape_nix_string("a${b}"), "a\\${b}");
    }

    #[test]
    fn probe_expr_for_a_revision_fetches_it_instead_of_the_working_tree() {
        let expr = probe_expr("/repo", &Source::Revision("a".repeat(40)));
        assert!(expr.contains(r#"builtins.fetchGit { url = "/repo""#));
        assert!(expr.contains("allRefs = true"));
        assert!(expr.contains("builtins.currentSystem"));
        // fetchGit hands us the narHash, so there is nothing left to hash.
        assert!(expr.contains(".narHash"));
        // No working-tree filter: the commit's tree is already the file set.
        assert!(!expr.contains("builtins.path"));
    }

    #[test]
    fn locked_self_is_a_hash_pinned_expression_for_either_source() {
        let snap = Locked::Snapshot {
            store_path: "/nix/store/x-source".into(),
            nar_hash: "sha256-aaa".into(),
        };
        assert!(self_expr(&snap).contains(r#"path = "/nix/store/x-source"; sha256 = "sha256-aaa""#));

        // A revision stays a fetchGit call: its store path may not exist yet.
        let rev = Locked::Revision {
            root: "/repo".into(),
            rev: "a".repeat(40),
            nar_hash: "sha256-bbb".into(),
        };
        let e = self_expr(&rev);
        assert!(e.starts_with("builtins.fetchGit"));
        assert!(e.contains(r#"narHash = "sha256-bbb""#));
    }

    #[test]
    fn probe_expr_admits_listed_files_and_reads_the_host_system() {
        let expr = probe_expr("/repo", &Source::WorkingTree(vec!["a/b.nix".to_string()]));
        assert!(expr.contains(r#""a" "a/b.nix""#));
        assert!(expr.contains("builtins.currentSystem"));
        assert!(expr.contains("name = \"source\""));
    }

    #[test]
    fn body_attr_selects_from_imported_repo_with_the_host_system() {
        let body = body_expr(Some("pkgs.foo"), None, "x86_64-linux");
        assert!(body.contains("intersectAttrs"));
        assert!(body.contains("{ system = \"x86_64-linux\"; }"));
        assert!(body.ends_with(").pkgs.foo"));
    }

    #[test]
    fn body_expr_is_verbatim() {
        assert_eq!(body_expr(None, Some("1 + 1"), "x86_64-linux"), "(1 + 1)");
    }

    #[test]
    fn last_component_of_attrpath() {
        assert_eq!(last_component("a.b.c"), "c");
        assert_eq!(last_component("solo"), "solo");
    }

    #[test]
    fn pick_bin_prefers_name_then_sole_entry() {
        let entries = [PathBuf::from("/o/bin/a"), PathBuf::from("/o/bin/b")];
        assert_eq!(pick_bin(&entries, Some("b")).unwrap(), PathBuf::from("/o/bin/b"));
        assert!(pick_bin(&entries, None).is_err());
        assert_eq!(pick_bin(&entries[..1], None).unwrap(), PathBuf::from("/o/bin/a"));
    }

    #[test]
    fn pick_bin_resolves_a_multi_binary_output_by_declared_name() {
        // slippi-dolphin: bin/ has dolphin-emu and dolphin-emu-nogui, and the
        // attrpath matches neither — only meta.mainProgram picks the winner.
        let entries = [
            PathBuf::from("/o/bin/dolphin-emu"),
            PathBuf::from("/o/bin/dolphin-emu-nogui"),
        ];
        assert!(pick_bin(&entries, Some("slippi-dolphin")).is_err());
        assert_eq!(
            pick_bin(&entries, Some("dolphin-emu")).unwrap(),
            PathBuf::from("/o/bin/dolphin-emu")
        );
    }
}
