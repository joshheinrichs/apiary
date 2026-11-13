use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{IsTerminal, Stderr, stderr};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Terminal, TerminalOptions, Viewport};
use systemd::journal::{self, JournalRecord};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const TICK: Duration = Duration::from_millis(80);
const LOG_GRACE: Duration = Duration::from_millis(250);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// Which unit files changed. What to start is left to systemd's own state: see `restarts` and
/// `wanted`.
pub struct Plan {
    pub removed: Vec<String>,
    pub changed: Vec<String>,
    pub added: Vec<String>,
}

impl Plan {
    fn stops(&self) -> Vec<String> {
        let mut stops: Vec<String> = self.removed.iter().chain(&self.changed).cloned().collect();
        stops.sort();
        stops
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Verb {
    Unlink,
    Link,
    Stop,
    Reload,
    Start,
}

impl Verb {
    fn word(self) -> &'static str {
        match self {
            Verb::Unlink => "unlink",
            Verb::Link => "link",
            Verb::Stop => "stop",
            Verb::Reload => "reload",
            Verb::Start => "start",
        }
    }
}

type Key = (Verb, String);

/// `ended` carries systemd's job result; "done" is success.
#[derive(Clone, Debug, Default)]
struct Job {
    began: Option<Instant>,
    ended: Option<(Instant, String)>,
    count: (usize, usize),
}

#[derive(Clone, Debug, Default)]
struct Facts {
    load: String,
    active: String,
    sub: String,
    after: Vec<String>,
    wanted_by: Vec<String>,
}

/// Facts missing from `facts` are stale and get fetched for every unit on the board.
#[derive(Clone, Debug, Default)]
struct Board {
    planned: BTreeSet<Key>,
    nodes: BTreeMap<Key, Job>,
    facts: BTreeMap<String, Facts>,
    notes: BTreeMap<String, String>,
}

#[derive(Debug)]
enum JobEvent {
    New {
        unit: String,
        at: Instant,
    },
    Removed {
        unit: String,
        result: String,
        at: Instant,
    },
}

struct LogLine {
    unit: String,
    own: bool,
    priority: u8,
    message: String,
}

enum Event {
    Job(JobEvent),
    Log(LogLine),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum State {
    Pending,
    Running,
    Done,
    Failed,
}

fn planned(plan: &Plan) -> Board {
    let ours =
        [(Verb::Unlink, ""), (Verb::Link, ""), (Verb::Reload, "")].map(|(v, s)| (v, s.to_owned()));
    let stops = plan.stops().into_iter().map(|u| (Verb::Stop, u));
    with_planned(Board::default(), ours.into_iter().chain(stops))
}

fn with_planned(board: Board, keys: impl IntoIterator<Item = Key>) -> Board {
    keys.into_iter().fold(board, |board, key| {
        let mut planned = board.planned;
        planned.insert(key.clone());
        update(Board { planned, ..board }, key, |job| job)
    })
}

/// Changed units come back only if systemd had them up before we stopped them.
fn restarts(plan: &Plan, facts: &BTreeMap<String, Facts>) -> Vec<String> {
    let up = |u: &String| {
        facts
            .get(u)
            .is_some_and(|f| matches!(f.active.as_str(), "active" | "activating" | "reloading"))
    };
    plan.changed.iter().filter(|u| up(u)).cloned().collect()
}

/// Added units start when systemd says something active wants them.
fn wanted(plan: &Plan, facts: &BTreeMap<String, Facts>) -> Vec<String> {
    let active = |u: &String| facts.get(u).is_some_and(|f| f.active == "active");
    plan.added
        .iter()
        .filter(|u| {
            facts
                .get(*u)
                .is_some_and(|f| f.wanted_by.iter().any(active))
        })
        .cloned()
        .collect()
}

fn update(board: Board, key: Key, f: impl FnOnce(Job) -> Job) -> Board {
    let mut nodes = board.nodes;
    let job = nodes.remove(&key).unwrap_or_default();
    nodes.insert(key, f(job));
    Board { nodes, ..board }
}

fn stale(board: Board, unit: &str) -> Board {
    let mut facts = board.facts;
    facts.remove(unit);
    Board { facts, ..board }
}

fn on_job(board: Board, phase: Verb, event: JobEvent) -> Board {
    match event {
        JobEvent::New { unit, at } => {
            let board = stale(board, &unit);
            update(board, (phase, unit), |job| Job {
                began: job.began.or(Some(at)),
                ..job
            })
        }
        JobEvent::Removed { unit, result, at } => {
            if !board.nodes.contains_key(&(phase, unit.clone())) {
                return board;
            }
            let board = stale(board, &unit);
            update(board, (phase, unit), |job| Job {
                ended: Some((at, result)),
                ..job
            })
        }
    }
}

fn state(board: &Board, key: &Key) -> State {
    let job = board.nodes.get(key).cloned().unwrap_or_default();
    let crashed = key.0 == Verb::Start
        && board
            .facts
            .get(&key.1)
            .is_some_and(|f| f.active == "failed");
    match (&job.began, &job.ended) {
        (_, Some((_, result))) if result == "done" && !crashed => State::Done,
        (_, Some(_)) => State::Failed,
        (Some(_), None) => State::Running,
        (None, None) => State::Pending,
    }
}

/// What a node waits on, top to bottom in display order.
fn deps(board: &Board, key: &Key) -> Vec<Key> {
    let of_verb = |verb: Verb| -> Vec<Key> {
        board
            .planned
            .iter()
            .filter(|k| k.0 == verb)
            .cloned()
            .collect()
    };
    match key.0 {
        Verb::Link => vec![(Verb::Unlink, String::new())],
        Verb::Reload => [(Verb::Link, String::new())]
            .into_iter()
            .chain(of_verb(Verb::Stop))
            .collect(),
        Verb::Start => board
            .facts
            .get(&key.1)
            .map(|f| f.after.iter())
            .into_iter()
            .flatten()
            .map(|u| (Verb::Start, u.clone()))
            .filter(|k| board.nodes.contains_key(k) && !board.planned.contains(k))
            .collect(),
        Verb::Unlink | Verb::Stop => vec![],
    }
}

fn goal_deps(board: &Board) -> Vec<Key> {
    let starts = board.planned.iter().filter(|k| k.0 == Verb::Start).cloned();
    [(Verb::Reload, String::new())]
        .into_iter()
        .chain(starts)
        .collect()
}

enum Body {
    Node(Key),
    Note(String),
}

struct Row {
    prefix: String,
    body: Body,
}

fn note(board: &Board, key: &Key) -> Option<String> {
    if state(board, key) != State::Failed {
        return None;
    }
    match key.0 {
        Verb::Unlink | Verb::Link | Verb::Reload => {
            board.nodes[key].ended.as_ref().map(|(_, r)| r.clone())
        }
        Verb::Stop | Verb::Start => board.notes.get(&key.1).cloned(),
    }
}

/// Drawn bottom-up like nix-output-monitor: built top-down from the goal, then flipped, so each
/// node sits below what it waits on and the goal is the last row.
fn tree(board: &Board) -> Vec<Row> {
    fn walk(
        board: &Board,
        key: &Key,
        indent: &str,
        last: bool,
        seen: &mut BTreeSet<Key>,
        rows: &mut Vec<Row>,
    ) {
        if !seen.insert(key.clone()) {
            return;
        }
        if let Some(text) = note(board, key) {
            rows.push(Row {
                prefix: format!("{indent}│    "),
                body: Body::Note(text),
            });
        }
        let branch = if last { "└─ " } else { "├─ " };
        rows.push(Row {
            prefix: format!("{indent}{branch}"),
            body: Body::Node(key.clone()),
        });
        let child_indent = format!("{indent}{}", if last { "   " } else { "│  " });
        let kids: Vec<Key> = deps(board, key).into_iter().rev().collect();
        for (i, kid) in kids.iter().enumerate() {
            walk(board, kid, &child_indent, i + 1 == kids.len(), seen, rows);
        }
    }
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();
    let top: Vec<Key> = goal_deps(board).into_iter().rev().collect();
    for (i, key) in top.iter().enumerate() {
        walk(board, key, "", i + 1 == top.len(), &mut seen, &mut rows);
    }
    rows.reverse();
    rows.into_iter()
        .map(|r| Row {
            prefix: r.prefix.replace('└', "┌"),
            ..r
        })
        .collect()
}

fn shown(rows: &[Row]) -> Vec<&Key> {
    rows.iter()
        .filter_map(|r| match &r.body {
            Body::Node(k) => Some(k),
            Body::Note(_) => None,
        })
        .collect()
}

fn busy(board: &Board) -> bool {
    shown(&tree(board))
        .into_iter()
        .any(|k| state(board, k) == State::Running)
}

fn detail(board: &Board, key: &Key) -> String {
    let job = &board.nodes[key];
    let (done, total) = job.count;
    let facts = board.facts.get(&key.1);
    match (key.0, &job.ended) {
        (Verb::Unlink, None) => format!("{done}/{total} checked"),
        (Verb::Unlink, Some(_)) => format!("{done} old links"),
        (Verb::Link, None) => format!("{done}/{total} files"),
        (Verb::Link, Some(_)) => format!("{done} files"),
        (Verb::Reload, _) => String::new(),
        _ if job.began.is_none() => String::new(),
        (verb, ended) => {
            let unit = facts.map(unit_state).unwrap_or_default();
            let result = ended.as_ref().map(|(_, r)| r.as_str());
            let show_unit =
                ended.is_none() || verb == Verb::Start || facts.is_some_and(|f| f.load != "loaded");
            [result, show_unit.then_some(unit.as_str())]
                .into_iter()
                .flatten()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        }
    }
}

/// The unit as systemd reports it: its LoadState when not loaded, else ActiveState (SubState).
fn unit_state(f: &Facts) -> String {
    match f.load.as_str() {
        "loaded" => format!("{} ({})", f.active, f.sub),
        load => load.to_owned(),
    }
}

fn elapsed(job: &Job, now: Instant) -> Option<Duration> {
    let began = job.began?;
    Some(job.ended.as_ref().map(|(t, _)| *t).unwrap_or(now) - began)
}

/// Whole seconds, then minutes, like nix-output-monitor.
fn human(d: Duration) -> String {
    match d.as_secs() {
        s if s < 60 => format!("{s}s"),
        s => format!("{}m{}s", s / 60, s % 60),
    }
}

/// The whole frame as lines: header, tree, goal, totals. `finished_at` is the wall-clock time
/// once the run is over.
fn frame(
    board: &Board,
    goal: &str,
    started: Instant,
    now: Instant,
    tick: usize,
    finished_at: Option<&str>,
) -> Vec<Line<'static>> {
    let edge = Style::new().bold();
    let wire = Style::new().fg(Color::Blue);
    let dim = Style::new().fg(Color::DarkGray);
    let rows = tree(board);

