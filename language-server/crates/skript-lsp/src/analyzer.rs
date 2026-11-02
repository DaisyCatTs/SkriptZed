//! "Check against real Skript" — an opt-in bridge to SkAnalyzer.
//!
//! Everything else in this server *approximates* Skript: it matches lines
//! against `docs.json`, which cannot see type checks, section rules, option
//! expansion or anything an addon decides at parse time. SkAnalyzer runs the
//! real parser (a patched "Skratched" Skript build) outside a Minecraft server,
//! so its errors are exactly the ones a server would print on `/sk reload`.
//!
//! It is a Java 25 program, LGPL, and installed by the user — never bundled —
//! so this module only drives a process it is pointed at. That process is an
//! interactive shell: one command per stdin line, answers as log lines on
//! stdout. Three properties of that shell shape the code below:
//!
//! * A request that fails logs one `ERROR [SkAnalyzer]` line and **never**
//!   emits a result, and nothing echoes which command an answer belongs to.
//!   So answers are matched by path, an `ERROR [SkAnalyzer]` line fails the
//!   request in flight, and every wait has a deadline.
//! * After a timeout the shell's state is unknown — a late answer could be
//!   mistaken for the reply to the *next* request. The process is killed
//!   instead, and the caller decides whether to start a new one.
//! * EOF on stdin makes the shell spin, so stdin stays open until shutdown.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{timeout, timeout_at, Instant};

/// The oldest Java SkAnalyzer runs on.
const MIN_JAVA: u32 = 25;
/// Boot is ~3 s on a warm machine; a cold JVM plus addons can take far longer.
const START_LIMIT: Duration = Duration::from_secs(60);
/// `java -version` should answer in well under a second.
const JAVA_PROBE_LIMIT: Duration = Duration::from_secs(15);
/// How long `exit` gets before the process is killed.
const EXIT_GRACE: Duration = Duration::from_secs(3);
/// Output lines kept for a crash report.
const TAIL_LINES: usize = 20;

/// The startup line after which the shell reads commands.
const READY_MARKER: &str = "Type 'help' for help.";
/// Printed (then the process exits 1) when `plugins/` has no Skratched build.
const BRIDGE_MISSING_MARKER: &str = "Required Skript bridge is missing";
const ANALYZER_TAG: &str = "[SkAnalyzer] ";

/// Where SkAnalyzer and its inputs live. All of it is user-supplied.
#[derive(Debug, Clone)]
pub struct AnalyzerConfig {
    /// A Java 25+ executable.
    pub java: PathBuf,
    /// The SkAnalyzer shell jar (`skanalyzer-shell-bundler-*.jar`).
    pub jar: PathBuf,
    /// A directory owned by the analyzer. It writes `plugins/Skript` and
    /// `libraries/` here, and loads every jar in `plugins/` — which is where
    /// the Skratched Skript build must be.
    pub work_dir: PathBuf,
    /// Extra addon jars, passed as `--add-plugin`.
    pub plugins: Vec<PathBuf>,
    /// Per-request limit.
    pub timeout: Duration,
}

/// One problem Skript reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 0-based. Skript reports lines only, never columns.
    pub line: u32,
    pub message: String,
    pub severity: FindingSeverity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalyzerError {
    /// `java` could not be run at all.
    JavaMissing,
    /// `java` runs but is older than 25; carries the version it reported.
    JavaTooOld(String),
    /// The SkAnalyzer jar does not exist.
    JarMissing,
    /// No Skratched Skript build in `<work_dir>/plugins`.
    SkriptBridgeMissing,
    /// The shell never said it was ready.
    StartTimeout,
    /// A request got no answer in time. The process has been killed.
    Timeout,
    /// The process is gone; carries its exit status and last output.
    Crashed(String),
    /// A path that cannot be sent to the shell.
    BadPath(String),
    /// The shell refused the request with an `ERROR [SkAnalyzer]` line.
    Rejected(String),
}

impl fmt::Display for AnalyzerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JavaMissing => write!(
                f,
                "could not run Java; point the analyzer's `java` setting at a Java {MIN_JAVA} \
                 or newer executable"
            ),
            Self::JavaTooOld(version) => write!(
                f,
                "SkAnalyzer needs Java {MIN_JAVA} or newer, but the configured Java is \
                 {version}; install a newer JDK and point the `java` setting at it"
            ),
            Self::JarMissing => write!(
                f,
                "the SkAnalyzer jar was not found; check the analyzer's `jar` setting"
            ),
            Self::SkriptBridgeMissing => write!(
                f,
                "no Skratched Skript build in the analyzer work directory's `plugins` folder; \
                 download one from https://github.com/SkEditorTeam/Skratches and put it there"
            ),
            Self::StartTimeout => write!(
                f,
                "SkAnalyzer did not finish starting within {} s",
                START_LIMIT.as_secs()
            ),
            Self::Timeout => write!(
                f,
                "SkAnalyzer did not answer in time and was stopped; raise the analyzer \
                 timeout if your scripts are large"
            ),
            Self::Crashed(detail) => write!(f, "SkAnalyzer stopped unexpectedly: {detail}"),
            Self::BadPath(detail) => write!(f, "cannot check this file with SkAnalyzer: {detail}"),
            Self::Rejected(message) => write!(f, "SkAnalyzer refused the request: {message}"),
        }
    }
}