    let body = rows.iter().map(|r| {
        let key = match &r.body {
            Body::Note(text) => {
                return Line::from(vec![
                    Span::styled("┃ ", edge),
                    Span::styled(r.prefix.clone(), wire),
                    Span::styled(format!("└ {text}"), Style::new().fg(Color::Red)),
                ]);
            }
            Body::Node(key) => key,
        };
        let state = state(board, key);
        let (glyph, color) = match state {
            State::Pending => ("⏸", Color::Blue),
            State::Running => (SPINNER[tick % SPINNER.len()], Color::Yellow),
            State::Done => ("✔", Color::Green),
            State::Failed => ("✗", Color::Red),
        };
        let took = elapsed(&board.nodes[key], now)
            .filter(|d| state == State::Running || d.as_secs() >= 1)
            .map(|d| format!(" ⏱ {}", human(d)))
            .unwrap_or_default();
        let detail = match detail(board, key) {
            d if d.is_empty() => d,
            d => format!("  {d}"),
        };
        let name = [key.0.word(), key.1.as_str()].join(" ");
        Line::from(vec![
            Span::styled("┃ ", edge),
            Span::styled(r.prefix.clone(), wire),
            Span::styled(
                format!("{glyph} {}", name.trim_end()),
                Style::new().fg(color),
            ),
            Span::styled(took, dim),
            Span::styled(detail, dim),
        ])
    });

    let keys = shown(&rows);
    let count = |s: State| keys.iter().filter(|k| state(board, k) == s).count();
    let sep = || Span::raw(" │ ");
    let status = match finished_at {
        Some(at) => Span::styled(
            format!("Finished at {at} after {}", human(now - started)),
            Style::new().fg(Color::Green).bold(),
        ),
        None => Span::styled(format!("⏱ {}", human(now - started)), dim),
    };
    let footer = Line::from(vec![
        Span::styled("┗━ ", edge),
        Span::raw("∑ "),
        Span::styled(
            format!("⏵ {}", count(State::Running)),
            Style::new().fg(Color::Yellow),
        ),
        sep(),
        Span::styled(
            format!("✔ {}", count(State::Done)),
            Style::new().fg(Color::Green),
        ),
        sep(),
        Span::styled(
            format!("✗ {}", count(State::Failed)),
            Style::new().fg(Color::Red),
        ),
        sep(),
        Span::styled(
            format!("⏸ {}", count(State::Pending)),
            Style::new().fg(Color::Blue),
        ),
        sep(),
        status,
    ]);

    [Line::from(vec![
        Span::styled("┏━ ", edge),
        Span::styled("Home:", edge),
    ])]
    .into_iter()
    .chain(body)
    .chain([
        Line::from(vec![
            Span::styled("┣━ ", edge),
            Span::styled(goal.to_owned(), edge),
        ]),
        footer,
    ])
    .collect()
}

fn failed_starts(board: &Board) -> Vec<String> {
    board
        .planned
        .iter()
        .filter(|k| k.0 == Verb::Start && state(board, k) == State::Failed)
        .map(|k| k.1.clone())
        .collect()
}

fn log_line(log: &LogLine) -> Line<'static> {
    let color = match log.priority {
        0..=3 => Color::Red,
        4 => Color::Yellow,
        _ => Color::Reset,
    };
    Line::from(vec![
        Span::styled(format!("{}: ", log.unit), Style::new().fg(Color::DarkGray)),
        Span::styled(log.message.clone(), Style::new().fg(color)),
    ])
}