impl std::error::Error for AnalyzerError {}

/// A running SkAnalyzer shell. Requests are serialised: one in flight at a time.
pub struct Analyzer {
    requests: mpsc::Sender<Request>,
    task: JoinHandle<()>,
}

struct Request {
    /// The path as sent to the shell: canonical, forward slashes.
    sent: String,
    /// The path as the caller gave it, so the reply is keyed the way they asked.
    asked: PathBuf,
    reply: oneshot::Sender<Result<HashMap<PathBuf, Vec<Finding>>, AnalyzerError>>,
}

impl Analyzer {
    /// Verifies Java and the jars, starts the shell and waits until it is ready.
    pub async fn start(config: AnalyzerConfig) -> Result<Self, AnalyzerError> {
        check_java(&config.java).await?;
        if !config.jar.is_file() {
            return Err(AnalyzerError::JarMissing);
        }
        if let Some(missing) = config.plugins.iter().find(|jar| !jar.is_file()) {
            return Err(AnalyzerError::BadPath(format!(
                "addon jar not found: {}",
                missing.display()
            )));
        }
        std::fs::create_dir_all(&config.work_dir).map_err(|error| {
            AnalyzerError::BadPath(format!("{}: {error}", config.work_dir.display()))
        })?;
        // The shell would report this itself, but only after booting a JVM.
        if config.plugins.is_empty() && !has_jar(&config.work_dir.join("plugins")) {
            return Err(AnalyzerError::SkriptBridgeMissing);
        }

        let mut command = Command::new(&config.java);
        // The shell exits by itself when this process dies, so an editor crash
        // does not leave a JVM behind.
        command
            .arg(format!("-Dskanalyzer.parentProcess={}", std::process::id()))
            .arg("-jar")
            .arg(&config.jar);
        for plugin in &config.plugins {
            command.arg("--add-plugin").arg(plugin);
        }
        command.current_dir(&config.work_dir);
        launch(command, START_LIMIT, config.timeout).await
    }

    /// Loads (or reloads) `path` and returns what Skript reported.
    ///
    /// `load` rather than `parse`, so the script stays loaded and functions it
    /// defines resolve when other files are checked. The requested file is
    /// keyed by `path` exactly as given; any other file in the answer is keyed
    /// by the absolute path SkAnalyzer reported.
    pub async fn check(
        &self,
        path: &Path,
    ) -> Result<HashMap<PathBuf, Vec<Finding>>, AnalyzerError> {
        let sent = command_path(path)?;
        let (reply, answer) = oneshot::channel();
        let request = Request {
            sent,
            asked: path.to_path_buf(),
            reply,
        };
        let gone = || AnalyzerError::Crashed("the analyzer task has stopped".to_owned());
        self.requests.send(request).await.map_err(|_| gone())?;
        answer.await.map_err(|_| gone())?
    }

    /// Asks the shell to exit, killing it if it does not.
    pub async fn shutdown(self) {
        drop(self.requests);
        // The task's own waits are bounded; this is a backstop against a bug.
        let mut task = self.task;
        if timeout(EXIT_GRACE * 2, &mut task).await.is_err() {
            task.abort();
        }
    }
}

/// Spawns `command`, waits for the ready line, then hands the process to a task.
async fn launch(
    mut command: Command,
    start_limit: Duration,
    request_limit: Duration,
) -> Result<Analyzer, AnalyzerError> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| AnalyzerError::Crashed(format!("could not start: {error}")))?;
    let stdin = child.stdin.take().expect("stdin is piped");
    let (line_tx, mut lines) = mpsc::unbounded_channel();
    // stderr carries the JVM's own warnings and stack traces; both streams feed
    // one channel, which closes only when the process has closed both.
    forward_lines(
        child.stdout.take().expect("stdout is piped"),
        line_tx.clone(),
    );
    forward_lines(child.stderr.take().expect("stderr is piped"), line_tx);

    let mut tail = Tail::default();
    let deadline = Instant::now() + start_limit;
    loop {
        match timeout_at(deadline, lines.recv()).await {
            Err(_) => {
                kill(&mut child).await;
                return Err(AnalyzerError::StartTimeout);
            }
            Ok(None) => return Err(crashed(&mut child, &tail).await),
            Ok(Some(line)) => {
                let plain = strip_ansi(&line);
                if plain.contains(BRIDGE_MISSING_MARKER) {
                    kill(&mut child).await;
                    return Err(AnalyzerError::SkriptBridgeMissing);
                }
                if plain.contains(READY_MARKER) {
                    break;
                }
                tail.push(plain);
            }
        }
    }

    let (requests, inbox) = mpsc::channel(16);
    let shell = Shell {
        child,
        stdin,
        lines,
        tail,
        request_limit,
    };
    let task = tokio::spawn(shell.run(inbox));
    Ok(Analyzer { requests, task })
}

/// The task that owns the process.
struct Shell {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::UnboundedReceiver<String>,
    tail: Tail,
    request_limit: Duration,
}