/// `USER_UNIT` is the manager talking about a unit; `_SYSTEMD_USER_UNIT` is the unit's own output.
fn parse_entry(entry: &JournalRecord) -> Option<LogLine> {
    let manager = entry.get("USER_UNIT");
    let unit = manager.or(entry.get("_SYSTEMD_USER_UNIT"))?;
    Some(LogLine {
        unit: unit.clone(),
        own: manager.is_none(),
        priority: entry
            .get("PRIORITY")
            .and_then(|p| p.parse().ok())
            .unwrap_or(6),
        message: entry.get("MESSAGE")?.trim_end().to_owned(),
    })
}

fn parse_facts(props: &HashMap<String, OwnedValue>) -> Facts {
    let string = |k: &str| props.get(k).and_then(|v| String::try_from(v.clone()).ok());
    let strings = |k: &str| {
        props
            .get(k)
            .and_then(|v| Vec::<String>::try_from(v.clone()).ok())
    };
    Facts {
        load: string("LoadState").unwrap_or_default(),
        active: string("ActiveState").unwrap_or_default(),
        sub: string("SubState").unwrap_or_default(),
        after: strings("After").unwrap_or_default(),
        wanted_by: ["WantedBy", "RequiredBy", "UpheldBy"]
            .into_iter()
            .flat_map(|k| strings(k).unwrap_or_default())
            .collect(),
    }
}

fn manager(conn: &Connection) -> Result<Proxy<'static>> {
    Proxy::new(conn, SYSTEMD, MANAGER_PATH, MANAGER).context("systemd manager proxy")
}

fn unit_facts(conn: &Connection, unit: &str) -> Result<Facts> {
    let path: OwnedObjectPath = manager(conn)?.call("LoadUnit", &(unit,))?;
    let props: HashMap<String, OwnedValue> = conn
        .call_method(
            Some(SYSTEMD),
            &path,
            Some("org.freedesktop.DBus.Properties"),
            "GetAll",
            &("org.freedesktop.systemd1.Unit",),
        )?
        .body()
        .deserialize()?;
    Ok(parse_facts(&props))
}