impl Shell {
    async fn run(mut self, mut inbox: mpsc::Receiver<Request>) {
        let dead = loop {
            tokio::select! {
                request = inbox.recv() => {
                    let Some(request) = request else {
                        self.exit().await;
                        return;
                    };
                    let result = self.serve(&request).await;
                    let fatal = match &result {
                        Err(AnalyzerError::Timeout) => Some("an earlier request timed out".to_owned()),
                        Err(AnalyzerError::Crashed(detail)) => Some(detail.clone()),
                        _ => None,
                    };
                    let _ = request.reply.send(result);
                    if let Some(reason) = fatal {
                        break reason;
                    }
                }
                line = self.lines.recv() => match line {
                    // Idle output: startup stragglers, answers to abandoned work.
                    Some(line) => self.tail.push(strip_ansi(&line)),
                    None => match crashed(&mut self.child, &self.tail).await {
                        AnalyzerError::Crashed(detail) => break detail,
                        other => break other.to_string(),
                    },
                },
            }
        };
        // Keep answering, so a caller never waits on a process that is gone.
        while let Some(request) = inbox.recv().await {
            let _ = request
                .reply
                .send(Err(AnalyzerError::Crashed(dead.clone())));
        }
    }

    async fn serve(
        &mut self,
        request: &Request,
    ) -> Result<HashMap<PathBuf, Vec<Finding>>, AnalyzerError> {
        let command = format!("load {}\n", request.sent);
        let written = async {
            self.stdin.write_all(command.as_bytes()).await?;
            self.stdin.flush().await
        };
        if let Err(error) = written.await {
            return Err(AnalyzerError::Crashed(format!(
                "could not send the request: {error}"
            )));
        }

        let wanted = path_key(&request.sent);
        let deadline = Instant::now() + self.request_limit;
        loop {
            let line = match timeout_at(deadline, self.lines.recv()).await {
                Err(_) => {
                    kill(&mut self.child).await;
                    return Err(AnalyzerError::Timeout);
                }
                Ok(None) => return Err(crashed(&mut self.child, &self.tail).await),
                Ok(Some(line)) => strip_ansi(&line),
            };
            if let Some(results) = parse_results(&line) {
                if results.iter().any(|(path, _)| path_key(path) == wanted) {
                    return Ok(key_results(results, &wanted, &request.asked));
                }
                continue;
            }
            if let Some(message) = analyzer_error(&line) {
                return Err(AnalyzerError::Rejected(message.to_owned()));
            }
            self.tail.push(line);
        }
    }

    async fn exit(&mut self) {
        let _ = self.stdin.write_all(b"exit\n").await;
        let _ = self.stdin.flush().await;
        if timeout(EXIT_GRACE, self.child.wait()).await.is_err() {
            kill(&mut self.child).await;
        }
    }
}

/// The last few lines of output, for crash reports.
#[derive(Default)]
struct Tail(std::collections::VecDeque<String>);

impl Tail {
    fn push(&mut self, line: String) {
        if line.trim().is_empty() {
            return;
        }
        if self.0.len() == TAIL_LINES {
            self.0.pop_front();
        }
        self.0.push_back(line);
    }

    fn render(&self) -> String {
        self.0
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn forward_lines<R: AsyncRead + Unpin + Send + 'static>(
    stream: R,
    lines: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buffer);
                    let line = line.trim_end_matches(['\n', '\r']).to_owned();
                    if lines.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// Builds the `Crashed` error once the output streams have closed.
async fn crashed(child: &mut Child, tail: &Tail) -> AnalyzerError {
    let status = match timeout(EXIT_GRACE, child.wait()).await {
        Ok(Ok(status)) => status.to_string(),
        Ok(Err(error)) => format!("unknown exit status ({error})"),
        Err(_) => {
            kill(child).await;
            "closed its output but did not exit, so it was killed".to_owned()
        }
    };
    let tail = tail.render();
    if tail.is_empty() {
        AnalyzerError::Crashed(status)
    } else {
        AnalyzerError::Crashed(format!("{status}; last output:\n{tail}"))
    }
}

async fn kill(child: &mut Child) {
    let _ = child.start_kill();
    let _ = timeout(EXIT_GRACE, child.wait()).await;
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    // CREATE_NO_WINDOW: an editor is a GUI process, and without this every
    // spawned JVM flashes a console window.
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

/// Runs `java -version` and insists on Java 25 or newer.
async fn check_java(java: &Path) -> Result<(), AnalyzerError> {
    let mut command = Command::new(java);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hide_console(&mut command);
    let output = match timeout(JAVA_PROBE_LIMIT, command.output()).await {
        Err(_) => return Err(AnalyzerError::StartTimeout),
        Ok(Err(_)) => return Err(AnalyzerError::JavaMissing),
        Ok(Ok(output)) => output,
    };
    // `-version` writes to stderr; some wrappers use stdout.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    match parse_java_version(&text) {
        Some((major, _)) if major >= MIN_JAVA => Ok(()),
        Some((_, version)) => Err(AnalyzerError::JavaTooOld(version)),
        None => Err(AnalyzerError::JavaTooOld(
            text.lines()
                .next()
                .unwrap_or("no version output")
                .trim()
                .to_owned(),
        )),
    }
}

/// Reads the major version from `java -version` output: `"25.0.4"`, `"25-ea"`,
/// or the pre-9 `"1.8.0_392"` scheme.
fn parse_java_version(text: &str) -> Option<(u32, String)> {
    let line = text.lines().find(|line| line.contains("version \""))?;
    let quoted = line.split('"').nth(1)?;
    let mut parts = quoted.split(|c: char| !c.is_ascii_digit());
    let first: u32 = parts.next()?.parse().ok()?;
    let major = if first == 1 {
        parts.next()?.parse().ok()?
    } else {
        first
    };
    Some((major, quoted.to_owned()))
}

fn has_jar(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
        })
    })
}

/// The path as the shell must receive it: canonical, absolute, forward slashes.
///
/// The shell splits its argument on spaces and rejoins with single spaces, so
/// a path with a run of spaces (or a leading/trailing one) would silently name
/// a different file. Those are refused rather than mangled.
fn command_path(path: &Path) -> Result<String, AnalyzerError> {
    let bad = |detail: String| AnalyzerError::BadPath(format!("{}: {detail}", path.display()));
    let canonical = std::fs::canonicalize(path).map_err(|error| bad(error.to_string()))?;
    if !canonical.is_file() {
        return Err(bad("not a file".to_owned()));
    }
    let text = canonical
        .to_str()
        .ok_or_else(|| bad("the path is not valid UTF-8".to_owned()))?;
    // Windows canonical paths are verbatim (`\\?\C:\…`); Java wants `C:/…`.
    let text = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!("//{rest}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(text).to_owned()
    };
    let text = text.replace('\\', "/");
    if text.contains("  ") || text.starts_with(' ') || text.ends_with(' ') {
        return Err(bad(
            "SkAnalyzer cannot address paths with consecutive, leading or trailing spaces"
                .to_owned(),
        ));
    }
    if text.contains(['\n', '\r']) {
        return Err(bad("the path contains a line break".to_owned()));
    }
    Ok(text)
}

/// A comparison key for paths as either side spells them. Windows file systems
/// are case-insensitive, and Java may report a drive letter in either case.
fn path_key(path: &str) -> String {
    let key = path.replace('\\', "/");
    let key = key.trim_end_matches('/');
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key.to_owned()
    }
}

fn key_results(
    results: Vec<(String, Vec<Finding>)>,
    wanted: &str,
    asked: &Path,
) -> HashMap<PathBuf, Vec<Finding>> {
    results
        .into_iter()
        .map(|(path, findings)| {
            let key = if path_key(&path) == wanted {
                asked.to_path_buf()
            } else {
                native_path(&path)
            };
            (key, findings)
        })
        .collect()
}

fn native_path(reported: &str) -> PathBuf {
    PathBuf::from(reported.replace('/', std::path::MAIN_SEPARATOR_STR))
}

/// Parses one line of shell output. `None` unless it is a results line.
///
/// Accepts the line raw — ANSI escapes and the `[time] INFO  [SkAnalyzer]`
/// prefix are stripped here. Keys are the reported absolute paths with native
/// separators; findings are sorted by line, because Skript reports them in
/// the order its loader happened to reach them.
#[cfg(test)]
pub fn parse_output_line(line: &str) -> Option<HashMap<PathBuf, Vec<Finding>>> {
    let results = parse_results(&strip_ansi(line))?;
    Some(
        results
            .into_iter()
            .map(|(path, findings)| (native_path(&path), findings))
            .collect(),
    )
}

#[derive(Deserialize)]
struct Output {
    results: HashMap<String, FileResult>,
}

#[derive(Deserialize)]
struct FileResult {
    #[serde(default)]
    errors: Option<Vec<RawError>>,
}

#[derive(Deserialize)]
struct RawError {
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    level: Option<String>,
}

/// [`parse_output_line`] on an already-stripped line, keeping the paths verbatim.
fn parse_results(plain: &str) -> Option<Vec<(String, Vec<Finding>)>> {
    let plain = plain.trim();
    let json = if plain.starts_with('{') {
        plain
    } else {
        let start = plain.find(ANALYZER_TAG)? + ANALYZER_TAG.len();
        &plain[start..]
    };
    if !json.starts_with('{') || !json.contains("\"results\"") {
        return None;
    }
    let output: Output = serde_json::from_str(json).ok()?;
    Some(
        output
            .results
            .into_iter()
            .map(|(path, result)| (path, findings(result.errors.unwrap_or_default())))
            .collect(),
    )
}

fn findings(errors: Vec<RawError>) -> Vec<Finding> {
    let mut out: Vec<Finding> = errors
        .into_iter()
        .map(|error| Finding {
            // Skript counts from 1; a missing or zero line is file-level.
            line: error.line.unwrap_or(0).saturating_sub(1),
            message: error.message.unwrap_or_default(),
            severity: severity(error.level.as_deref()),
        })
        .collect();
    out.sort_by_key(|finding| finding.line);
    out
}

/// Maps `java.util.logging` level names, which is what Skript logs with.
fn severity(level: Option<&str>) -> FindingSeverity {
    match level.map(str::to_ascii_uppercase).as_deref() {
        Some("SEVERE" | "ERROR") => FindingSeverity::Error,
        Some("WARNING" | "WARN") => FindingSeverity::Warning,
        _ => FindingSeverity::Info,
    }
}

/// The message of an `ERROR [SkAnalyzer] …` line. Errors from other loggers
/// (an addon, Skript itself) can appear mid-load without failing the request.
fn analyzer_error(plain: &str) -> Option<&str> {
    let (level, rest) = plain.trim().split_once("] ")?;
    if !level.starts_with('[') {
        return None;
    }
    let rest = rest.trim_start().strip_prefix("ERROR")?.trim_start();
    Some(rest.strip_prefix(ANALYZER_TAG)?.trim())
}