/// Subscribes before returning, so every job queued afterwards is seen.
fn watch_jobs(conn: &Connection, tx: Sender<Event>) -> Result<()> {
    let manager = manager(conn)?;
    let signals = manager.receive_all_signals()?;
    manager
        .call_method("Subscribe", &())
        .context("subscribing to systemd")?;
    thread::spawn(move || {
        for message in signals {
            let at = Instant::now();
            let body = message.body();
            let event = match message.header().member().map(|m| m.as_str()) {
                Some("JobNew") => body
                    .deserialize::<(u32, OwnedObjectPath, String)>()
                    .map(|(_, _, unit)| JobEvent::New { unit, at }),
                Some("JobRemoved") => body
                    .deserialize::<(u32, OwnedObjectPath, String, String)>()
                    .map(|(_, _, unit, result)| JobEvent::Removed { unit, result, at }),
                _ => continue,
            };
            let Ok(event) = event else { continue };
            if tx.send(Event::Job(event)).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// sd_journal handles are not `Send`, so the journal is opened on the thread that reads it.
fn follow_journal(since: SystemTime, tx: Sender<Event>) -> Result<()> {
    let since = since.duration_since(UNIX_EPOCH)?.as_micros() as u64;
    let (opened_tx, opened) = mpsc::channel();
    thread::spawn(move || {
        let journal = journal::OpenOptions::default()
            .current_user(true)
            .local_only(true)
            .open()
            .and_then(|mut j| j.seek_realtime_usec(since).map(|_| j));
        let mut journal = match journal {
            Ok(j) => j,
            Err(e) => return drop(opened_tx.send(Err(e))),
        };
        let _ = opened_tx.send(Ok(()));
        loop {
            while let Ok(Some(entry)) = journal.next_entry() {
                let Some(line) = parse_entry(&entry) else {
                    continue;
                };
                if tx.send(Event::Log(line)).is_err() {
                    return;
                }
            }
            if journal.wait(Some(Duration::from_millis(200))).is_err() {
                return;
            }
        }
    });
    opened.recv()?.context("opening the user journal")
}

enum Ui {
    Inline(Terminal<CrosstermBackend<Stderr>>),
    Plain,
}

impl Ui {
    fn open(height: usize) -> Result<Self> {
        if !stderr().is_terminal() {
            return Ok(Ui::Plain);
        }
        let (_, rows) = ratatui::crossterm::terminal::size()?;
        let height = (height as u16).clamp(4, (rows * 2 / 3).max(4));
        let viewport = Viewport::Inline(height);
        let terminal = Terminal::with_options(
            CrosstermBackend::new(stderr()),
            TerminalOptions { viewport },
        )?;
        Ok(Ui::Inline(terminal))
    }

    fn print(&mut self, lines: Vec<Line<'static>>) -> Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let Ui::Inline(terminal) = self else {
            lines.iter().for_each(|l| eprintln!("{l}"));
            return Ok(());
        };
        let width = terminal.size()?.width.max(1) as usize;
        let height: usize = lines.iter().map(|l| l.width().div_ceil(width).max(1)).sum();
        terminal.insert_before(height as u16, |buf| {
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(buf.area, buf)
        })?;
        Ok(())
    }

    /// Too tall for the viewport: keep the header and the newest rows, drop the oldest.
    fn draw(&mut self, lines: Vec<Line<'static>>) -> Result<()> {
        let Ui::Inline(terminal) = self else {
            return Ok(());
        };
        terminal.draw(|f| {
            let room = f.area().height as usize;
            let lines = match lines.len() > room {
                false => lines,
                true => {
                    let cut = lines.len() - room + 2;
                    let more = Line::from(vec![
                        Span::styled("┃ ", Style::new().bold()),
                        Span::styled(
                            format!("⋮ {} more", cut - 1),
                            Style::new().fg(Color::DarkGray),
                        ),
                    ]);
                    lines[..1]
                        .iter()
                        .cloned()
                        .chain([more])
                        .chain(lines[cut..].iter().cloned())
                        .collect()
                }
            };
            f.render_widget(Paragraph::new(lines), f.area());
        })?;
        Ok(())
    }

    /// The final frame goes into scrollback like a log line and the viewport is emptied, so the
    /// prompt lands right under it instead of below the viewport's unused rows.
    fn close(&mut self, lines: Vec<Line<'static>>) -> Result<()> {
        self.print(lines)?;
        let Ui::Inline(terminal) = self else {
            return Ok(());
        };
        terminal.clear()?;
        let top = terminal.get_frame().area().top();
        terminal.set_cursor_position((0, top))?;
        terminal.show_cursor()?;
        Ok(())
    }
}

/// The effectful shell: owns the bus, the journal feed and the terminal, and folds what they
/// report into the board.
pub struct Session {
    board: Board,
    conn: Connection,
    events: Receiver<Event>,
    ui: Ui,
    phase: Verb,
    goal: String,
    started: Instant,
    drawn: Instant,
    tick: usize,
    restart: Vec<String>,
}

impl Session {
    pub fn open(goal: String, plan: &Plan) -> Result<Self> {
        let started = Instant::now();
        let conn = Connection::session().context("connecting to the session bus")?;
        let (tx, events) = mpsc::channel();
        watch_jobs(&conn, tx.clone())?;
        follow_journal(SystemTime::now(), tx)?;
        let ui = Ui::open(plan.removed.len() + 2 * plan.changed.len() + plan.added.len() + 8)?;
        let mut session = Session {
            board: planned(plan),
            conn,
            events,
            ui,
            phase: Verb::Stop,
            goal,
            started,
            drawn: started,
            tick: 0,
            restart: Vec::new(),
        };
        session.fetch(plan.changed.iter().cloned());
        session.restart = restarts(plan, &session.board.facts);
        session.board = with_planned(
            std::mem::take(&mut session.board),
            session.restart.iter().map(|u| (Verb::Start, u.clone())),
        );
        session.draw()?;
        Ok(session)
    }

    fn draw(&mut self) -> Result<()> {
        self.tick += 1;
        self.drawn = Instant::now();
        let lines = frame(
            &self.board,
            &self.goal,
            self.started,
            self.drawn,
            self.tick,
            None,
        );
        self.ui.draw(lines)
    }

    fn set(&mut self, key: Key, f: impl FnOnce(Job) -> Job) {
        self.board = update(std::mem::take(&mut self.board), key, f);
    }

    /// Runs one of our own steps as a node; `work` reports progress as a running count.
    pub fn step(
        &mut self,
        verb: Verb,
        total: usize,
        work: impl FnOnce(&mut dyn FnMut(usize)) -> Result<usize>,
    ) -> Result<usize> {
        let key = (verb, String::new());
        let now = Instant::now();
        self.set(key.clone(), |job| Job {
            began: Some(now),
            count: (0, total),
            ..job
        });
        let outcome = work(&mut |done| {
            self.set(key.clone(), |job| Job {
                count: (done, total),
                ..job
            });
            if self.drawn.elapsed() >= TICK {
                let _ = self.draw();
            }
        });
        let result = match &outcome {
            Ok(_) => "done".to_owned(),
            Err(e) => format!("{e:#}"),
        };
        let done = *outcome.as_ref().unwrap_or(&0);
        let now = Instant::now();
        self.set(key, |job| Job {
            ended: Some((now, result)),
            count: if outcome.is_ok() {
                (done, total)
            } else {
                job.count
            },
            ..job
        });
        self.draw()?;
        outcome
    }

    /// Folds in whatever arrived within `wait`, refreshes stale facts, prints logs, redraws.
    fn pump(&mut self, wait: Duration) -> Result<()> {
        let first = match self.events.recv_timeout(wait) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => anyhow::bail!("systemd event feed ended"),
        };
        let arrived: Vec<Event> = first.into_iter().chain(self.events.try_iter()).collect();
        let mut logs = Vec::new();
        for event in arrived {
            match event {
                Event::Job(job) => {
                    self.board = on_job(std::mem::take(&mut self.board), self.phase, job)
                }
                Event::Log(line) => logs.push(line),
            }
        }

        let units: BTreeSet<String> = self
            .board
            .nodes
            .keys()
            .map(|k| k.1.clone())
            .filter(|u| !u.is_empty())
            .collect();
        self.fetch(units.iter().cloned());

        let ours: Vec<LogLine> = logs
            .into_iter()
            .filter(|l| units.contains(&l.unit))
            .collect();
        for line in ours.iter().filter(|l| l.own) {
            self.board
                .notes
                .insert(line.unit.clone(), line.message.clone());
        }
        self.ui.print(ours.iter().map(log_line).collect())?;
        self.draw()
    }

    /// Fills in facts for whichever of `units` are missing them.
    fn fetch(&mut self, units: impl IntoIterator<Item = String>) {
        for unit in units {
            if self.board.facts.contains_key(&unit) {
                continue;
            }
            let facts = unit_facts(&self.conn, &unit).unwrap_or_default();
            self.board.facts.insert(unit, facts);
        }
    }

    fn queue(&mut self, verb: Verb, units: &[String]) -> Result<()> {
        self.phase = verb;
        let method = if verb == Verb::Start {
            "StartUnit"
        } else {
            "StopUnit"
        };
        let manager = manager(&self.conn)?;
        for unit in units {
            let now = Instant::now();
            let queued = manager.call::<_, _, OwnedObjectPath>(method, &(unit, "replace"));
            let ended = queued.err().map(|e| (now, e.to_string()));
            self.set((verb, unit.clone()), |job| Job {
                began: Some(now),
                ended,
                ..job
            });
        }
        while busy(&self.board) {
            self.pump(TICK)?;
        }
        Ok(())
    }

    /// Stop, reload, start. Returns the planned starts that did not come up.
    pub fn systemd(&mut self, plan: &Plan) -> Result<Vec<String>> {
        self.queue(Verb::Stop, &plan.stops())?;

        let key = (Verb::Reload, String::new());
        let now = Instant::now();
        self.set(key.clone(), |job| Job {
            began: Some(now),
            ..job
        });
        self.draw()?;
        let reloaded = manager(&self.conn)?.call::<_, _, ()>("Reload", &());
        let result = reloaded
            .as_ref()
            .map(|_| "done".to_owned())
            .unwrap_or_else(|e| e.to_string());
        let now = Instant::now();
        self.set(key, |job| Job {
            ended: Some((now, result)),
            ..job
        });
        self.board.facts.clear();
        reloaded.context("systemctl --user daemon-reload")?;

        self.fetch(plan.added.iter().cloned());
        let wanted_by: Vec<String> = plan
            .added
            .iter()
            .filter_map(|u| self.board.facts.get(u))
            .flat_map(|f| f.wanted_by.clone())
            .collect();
        self.fetch(wanted_by);
        let mut starts = [self.restart.clone(), wanted(plan, &self.board.facts)].concat();
        starts.sort();
        self.board = with_planned(
            std::mem::take(&mut self.board),
            starts.iter().map(|u| (Verb::Start, u.clone())),
        );
        self.queue(Verb::Start, &starts)?;
        thread::sleep(LOG_GRACE);
        self.board.facts.clear();
        self.pump(Duration::ZERO)?;
        Ok(failed_starts(&self.board))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let at = jiff::Zoned::now().strftime("%H:%M:%S").to_string();
        let lines = frame(
            &self.board,
            &self.goal,
            self.started,
            Instant::now(),
            self.tick,
            Some(&at),
        );
        let _ = self.ui.close(lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(active: &str, after: &[&str]) -> Facts {
        Facts {
            load: "loaded".into(),
            active: active.into(),
            sub: "running".into(),
            wanted_by: vec![],
            after: after.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn picture(board: &Board) -> Vec<String> {
        tree(board)
            .iter()
            .map(|r| match &r.body {
                Body::Node(k) => format!("{}{} {}", r.prefix, k.0.word(), k.1)
                    .trim_end()
                    .to_owned(),
                Body::Note(t) => format!("{}└ {t}", r.prefix),
            })
            .collect()
    }

    #[test]
    fn the_dag_reads_top_down_in_the_order_things_happen() {
        let t0 = Instant::now();
        let plan = Plan {
            removed: vec![],
            changed: vec!["a.service".into()],
            added: vec![],
        };
        let starts = ["a.service", "b.service"].map(|u| (Verb::Start, u.to_owned()));
        let board = with_planned(planned(&plan), starts);
        let board = on_job(
            board,
            Verb::Start,
            JobEvent::New {
                unit: "dep.service".into(),
                at: t0,
            },
        );
        let board = on_job(
            board,
            Verb::Start,
            JobEvent::New {
                unit: "unrelated.service".into(),
                at: t0,
            },
        );
        let mut board = on_job(
            board,
            Verb::Start,
            JobEvent::New {
                unit: "a.service".into(),
                at: t0,
            },
        );
        board.facts.insert(
            "a.service".into(),
            facts("activating", &["dep.service", "basic.target"]),
        );
        board
            .facts
            .insert("dep.service".into(), facts("activating", &[]));

        assert_eq!(
            picture(&board),
            [
                "      ┌─ unlink",
                "   ┌─ link",
                "   ├─ stop a.service",
                "┌─ reload",
                "│  ┌─ start dep.service",
                "├─ start a.service",
                "├─ start b.service",
            ]
        );
        assert!(busy(&board));

        let board = on_job(
            board,
            Verb::Start,
            JobEvent::Removed {
                unit: "dep.service".into(),
                result: "failed".into(),
                at: t0,
            },
        );
        let board = on_job(
            board,
            Verb::Start,
            JobEvent::Removed {
                unit: "a.service".into(),
                result: "dependency".into(),
                at: t0,
            },
        );
        assert!(
            !busy(&board),
            "unrelated.service is off the tree and must not hold the phase open"
        );
        assert_eq!(failed_starts(&board), ["a.service"]);
        let mut board = board;
        board
            .facts
            .insert("a.service".into(), facts("inactive", &[]));
        assert_eq!(
            detail(&board, &(Verb::Start, "a.service".into())),
            "dependency · inactive (running)"
        );
        board.facts.insert(
            "a.service".into(),
            Facts {
                load: "not-found".into(),
                ..facts("inactive", &[])
            },
        );
        assert_eq!(
            detail(&board, &(Verb::Start, "a.service".into())),
            "dependency · not-found"
        );
    }

    #[test]
    fn what_starts_is_what_systemd_says_was_up_or_is_wanted() {
        let plan = Plan {
            removed: vec!["gone.service".into()],
            changed: ["up.timer", "done.service", "booting.service"]
                .map(String::from)
                .to_vec(),
            added: ["wanted.service", "orphan.service", "dormant.service"]
                .map(String::from)
                .to_vec(),
        };
        let with_wanted_by = |active: &str, by: &[&str]| Facts {
            wanted_by: by.iter().map(|s| s.to_string()).collect(),
            ..facts(active, &[])
        };
        let facts: BTreeMap<String, Facts> = [
            ("up.timer", facts("active", &[])),
            ("done.service", facts("inactive", &[])),
            ("booting.service", facts("activating", &[])),
            (
                "wanted.service",
                with_wanted_by("inactive", &["default.target"]),
            ),
            ("orphan.service", with_wanted_by("inactive", &[])),
            (
                "dormant.service",
                with_wanted_by("inactive", &["sleepy.target"]),
            ),
            ("default.target", facts("active", &[])),
            ("sleepy.target", facts("inactive", &[])),
        ]
        .map(|(u, f)| (u.to_owned(), f))
        .into();
        assert_eq!(restarts(&plan, &facts), ["up.timer", "booting.service"]);
        assert_eq!(wanted(&plan, &facts), ["wanted.service"]);
        assert_eq!(
            plan.stops(),
            [
                "booting.service",
                "done.service",
                "gone.service",
                "up.timer"
            ]
        );
    }

    #[test]
    fn a_start_that_crashes_after_its_job_is_a_failure() {
        let t0 = Instant::now();
        let empty = Plan {
            removed: vec![],
            changed: vec![],
            added: vec![],
        };
        let board = with_planned(planned(&empty), [(Verb::Start, "a.service".to_owned())]);
        let board = on_job(
            board,
            Verb::Start,
            JobEvent::New {
                unit: "a.service".into(),
                at: t0,
            },
        );
        let mut board = on_job(
            board,
            Verb::Start,
            JobEvent::Removed {
                unit: "a.service".into(),
                result: "done".into(),
                at: t0,
            },
        );
        board.facts.insert("a.service".into(), facts("active", &[]));
        assert_eq!(
            state(&board, &(Verb::Start, "a.service".into())),
            State::Done
        );
        board.facts.insert("a.service".into(), facts("failed", &[]));
        assert_eq!(failed_starts(&board), ["a.service"]);
    }

    #[test]
    fn journal_entries_attribute_to_units() {
        let entry = |pairs: &[(&str, &str)]| -> JournalRecord {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let from_manager = entry(&[
            ("USER_UNIT", "a.service"),
            ("_SYSTEMD_USER_UNIT", "init.scope"),
            ("MESSAGE", "Started a."),
        ]);
        assert_eq!(parse_entry(&from_manager).unwrap().unit, "a.service");
        let own = parse_entry(&entry(&[
            ("_SYSTEMD_USER_UNIT", "a.service"),
            ("MESSAGE", "boom\n"),
            ("PRIORITY", "3"),
        ]))
        .unwrap();
        assert_eq!((own.priority, own.message.as_str()), (3, "boom"));
        assert!(parse_entry(&entry(&[("MESSAGE", "kernel")])).is_none());
    }
}