/// Removes ANSI escape sequences (CSI, OSC, nF and two-byte escapes).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in `@`..=`~`.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ST (`ESC \`).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // nF escapes (`ESC ( B`): intermediates, then one final byte.
            Some(' '..='/') => {
                for c in chars.by_ref() {
                    if !(' '..='/').contains(&c) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured verbatim from SkAnalyzer 2.0.0-beta.7 + Skript 2.16.1.
    const BROKEN_LINE: &str = r#"[02:56:26] INFO  [SkAnalyzer] {"results":{"C:/server/plugins/Skript/scripts/broken.sk":{"errors":[{"line":11,"message":"Can't understand this condition/effect: this is not a valid line","level":"SEVERE"},{"line":2,"message":"Can't understand this condition/effect: send \"hi\" to playerr","level":"SEVERE"},{"line":3,"message":"Can't understand this condition/effect: set {_x} to","level":"SEVERE"}],"structure":{"commands":[{"line":8,"value":"foo","aliases":[],"prefix":"skript","usage":"Correct usage: /foo","arguments":[]}],"events":[{"line":1,"value":"join","id":"join","eventPriority":"HIGH"}],"functions":[{"line":5,"value":"greet","local":false,"parameters":{"p":"player"}}],"options":{}},"usedAddons":["Skript"]}}}"#;
    const ADDON_LINE: &str = r#"[02:57:21] INFO  [SkAnalyzer] {"results":{"C:/server/plugins/Skript/scripts/addon2.sk":{"errors":[{"line":8,"message":"Can't understand this expression: 'nbt compound of player'","level":"SEVERE"},{"line":9,"message":"Can't understand this expression: 'bossbar named \"x\"'","level":"SEVERE"},{"line":10,"message":"Can't understand this expression: 'frobnicate the player'","level":"SEVERE"}],"structure":{"commands":[{"line":4,"value":"x","aliases":[],"prefix":"skript","usage":"Correct usage: /x","arguments":[]}],"events":[],"functions":[],"options":{}},"usedAddons":["Skript","skript-reflect"]}}}"#;
    const GOOD_LINE: &str = r#"[02:56:26] INFO  [SkAnalyzer] {"results":{"C:/server/plugins/Skript/scripts/good.sk":{"errors":[],"structure":{"commands":[],"events":[{"line":1,"value":"join","id":"join","eventPriority":"HIGH"}],"functions":[],"options":{}},"usedAddons":["Skript"]}}}"#;
    const MIXED_LINE: &str = r#"[03:07:07] INFO  [SkAnalyzer] {"results":{"C:/server/plugins/Skript/scripts/with space/edge.sk":{"errors":[{"line":6,"message":"Empty configuration section! You might want to indent one or more of the subsequent lines to make them belong to this section or remove the colon at the end of the line if you don't want this line to start a section.","level":"WARNING"},{"line":8,"message":"indentation error: expected 1 tab, but found '__->' [-> = tab, _ = space, ? = other whitespace]","level":"SEVERE"},{"line":1,"message":"Can't understand this structure: on jion","level":"SEVERE"},{"line":13,"message":"Cannot recognise the type 'numbr'","level":"SEVERE"},{"line":5,"message":"List is missing 'and' or 'or', defaulting to 'and': \"a\", \"b\"","level":"WARNING"}],"structure":{"commands":[],"events":[{"line":4,"value":"join","id":"join","eventPriority":"HIGH"}],"functions":[],"options":{"greeting":"hi"}},"usedAddons":["Skript"]}}}"#;
    const TWO_FILES_LINE: &str = r#"[03:07:05] INFO  [SkAnalyzer] {"results":{"C:/x/proj/b.sk":{"errors":[],"structure":{"commands":[],"events":[{"line":1,"value":"load","id":"script_loadunload","eventPriority":"HIGH"}],"functions":[],"options":{}},"usedAddons":["Skript"]},"C:/x/proj/a.sk":{"errors":[],"structure":{"commands":[],"events":[],"functions":[{"line":1,"value":"helper","local":false,"parameters":{"n":"number"},"returnType":"number"}],"options":{}},"usedAddons":["Skript"]}}}"#;
    const MISSING_FILE_LINE: &str = r"[03:07:04] ERROR [SkAnalyzer] Invalid argument! Specified file does not exist: C:\x\nope.sk";

    fn only(map: HashMap<PathBuf, Vec<Finding>>) -> (PathBuf, Vec<Finding>) {
        assert_eq!(map.len(), 1, "{map:?}");
        map.into_iter().next().unwrap()
    }

    #[test]
    fn parses_a_real_error_line_into_sorted_zero_based_findings() {
        let (path, findings) = only(parse_output_line(BROKEN_LINE).unwrap());
        assert!(path.ends_with("scripts/broken.sk") || path.ends_with(r"scripts\broken.sk"));
        let lines: Vec<u32> = findings.iter().map(|f| f.line).collect();
        assert_eq!(lines, [1, 2, 10]);
        assert_eq!(
            findings[0].message,
            "Can't understand this condition/effect: send \"hi\" to playerr"
        );
        assert!(findings
            .iter()
            .all(|f| f.severity == FindingSeverity::Error));
    }

    #[test]
    fn parses_addon_and_clean_lines() {
        let (_, findings) = only(parse_output_line(ADDON_LINE).unwrap());
        assert_eq!(findings.len(), 3);
        assert_eq!(findings[0].line, 7);
        let (_, findings) = only(parse_output_line(GOOD_LINE).unwrap());
        assert!(findings.is_empty());
    }

    #[test]
    fn maps_warning_and_severe_levels() {
        let (path, findings) = only(parse_output_line(MIXED_LINE).unwrap());
        assert!(path.to_string_lossy().contains("with space"));
        let summary: Vec<(u32, FindingSeverity)> =
            findings.iter().map(|f| (f.line, f.severity)).collect();
        assert_eq!(
            summary,
            [
                (0, FindingSeverity::Error),
                (4, FindingSeverity::Warning),
                (5, FindingSeverity::Warning),
                (7, FindingSeverity::Error),
                (12, FindingSeverity::Error),
            ]
        );
        assert_eq!(severity(Some("INFO")), FindingSeverity::Info);
        assert_eq!(severity(Some("severe")), FindingSeverity::Error);
        assert_eq!(severity(None), FindingSeverity::Info);
    }

    #[test]
    fn keeps_every_file_of_a_multi_file_answer() {
        let map = parse_output_line(TWO_FILES_LINE).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.keys().all(|path| path.is_absolute() || !cfg!(windows)));
    }

    #[test]
    fn ignores_lines_that_are_not_results() {
        for line in [
            "[02:56:25] INFO  [SkAnalyzer] Type 'help' for help.",
            "[03:08:26] INFO  [SkAnalyzer] Successfully unloaded specified script(s)",
            MISSING_FILE_LINE,
            "\tat java.base/java.lang.Thread.run(Thread.java:1474)",
            "[02:56:26] INFO  [SkAnalyzer] {\"results\":",
            "",
        ] {
            assert!(parse_output_line(line).is_none(), "{line}");
        }
    }

    #[test]
    fn tolerates_nulls_and_missing_fields() {
        let line = r#"{"results":{"/a.sk":{"errors":[{"line":null,"message":"x","level":null},{"line":0}]},"/b.sk":{"errors":null},"/c.sk":{}}}"#;
        let map = parse_output_line(line).unwrap();
        let a = &map[&native_path("/a.sk")];
        assert_eq!(a.len(), 2);
        assert!(a
            .iter()
            .all(|f| f.line == 0 && f.severity == FindingSeverity::Info));
        assert!(map[&native_path("/b.sk")].is_empty());
        assert!(map[&native_path("/c.sk")].is_empty());
    }

    #[test]
    fn strips_ansi_before_parsing() {
        let coloured = format!(
            "\u{1b}[37m[02:56:26] \u{1b}[32;1mINFO \u{1b}[0m \u{1b}[36m[SkAnalyzer]\u{1b}[0m {}\u{1b}[m\r",
            &GOOD_LINE[GOOD_LINE.find('{').unwrap()..]
        );
        assert!(parse_output_line(&coloured).is_some());
        assert_eq!(
            strip_ansi("a\u{1b}]0;title\u{7}b\u{1b}]8;;x\u{1b}\\c\u{1b}(Bd"),
            "abcd"
        );
        assert_eq!(strip_ansi("\u{1b}[1;31merror\u{1b}[0m"), "error");
    }

    #[test]
    fn recognises_only_analyzer_errors_as_rejections() {
        assert_eq!(
            analyzer_error(MISSING_FILE_LINE),
            Some(r"Invalid argument! Specified file does not exist: C:\x\nope.sk")
        );
        assert_eq!(
            analyzer_error("[03:08:30] ERROR [SkAnalyzer] Unknown command: frobnicate"),
            Some("Unknown command: frobnicate")
        );
        assert_eq!(analyzer_error("[02:57:19] ERROR [PluginLoader] boom"), None);
        assert_eq!(analyzer_error("[02:57:19] INFO  [SkAnalyzer] fine"), None);
    }

    #[test]
    fn matches_paths_across_separators_and_case() {
        assert_eq!(path_key(r"C:\a\b.sk"), path_key("C:/a/b.sk"));
        assert_eq!(path_key("C:/a/dir/"), path_key("C:/a/dir"));
        if cfg!(windows) {
            assert_eq!(path_key("c:/A/B.SK"), path_key("C:/a/b.sk"));
        } else {
            assert_ne!(path_key("/A/b.sk"), path_key("/a/b.sk"));
        }
    }

    #[test]
    fn reads_java_versions() {
        let graal = "java version \"25.0.4.1.1\" 2026-08-18 LTS\nJava(TM) SE Runtime";
        assert_eq!(parse_java_version(graal), Some((25, "25.0.4.1.1".into())));
        let ea = "openjdk version \"26-ea\" 2026-03-17";
        assert_eq!(parse_java_version(ea).map(|v| v.0), Some(26));
        let old = "Picked up _JAVA_OPTIONS: -Xmx1g\nopenjdk version \"1.8.0_392\"";
        assert_eq!(parse_java_version(old), Some((8, "1.8.0_392".into())));
        assert_eq!(
            parse_java_version("openjdk version \"21.0.2\""),
            Some((21, "21.0.2".into()))
        );
        assert_eq!(parse_java_version("bash: java: not found"), None);
    }

    #[tokio::test]
    async fn reports_a_java_that_cannot_run() {
        let missing = Path::new("definitely-not-a-java-binary-zedskript");
        assert_eq!(check_java(missing).await, Err(AnalyzerError::JavaMissing));
    }

    #[test]
    fn refuses_paths_the_shell_would_mangle() {
        let dir = scratch_dir("paths");
        let doubled = dir.join("two  spaces.sk");
        std::fs::write(&doubled, "").unwrap();
        assert!(matches!(
            command_path(&doubled),
            Err(AnalyzerError::BadPath(_))
        ));
        assert!(matches!(
            command_path(&dir.join("missing.sk")),
            Err(AnalyzerError::BadPath(_))
        ));
        assert!(matches!(command_path(&dir), Err(AnalyzerError::BadPath(_))));

        let single = dir.join("one space.sk");
        std::fs::write(&single, "").unwrap();
        let sent = command_path(&single).unwrap();
        assert!(sent.ends_with("/one space.sk"), "{sent}");
        assert!(!sent.contains('\\') && !sent.starts_with("//?"), "{sent}");
    }

    // ---- process behaviour against a fake shell -------------------------

    /// Mimics SkAnalyzer's protocol. The file name picks the behaviour.
    const FAKE: &str = r#"
const mode = process.argv[2] || 'normal';
const stamp = () => '\x1b[37m[12:00:00]\x1b[0m';
const log = (level, tag, msg) => process.stdout.write(`${stamp()} ${level} [${tag}] ${msg}\r\n`);
process.stdout.write('SkAnalyzer v0-fake\r\n');
process.stderr.write('WARNING: A restricted method in java.lang.System has been called\n');
log('INFO ', 'SkAnalyzer', 'Starting...');
if (mode === 'nobridge') {
  log('ERROR', 'SkAnalyzer', 'Required Skript bridge is missing!');
  process.exit(1);
}
if (mode === 'die') { process.stderr.write('Error: boom\n'); process.exit(7); }
if (mode !== 'silent') log('INFO ', 'SkAnalyzer', "\x1b[1mType 'help' for help.\x1b[0m");
let buf = '';
process.stdin.on('data', d => {
  buf += d;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i).trim(); buf = buf.slice(i + 1);
    const [cmd, ...rest] = line.split(' ');
    const path = rest.join(' ');
    if (cmd === 'exit') process.exit(0);
    if (cmd !== 'load') { log('ERROR', 'SkAnalyzer', `Unknown command: ${cmd}`); continue; }
    if (path.includes('hang')) continue;
    if (path.includes('crash')) { process.stderr.write('Exception in thread "main"\n'); process.exit(3); }
    if (path.includes('reject')) { log('ERROR', 'SkAnalyzer', `Invalid argument! ${path}`); continue; }
    // Noise a real load can produce before its answer.
    log('ERROR', 'SkBee', 'addon complained');
    log('INFO ', 'SkAnalyzer', JSON.stringify({ results: { 'C:/elsewhere/other.sk': { errors: [] } } }));
    const reported = /^[a-z]:/.test(path) ? path[0].toUpperCase() + path.slice(1) : path;
    const swapped = process.platform === 'win32' ? reported.replace(/^./, c => c === c.toUpperCase() ? c.toLowerCase() : c.toUpperCase()) : reported;
    log('INFO ', 'SkAnalyzer', JSON.stringify({ results: { [swapped]: { errors: [
      { line: 2, message: "Can't understand this condition/effect: x", level: 'SEVERE' },
      { line: 1, message: 'List is missing and', level: 'WARNING' },
    ], structure: {}, usedAddons: ['Skript'] } } }));
  }
});
"#;

    /// A fresh directory per call: tests run in parallel.
    fn scratch_dir(name: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "zedskript-analyzer-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn fake(mode: &str, start_limit: Duration) -> (Result<Analyzer, AnalyzerError>, PathBuf) {
        let dir = scratch_dir(mode);
        let script = dir.join("fake-skanalyzer.js");
        std::fs::write(&script, FAKE).unwrap();
        let mut command = Command::new("node");
        command.arg(&script).arg(mode).current_dir(&dir);
        (
            launch(command, start_limit, Duration::from_millis(1500)).await,
            dir,
        )
    }

    fn script(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "on join:\n\tx\n").unwrap();
        path
    }

    #[tokio::test]
    async fn answers_checks_matched_by_path() {
        let (analyzer, dir) = fake("normal", Duration::from_secs(20)).await;
        let analyzer = analyzer.unwrap();
        let path = script(&dir, "ok file.sk");
        for _ in 0..2 {
            let map = analyzer.check(&path).await.unwrap();
            assert_eq!(map.len(), 1, "unrelated answers must be skipped: {map:?}");
            let findings = &map[&path];
            assert_eq!(findings[0].line, 0);
            assert_eq!(findings[0].severity, FindingSeverity::Warning);
            assert_eq!(findings[1].line, 1);
            assert_eq!(findings[1].severity, FindingSeverity::Error);
        }
        analyzer.shutdown().await;
    }

    #[tokio::test]
    async fn rejection_fails_fast_and_keeps_the_shell() {
        let (analyzer, dir) = fake("normal", Duration::from_secs(20)).await;
        let analyzer = analyzer.unwrap();
        let started = std::time::Instant::now();
        let error = analyzer
            .check(&script(&dir, "reject.sk"))
            .await
            .unwrap_err();
        assert!(
            matches!(error, AnalyzerError::Rejected(ref m) if m.starts_with("Invalid argument"))
        );
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "must not wait for the timeout"
        );
        assert!(analyzer.check(&script(&dir, "fine.sk")).await.is_ok());
        analyzer.shutdown().await;
    }

    #[tokio::test]
    async fn a_hang_times_out_and_the_shell_is_retired() {
        let (analyzer, dir) = fake("normal", Duration::from_secs(20)).await;
        let analyzer = analyzer.unwrap();
        let error = analyzer.check(&script(&dir, "hang.sk")).await.unwrap_err();
        assert_eq!(error, AnalyzerError::Timeout);
        let next = analyzer.check(&script(&dir, "fine.sk")).await.unwrap_err();
        assert!(matches!(next, AnalyzerError::Crashed(ref m) if m.contains("timed out")));
        analyzer.shutdown().await;
    }

    #[tokio::test]
    async fn a_crash_surfaces_with_its_output() {
        let (analyzer, dir) = fake("normal", Duration::from_secs(20)).await;
        let analyzer = analyzer.unwrap();
        let error = analyzer.check(&script(&dir, "crash.sk")).await.unwrap_err();
        let AnalyzerError::Crashed(detail) = error else {
            panic!("expected Crashed, got {error:?}");
        };
        assert!(detail.contains('3'), "{detail}");
        assert!(detail.contains("Exception in thread"), "{detail}");
        let next = analyzer.check(&script(&dir, "fine.sk")).await.unwrap_err();
        assert!(matches!(next, AnalyzerError::Crashed(_)));
        analyzer.shutdown().await;
    }

    #[tokio::test]
    async fn startup_failures_are_distinguished() {
        let (result, _) = fake("nobridge", Duration::from_secs(20)).await;
        assert_eq!(result.err(), Some(AnalyzerError::SkriptBridgeMissing));

        let (result, _) = fake("die", Duration::from_secs(20)).await;
        match result {
            Err(AnalyzerError::Crashed(detail)) => assert!(detail.contains("boom"), "{detail}"),
            other => panic!("expected Crashed, got {:?}", other.err()),
        }

        let started = std::time::Instant::now();
        let (result, _) = fake("silent", Duration::from_millis(1500)).await;
        assert_eq!(result.err(), Some(AnalyzerError::StartTimeout));
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn start_checks_its_inputs_before_booting() {
        let dir = scratch_dir("inputs");
        let config = |java: &str, jar: PathBuf| AnalyzerConfig {
            java: PathBuf::from(java),
            jar,
            work_dir: dir.join("work"),
            plugins: Vec::new(),
            timeout: Duration::from_secs(5),
        };
        let missing_java = config("definitely-not-a-java-binary-zedskript", dir.join("x.jar"));
        assert_eq!(
            Analyzer::start(missing_java).await.err(),
            Some(AnalyzerError::JavaMissing)
        );

        // The remaining checks need a Java that passes the version probe.
        if check_java(Path::new("java")).await.is_err() {
            return;
        }
        let no_jar = config("java", dir.join("absent.jar"));
        assert_eq!(
            Analyzer::start(no_jar).await.err(),
            Some(AnalyzerError::JarMissing)
        );
        let jar = dir.join("fake.jar");
        std::fs::write(&jar, "").unwrap();
        let no_bridge = config("java", jar);
        assert_eq!(
            Analyzer::start(no_bridge).await.err(),
            Some(AnalyzerError::SkriptBridgeMissing)
        );
    }

    /// The real thing. Needs Java 25 on PATH and:
    /// `SKANALYZER_JAR` — the shell jar; `SKANALYZER_WORKDIR` — a work dir whose
    /// `plugins/` holds a Skratched Skript build.
    #[tokio::test]
    #[ignore = "needs Java 25, SkAnalyzer and a Skratched Skript build"]
    async fn real_skanalyzer_reports_a_severe_error_on_the_right_line() {
        let jar = std::env::var_os("SKANALYZER_JAR").expect("set SKANALYZER_JAR");
        let work_dir = std::env::var_os("SKANALYZER_WORKDIR").expect("set SKANALYZER_WORKDIR");
        let dir = scratch_dir("real");
        let path = dir.join("broken.sk");
        std::fs::write(&path, "on join:\n\tsend \"hi\" to playerr\n").unwrap();

        let started = std::time::Instant::now();
        let analyzer = Analyzer::start(AnalyzerConfig {
            java: PathBuf::from("java"),
            jar: jar.into(),
            work_dir: work_dir.into(),
            plugins: Vec::new(),
            timeout: Duration::from_secs(30),
        })
        .await
        .unwrap();
        let boot = started.elapsed();

        let first = std::time::Instant::now();
        let map = analyzer.check(&path).await.unwrap();
        let first = first.elapsed();
        let findings = &map[&path];
        assert!(
            findings
                .iter()
                .any(|f| f.line == 1 && f.severity == FindingSeverity::Error),
            "{findings:?}"
        );

        std::fs::write(&path, "on join:\n\tsend \"hi\" to player\n").unwrap();
        let again = std::time::Instant::now();
        let map = analyzer.check(&path).await.unwrap();
        let again = again.elapsed();
        assert!(
            map[&path].is_empty(),
            "a fixed file must come back clean: {map:?}"
        );

        analyzer.shutdown().await;
        eprintln!("boot {boot:?}, first check {first:?}, re-check {again:?}");
    }
}
