//! "Reload this script on my server" — an opt-in RCON bridge.
//!
//! On save, the script is reloaded on a running server with
//! `skript reload <name>`, and the errors Skript prints come back as
//! diagnostics. Off by default: it contacts a server, and it changes what that
//! server is running.
//!
//! What the reply contains was checked against Skript 2.17.1 and Paper:
//!
//! * `ReloadCommand` routes the reload's log through a `RedirectingLogHandler`
//!   whose recipients include the command sender, and that handler returns
//!   `DO_NOT_LOG`. So the errors go to the RCON sender (and players with
//!   `skript.reloadnotify`) and *not* to the console.
//! * Paper runs an RCON command on the main thread and replies with whatever
//!   was sent to the sender while it ran. With the default
//!   `script loader thread size: 0` Skript parses inside the command, so the
//!   errors and the "Encountered N errors" summary are in the reply. With
//!   async loading they arrive after the reply has gone; the reply then holds
//!   only "Reloading …", and the result is reported as unknown rather than
//!   as success.
//! * Paper serialises each message to legacy `§` text and appends `\n`. The
//!   colour survives, and is the only thing that tells a warning's
//!   `Line 3: (x.sk)` header from an error's.
//!
//! The vanilla RCON server reads one packet per socket read and drops the
//! connection when a read holds more than one, so a request is never sent
//! while another is unanswered.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tower_lsp::lsp_types::{Diagnostic, MessageType, NumberOrString, Url};

use crate::analyzer::{Finding, FindingSeverity};
use crate::convert::Encoding;
use crate::server::{Backend, State};
use crate::settings::RconSettings;

const AUTH: i32 = 3;
const AUTH_RESPONSE: i32 = 2;
const EXEC_COMMAND: i32 = 2;

const AUTH_ID: i32 = 1;
const COMMAND_ID: i32 = 2;
/// The id of the empty command sent after the real one. Its reply can only
/// come once every fragment of the real reply has been written, so it marks
/// the end of a reply split across packets.
const END_ID: i32 = 3;

/// Header (id and type) plus the two terminating NULs.
const MIN_PACKET: usize = 10;
/// Minecraft splits replies at 4096 body bytes; other servers are allowed a
/// little more before a length is treated as garbage.
const MAX_PACKET: usize = 64 * 1024;
/// A reply bigger than this is a misbehaving server, not a long error list.
const MAX_REPLY: usize = 4 * 1024 * 1024;

pub const SOURCE: &str = "Skript (server)";
pub const CODE: &str = "server-reload";

/// The RCON side of the server state.
#[derive(Debug, Default)]
pub struct RconState {
    pub settings: RconSettings,
    /// Files with a reload running. A save while one runs only marks the file
    /// in `pending`, and the running task goes again when it finishes, so
    /// rapid saves never stack up connections.
    pub in_flight: std::collections::HashSet<String>,
    pub pending: std::collections::HashSet<String>,
    /// Set once a failure has been shown in a window. Later failures are only
    /// logged, until the RCON settings change (see `apply_settings`).
    pub warned: bool,
    /// Per document, from the last reload. Kept until the next save, cleared
    /// on close.
    pub findings: std::collections::HashMap<String, Vec<Finding>>,
}

impl RconState {
    /// Claims `key` for a reload. `false` when one is already running; the
    /// save is then folded into that run instead.
    pub fn begin(&mut self, key: &str) -> bool {
        if self.in_flight.contains(key) {
            self.pending.insert(key.to_string());
            return false;
        }
        self.in_flight.insert(key.to_string());
        true
    }

    /// Ends a run for `key`. `true` when a save came in meanwhile, in which
    /// case the caller keeps the claim and reloads again.
    pub fn finish(&mut self, key: &str) -> bool {
        if self.pending.remove(key) {
            return true;
        }
        self.in_flight.remove(key);
        false
    }
}

/// Where to send commands. Deliberately not `Debug`: it holds the password.
pub struct Target {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub timeout: Duration,
}

impl Target {
    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RconError {
    /// The connection could not be made.
    Unreachable(String),
    /// The server answered the login with id -1.
    AuthRejected,
    /// No complete answer within the configured time.
    Timeout,
    /// The server sent something that is not RCON, or hung up mid-reply.
    Protocol(String),
}

impl fmt::Display for RconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(detail) => write!(f, "could not connect ({detail})"),
            Self::AuthRejected => write!(f, "the RCON password was rejected"),
            Self::Timeout => write!(f, "no answer in time"),
            Self::Protocol(detail) => write!(f, "unexpected RCON traffic ({detail})"),
        }
    }
}

impl From<io::Error> for RconError {
    fn from(error: io::Error) -> Self {
        RconError::Protocol(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub id: i32,
    pub kind: i32,
    pub body: Vec<u8>,
}

/// One packet: length, id and type as little-endian i32, the body, two NULs.
pub fn encode(id: i32, kind: i32, body: &str) -> Vec<u8> {
    let length = (body.len() + MIN_PACKET) as i32;
    let mut bytes = Vec::with_capacity(body.len() + MIN_PACKET + 4);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(&[0, 0]);
    bytes
}

pub async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Packet, RconError> {
    let length = reader.read_i32_le().await?;
    let length = usize::try_from(length)
        .ok()
        .filter(|length| (MIN_PACKET..=MAX_PACKET).contains(length))
        .ok_or_else(|| RconError::Protocol(format!("packet length {length}")))?;
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload).await?;
    let id = i32::from_le_bytes(payload[0..4].try_into().expect("4 bytes"));
    let kind = i32::from_le_bytes(payload[4..8].try_into().expect("4 bytes"));
    let mut body = payload.split_off(8);
    // Two NULs per the protocol; trimmed rather than cut so a server that
    // sends one is still understood.
    while body.last() == Some(&0) {
        body.pop();
    }
    Ok(Packet { id, kind, body })
}

/// Logs in, runs `command` and returns its whole reply, all within the
/// target's timeout.
pub async fn run_command(target: &Target, command: &str) -> Result<String, RconError> {
    tokio::time::timeout(target.timeout, exchange(target, command))
        .await
        .unwrap_or(Err(RconError::Timeout))
}

async fn exchange(target: &Target, command: &str) -> Result<String, RconError> {
    let mut stream = TcpStream::connect((target.host.as_str(), target.port))
        .await
        .map_err(|error| RconError::Unreachable(error.to_string()))?;
    login(&mut stream, &target.password).await?;
    command_reply(&mut stream, command).await
}

async fn login<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    password: &str,
) -> Result<(), RconError> {
    stream.write_all(&encode(AUTH_ID, AUTH, password)).await?;
    loop {
        let packet = read_packet(stream).await?;
        // Source servers send an empty response value before the verdict;
        // Minecraft does not. Either way only the auth response counts.
        if packet.kind != AUTH_RESPONSE {
            continue;
        }
        return match packet.id {
            AUTH_ID => Ok(()),
            -1 => Err(RconError::AuthRejected),
            other => Err(RconError::Protocol(format!(
                "login answered with id {other}"
            ))),
        };
    }
}

async fn command_reply<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    command: &str,
) -> Result<String, RconError> {
    stream
        .write_all(&encode(COMMAND_ID, EXEC_COMMAND, command))
        .await?;
    let mut reply = Vec::new();
    let mut end_sent = false;
    loop {
        let packet = read_packet(stream).await?;
        match packet.id {
            COMMAND_ID => {
                // Collected as bytes: Minecraft splits at a byte count, which
                // can fall inside a multi-byte character.
                reply.extend_from_slice(&packet.body);
                if reply.len() > MAX_REPLY {
                    return Err(RconError::Protocol("reply too large".into()));
                }
                // Only now, with the server busy writing, is a second packet
                // safe to send; see the module comment.
                if !end_sent {
                    stream.write_all(&encode(END_ID, EXEC_COMMAND, "")).await?;
                    end_sent = true;
                }
            }
            END_ID => return Ok(String::from_utf8_lossy(&reply).into_owned()),
            -1 => return Err(RconError::AuthRejected),
            _ => {}
        }
    }
}

/// How a reload went, as far as the reply says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Skript reported success. Any findings are warnings.
    Reloaded,
    /// Skript counted this many errors.
    Errors(usize),
    /// Skript refused before parsing (no such script, or it is disabled).
    Refused(String),
    /// The reply does not say. Carries its first line, for the log.
    Unconfirmed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub verdict: Verdict,
    pub findings: Vec<Finding>,
}

/// One line of the reply: its plain text and the colour it starts in.
struct ReplyLine {
    colour: Option<char>,
    text: String,
}

fn reply_line(raw: &str) -> ReplyLine {
    let mut colour = None;
    let mut seen_text = false;
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '§' {
            if let Some(code) = chars.next() {
                let code = code.to_ascii_lowercase();
                if !seen_text && colour.is_none() && code.is_ascii_hexdigit() {
                    colour = Some(code);
                }
            }
            continue;
        }
        if !c.is_whitespace() {
            seen_text = true;
        }
        text.push(c);
    }
    ReplyLine {
        colour,
        text: text.trim_end().to_string(),
    }
}

/// `Line 3: (sub/x.sk)`, the header Skript puts above each error. Matched by
/// shape rather than by the English word, so a translated `lang` still works.
fn line_header(text: &str) -> Option<(u32, &str)> {
    let (_, rest) = text.trim().split_once(' ')?;
    let (number, rest) = rest.split_once(": (")?;
    let file = rest.strip_suffix(')')?;
    let line = number.parse().ok()?;
    file.to_ascii_lowercase()
        .ends_with(".sk")
        .then_some((line, file))
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn severity_of(colour: Option<char>) -> FindingSeverity {
    match colour {
        // `gold` and `yellow` are what Skript's lang file gives warnings.
        Some('6' | 'e') => FindingSeverity::Warning,
        _ => FindingSeverity::Error,
    }
}

/// Reads Skript's reply to `skript reload <name>`.
pub fn parse_reply(reply: &str, name: &str) -> Report {
    let wanted = file_name(name).to_ascii_lowercase();
    let mut findings = Vec::new();
    let mut verdict = None;
    // A header waiting for its message line.
    let mut open: Option<(u32, FindingSeverity, bool)> = None;

    for raw in reply.lines() {
        let line = reply_line(raw);
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some((number, file)) = line_header(text) {
            let ours = file_name(file).to_ascii_lowercase() == wanted;
            open = Some((number.saturating_sub(1), severity_of(line.colour), ours));
            continue;
        }
        if let Some((number, severity, ours)) = open.take() {
            if ours {
                findings.push(Finding {
                    line: number,
                    severity,
                    message: text.to_string(),
                });
            }
            continue;
        }
        if text.contains("Successfully reloaded") {
            verdict = Some(Verdict::Reloaded);
        } else if let Some(count) = error_count(text) {
            verdict = Some(Verdict::Errors(count));
        } else if text.contains("Can't find the script") || text.contains("is currently disabled") {
            verdict = Some(Verdict::Refused(text.to_string()));
        } else if line.colour == Some('c') && raw.trim_start_matches(['§', 'c']).starts_with(' ') {
            // An error with no line attached, indented like a header's
            // message. Pinned to the first line so it is seen.
            findings.push(Finding {
                line: 0,
                severity: FindingSeverity::Error,
                message: text.to_string(),
            });
        }
    }

    let verdict = verdict.unwrap_or_else(|| {
        let errors = findings
            .iter()
            .filter(|finding| finding.severity == FindingSeverity::Error)
            .count();
        if errors > 0 {
            Verdict::Errors(errors)
        } else {
            let first = reply.lines().map(|raw| reply_line(raw).text).next();
            Verdict::Unconfirmed(first.unwrap_or_default().trim().to_string())
        }
    });
    // Skript counted no errors, so whatever a header's colour suggested,
    // none of these is one.
    if verdict == Verdict::Reloaded {
        for finding in &mut findings {
            finding.severity = FindingSeverity::Warning;
        }
    }
    Report { verdict, findings }
}

/// `Encountered 2 errors while reloading x.sk!`
fn error_count(text: &str) -> Option<usize> {
    let after = text.split_once("Encountered ")?.1;
    if !after.contains(" while reloading") {
        return None;
    }
    after.split_whitespace().next()?.parse().ok()
}

/// Why a save does not lead to a reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Off, or not a file. Silent.
    Off,
    NoPassword,
    /// Not under the scripts folder; carries the file and the folder used.
    Outside(PathBuf, Option<PathBuf>),
    /// A `-name.sk` file, which Skript treats as disabled.
    DisabledScript(String),
}

pub struct Job {
    pub name: String,
    pub target: Target,
}

/// What a save of `uri` should do under the current settings.
fn plan(state: &State, uri: &Url) -> Result<Job, Skip> {
    let settings = &state.rcon.settings;
    if !settings.enabled {
        return Err(Skip::Off);
    }
    let Ok(path) = uri.to_file_path() else {
        return Err(Skip::Off);
    };
    let password = settings
        .password
        .clone()
        .filter(|password| !password.is_empty())
        .ok_or(Skip::NoPassword)?;
    let root = scripts_root(
        &path,
        settings.scripts_root.as_deref().map(Path::new),
        state.detection.plugins_dir.as_deref(),
    );
    let name = root
        .as_deref()
        .and_then(|root| script_name(&path, root))
        .ok_or_else(|| Skip::Outside(path.clone(), root.clone()))?;
    if file_name(&name).starts_with('-') {
        return Err(Skip::DisabledScript(name));
    }
    Ok(Job {
        name,
        target: Target {
            host: settings.host.clone(),
            port: settings.port,
            password,
            timeout: Duration::from_millis(settings.timeout_ms.max(1)),
        },
    })
}

/// The server's `plugins/Skript/scripts`: the configured one, else the one
/// in the detected `plugins/` when the file is inside it, else the nearest
/// `Skript/scripts` above the file.
pub fn scripts_root(
    file: &Path,
    configured: Option<&Path>,
    plugins: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(root) = configured {
        return Some(root.to_path_buf());
    }
    if let Some(root) = plugins.map(|plugins| plugins.join("Skript").join("scripts")) {
        if script_name(file, &root).is_some() {
            return Some(root);
        }
    }
    file.ancestors()
        .find(|dir| {
            let named = |dir: Option<&Path>, name: &str| {
                dir.and_then(Path::file_name)
                    .and_then(|found| found.to_str())
                    .is_some_and(|found| found.eq_ignore_ascii_case(name))
            };
            named(Some(dir), "scripts") && named(dir.parent(), "Skript")
        })
        .map(Path::to_path_buf)
}

/// The name `skript reload` takes for `file`: its path below `root`, with
/// `/` separators, which Skript accepts on every OS.
pub fn script_name(file: &Path, root: &Path) -> Option<String> {
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
    let relative = canonical(file)
        .strip_prefix(canonical(root))
        .ok()?
        .to_path_buf();
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    let name = parts.join("/");
    let is_script = name.to_ascii_lowercase().ends_with(".sk");
    // A control character would end or split the command line.
    (is_script && !name.chars().any(char::is_control)).then_some(name)
}

/// Asks the server to reload `name` and reads the verdict.
pub async fn reload(target: &Target, name: &str) -> Result<Report, RconError> {
    // `skript` rather than the `sk` alias: an alias can be claimed by another
    // plugin, the command's own name cannot.
    let reply = run_command(target, &format!("skript reload {name}")).await?;
    Ok(parse_reply(&reply, name))
}

/// The findings as diagnostics, placed the way SkAnalyzer's are.
pub fn server_diagnostics(
    document: &skript_index::Document,
    findings: &[Finding],
    encoding: Encoding,
) -> Vec<Diagnostic> {
    crate::publish::analyzer_diagnostics(document, findings, encoding)
        .into_iter()
        .map(|diagnostic| Diagnostic {
            source: Some(SOURCE.into()),
            code: Some(NumberOrString::String(CODE.into())),
            ..diagnostic
        })
        .collect()
}

impl Backend {
    /// Reloads a saved script on the server, when RCON is enabled. Returns at
    /// once; the exchange runs on its own task.
    pub async fn reload_on_server(&self, uri: Url) {
        let key = uri.to_string();
        let first = {
            let mut state = self.state.write().await;
            if !state.rcon.settings.enabled || !state.rcon.begin(&key) {
                return;
            }
            plan(&state, &uri)
        };
        let backend = self.handle();
        tokio::spawn(async move { backend.reload_until_settled(key, uri, first).await });
    }

    async fn reload_until_settled(self, key: String, uri: Url, first: Result<Job, Skip>) {
        let mut next = first;
        loop {
            match next {
                Ok(job) => {
                    let outcome = reload(&job.target, &job.name).await;
                    self.settle(&key, &job, outcome).await;
                }
                Err(skip) => self.skipped(skip).await,
            }
            let mut state = self.state.write().await;
            if !state.rcon.finish(&key) {
                return;
            }
            // Planned afresh: the settings may have changed meanwhile.
            next = plan(&state, &uri);
        }
    }

    async fn settle(&self, key: &str, job: &Job, outcome: Result<Report, RconError>) {
        let report = match outcome {
            Ok(report) => report,
            Err(error) => {
                let address = job.target.address();
                let message = match error {
                    RconError::AuthRejected => format!(
                        "RCON password rejected by {address}; check `rcon.password`. \
                         This will not be shown again until the RCON settings change."
                    ),
                    other => format!(
                        "Couldn't reach the server's RCON at {address}: {other}. Saving still \
                         works; this will not be shown again until the RCON settings change."
                    ),
                };
                self.warn_once(message).await;
                return;
            }
        };

        let (level, line) = match &report.verdict {
            Verdict::Reloaded => (
                MessageType::INFO,
                format!("reloaded {} on the server", job.name),
            ),
            Verdict::Errors(count) => (
                MessageType::INFO,
                format!("{} has {count} error(s) on the server", job.name),
            ),
            Verdict::Refused(reason) => (MessageType::WARNING, format!("server: {reason}")),
            Verdict::Unconfirmed(first) => (
                MessageType::WARNING,
                format!(
                    "the server's reply to reloading {} did not say how it went (it began \
                     \"{first}\"). With `script loader thread size` above 0 in Skript's \
                     config.sk, reload errors arrive after the RCON reply and are not shown.",
                    job.name
                ),
            ),
        };
        self.client.log_message(level, line).await;

        {
            let mut state = self.state.write().await;
            // Turned off, or the tab closed, while the reload ran.
            if !state.rcon.settings.enabled || !state.open_in_editor.contains(key) {
                return;
            }
            if report.findings.is_empty() {
                state.rcon.findings.remove(key);
            } else {
                state.rcon.findings.insert(key.to_string(), report.findings);
            }
        }
        self.publish(key).await;
    }

    async fn skipped(&self, skip: Skip) {
        match skip {
            Skip::Off => {}
            Skip::NoPassword => {
                self.warn_once(
                    "RCON reload is enabled but `rcon.password` is not set, so nothing is sent. \
                     This will not be shown again until the RCON settings change."
                        .into(),
                )
                .await;
            }
            Skip::Outside(file, root) => {
                let root = root.map_or_else(
                    || "no Skript scripts folder was found; set `rcon.scriptsRoot`".to_string(),
                    |root| format!("it is not under {}", root.display()),
                );
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!("not reloading {} on the server: {root}", file.display()),
                    )
                    .await;
            }
            Skip::DisabledScript(name) => {
                self.client
                    .log_message(
                        MessageType::INFO,
                        format!("not reloading {name}: Skript treats `-` scripts as disabled"),
                    )
                    .await;
            }
        }
    }

    /// A window the first time, the log after that.
    async fn warn_once(&self, message: String) {
        let first = !std::mem::replace(&mut self.state.write().await.rcon.warned, true);
        if first {
            self.client
                .show_message(MessageType::WARNING, message.clone())
                .await;
        }
        self.client.log_message(MessageType::WARNING, message).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// What Paper sends back for `skript reload broken.sk` with two errors
    /// and a warning, built from Skript 2.17.1's english.lang templates as the
    /// legacy serializer renders them.
    const FAILED: &str = "§7[§6Skript§7] §rReloading §6broken.sk§r...\n\
        §c§lLine 3:§7 (broken.sk)§r\n§c    Can't understand this condition/effect: frobnicate the player§r\n§6    Line: §7frobnicate the player§r\n §r\n\
        §6§lLine 5:§7 (broken.sk)§r\n§e    The variable {_x} is never set§r\n§6    Line: §7send {_x}§r\n §r\n\
        §c§lLine 9:§7 (broken.sk)§r\n§c    'on joinn' is not a valid event§r\n§6    Line: §7on joinn:§r\n §r\n\
        §7[§6Skript§7] §r§cEncountered §62 §cerrors while reloading §6broken.sk§c! §7(§612ms§7)\n";

    const RELOADED: &str = "§7[§6Skript§7] §rReloading §6sub/fine.sk§r...\n\
        §7[§6Skript§7] §r§aSuccessfully reloaded §6sub/fine.sk§a. §7(§63ms§7)\n";

    #[test]
    fn encodes_a_packet() {
        let bytes = encode(7, EXEC_COMMAND, "list");
        assert_eq!(&bytes[0..4], &14i32.to_le_bytes());
        assert_eq!(&bytes[4..8], &7i32.to_le_bytes());
        assert_eq!(&bytes[8..12], &2i32.to_le_bytes());
        assert_eq!(&bytes[12..16], b"list");
        assert_eq!(&bytes[16..], &[0, 0]);
    }

    #[tokio::test]
    async fn decodes_what_it_encodes_and_rejects_bad_lengths() {
        let bytes = encode(-1, AUTH_RESPONSE, "");
        let packet = read_packet(&mut bytes.as_slice()).await.unwrap();
        assert_eq!(
            packet,
            Packet {
                id: -1,
                kind: AUTH_RESPONSE,
                body: Vec::new()
            }
        );

        for length in [3i32, -5, (MAX_PACKET + 1) as i32] {
            let mut bytes = length.to_le_bytes().to_vec();
            bytes.extend_from_slice(&[0; 16]);
            let error = read_packet(&mut bytes.as_slice()).await.unwrap_err();
            assert!(matches!(error, RconError::Protocol(_)), "{length}: {error}");
        }
        // Cut off mid-packet.
        let bytes = encode(1, 0, "hello");
        assert!(read_packet(&mut &bytes[..9]).await.is_err());
    }

    #[test]
    fn parses_errors_and_warnings_with_zero_based_lines() {
        let report = parse_reply(FAILED, "broken.sk");
        assert_eq!(report.verdict, Verdict::Errors(2));
        let summary: Vec<_> = report
            .findings
            .iter()
            .map(|finding| (finding.line, finding.severity, finding.message.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    2,
                    FindingSeverity::Error,
                    "Can't understand this condition/effect: frobnicate the player"
                ),
                (
                    4,
                    FindingSeverity::Warning,
                    "The variable {_x} is never set"
                ),
                (8, FindingSeverity::Error, "'on joinn' is not a valid event"),
            ]
        );
    }

    #[test]
    fn a_clean_reload_is_a_success_with_nothing_to_report() {
        let report = parse_reply(RELOADED, "sub/fine.sk");
        assert_eq!(report.verdict, Verdict::Reloaded);
        assert!(report.findings.is_empty());
    }

    #[test]
    fn a_success_never_carries_errors() {
        // Without colour codes a warning header looks like an error's; the
        // summary settles it.
        let reply = "Reloading x.sk...\nLine 2: (x.sk)\n    Something is odd\n    Line: odd\n \n\
                     [Skript] Successfully reloaded x.sk. (2ms)\n";
        let report = parse_reply(reply, "x.sk");
        assert_eq!(report.verdict, Verdict::Reloaded);
        assert_eq!(report.findings[0].severity, FindingSeverity::Warning);
    }

    #[test]
    fn reports_refusals_and_replies_that_say_nothing() {
        let missing =
            "§7[§6Skript§7] §r§cCan't find the script §7'§6gone.sk§7'§c in the scripts folder!\n";
        assert!(matches!(
            parse_reply(missing, "gone.sk").verdict,
            Verdict::Refused(reason) if reason.contains("gone.sk")
        ));

        // Async loading: the reply ends before Skript has parsed anything.
        let early = "§7[§6Skript§7] §rReloading §6x.sk§r...\n";
        assert_eq!(
            parse_reply(early, "x.sk").verdict,
            Verdict::Unconfirmed("[Skript] Reloading x.sk...".into())
        );
        assert!(matches!(
            parse_reply("Unknown or incomplete command", "x.sk").verdict,
            Verdict::Unconfirmed(_)
        ));
    }

    #[test]
    fn a_translated_reply_still_yields_its_errors() {
        let reply =
            "§c§lZeile 4:§7 (x.sk)§r\n§c    Kann das nicht verstehen§r\n§6    Zeile: §7foo§r\n";
        let report = parse_reply(reply, "x.sk");
        assert_eq!(report.verdict, Verdict::Errors(1));
        assert_eq!(report.findings[0].line, 3);
    }

    #[test]
    fn ignores_other_files_and_keeps_errors_without_a_line() {
        let reply = "§c§lLine 1:§7 (other.sk)§r\n§c    not ours§r\n\
                     §c    Could not load x.sk: access denied§r\n\
                     §7[§6Skript§7] §r§cEncountered §61 §cerror while reloading §6x.sk§c!\n";
        let report = parse_reply(reply, "folder/x.sk");
        assert_eq!(report.verdict, Verdict::Errors(1));
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].line, 0);
        assert!(report.findings[0].message.starts_with("Could not load"));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skript-lsp-rcon-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "on load:\n\tbroadcast \"hi\"\n").unwrap();
    }

    #[test]
    fn names_a_script_by_its_path_under_the_scripts_folder() {
        let server = scratch("names");
        let scripts = server.join("plugins").join("Skript").join("scripts");
        let nested = scripts.join("games").join("arena.sk");
        let top = scripts.join("hub.sk");
        let outside = server.join("elsewhere.sk");
        for file in [&nested, &top, &outside] {
            touch(file);
        }

        assert_eq!(
            script_name(&nested, &scripts).as_deref(),
            Some("games/arena.sk")
        );
        assert_eq!(script_name(&top, &scripts).as_deref(), Some("hub.sk"));
        assert_eq!(script_name(&outside, &scripts), None);

        // Found from the detected plugins folder, or failing that, from the
        // file's own ancestors.
        let plugins = server.join("plugins");
        assert_eq!(
            scripts_root(&nested, None, Some(&plugins)),
            Some(scripts.clone())
        );
        assert_eq!(scripts_root(&nested, None, None), Some(scripts.clone()));
        assert_eq!(scripts_root(&outside, None, Some(&plugins)), None);
        // An explicit setting wins outright.
        assert_eq!(
            scripts_root(&nested, Some(&server), Some(&plugins)),
            Some(server.clone())
        );
        assert_eq!(
            script_name(&outside, &server).as_deref(),
            Some("elsewhere.sk")
        );
    }

    #[test]
    fn plans_only_when_enabled_configured_and_inside_the_folder() {
        let server = scratch("plan");
        let scripts = server.join("plugins").join("Skript").join("scripts");
        let script = scripts.join("a.sk");
        let disabled = scripts.join("-off.sk");
        touch(&script);
        touch(&disabled);
        let uri = Url::from_file_path(&script).unwrap();

        let mut state = State::default();
        assert_eq!(plan(&state, &uri).err(), Some(Skip::Off));

        state.rcon.settings.enabled = true;
        assert_eq!(plan(&state, &uri).err(), Some(Skip::NoPassword));

        state.rcon.settings.password = Some("hunter2".into());
        let job = plan(&state, &uri).expect("a job");
        assert_eq!(job.name, "a.sk");
        assert_eq!(job.target.address(), "127.0.0.1:25575");
        assert_eq!(job.target.timeout, Duration::from_millis(5000));

        let off = Url::from_file_path(&disabled).unwrap();
        assert_eq!(
            plan(&state, &off).err(),
            Some(Skip::DisabledScript("-off.sk".into()))
        );

        state.rcon.settings.scripts_root = Some(server.join("nowhere").display().to_string());
        assert!(matches!(plan(&state, &uri).err(), Some(Skip::Outside(..))));
    }

    #[test]
    fn rapid_saves_coalesce_into_one_more_run() {
        let mut rcon = RconState::default();
        assert!(rcon.begin("a"));
        assert!(!rcon.begin("a"));
        assert!(!rcon.begin("a"));
        assert!(rcon.begin("b"), "files are independent");
        // Two saves while running mean one more run, not two.
        assert!(rcon.finish("a"));
        assert!(!rcon.finish("a"));
        assert!(rcon.begin("a"), "free again");
    }

    /// The password never appears in a debug dump of the settings.
    #[test]
    fn debug_output_hides_the_password() {
        let settings = RconSettings {
            password: Some("hunter2".into()),
            ..Default::default()
        };
        assert!(!format!("{settings:?}").contains("hunter2"));
    }

    /// A Minecraft-like RCON server: one packet per read, replies split into
    /// `chunk`-byte packets. Returns the commands it ran.
    async fn fake_server(
        password: &'static str,
        reply: &'static str,
        chunk: usize,
    ) -> (u16, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut ran = Vec::new();
            while let Ok(packet) = read_packet(&mut socket).await {
                let body = String::from_utf8(packet.body).unwrap();
                match packet.kind {
                    AUTH if body == password => {
                        socket
                            .write_all(&encode(packet.id, AUTH_RESPONSE, ""))
                            .await
                            .unwrap();
                    }
                    AUTH => {
                        socket
                            .write_all(&encode(-1, AUTH_RESPONSE, ""))
                            .await
                            .unwrap();
                    }
                    _ => {
                        // Paper answers a blank command with an empty reply.
                        let text = if body.is_empty() { "" } else { reply };
                        let bytes = text.as_bytes();
                        let mut parts: Vec<&[u8]> = bytes.chunks(chunk).collect();
                        if parts.is_empty() {
                            parts.push(&[]);
                        }
                        for part in parts {
                            let mut packet = encode(packet.id, 0, "");
                            // Splice raw bytes in: a chunk may end mid-character.
                            packet.truncate(12);
                            packet.extend_from_slice(part);
                            packet.extend_from_slice(&[0, 0]);
                            let length = (packet.len() - 4) as i32;
                            packet[0..4].copy_from_slice(&length.to_le_bytes());
                            socket.write_all(&packet).await.unwrap();
                        }
                        ran.push(body);
                    }
                }
            }
            ran
        });
        (port, task)
    }

    fn target(port: u16, password: &str) -> Target {
        Target {
            host: "127.0.0.1".into(),
            port,
            password: password.into(),
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn reloads_through_a_fake_server_and_reassembles_split_replies() {
        // 7-byte packets split `§` (two bytes in UTF-8) more than once.
        let (port, server) = fake_server("secret", FAILED, 7).await;
        let report = reload(&target(port, "secret"), "broken.sk").await.unwrap();
        assert_eq!(report.verdict, Verdict::Errors(2));
        assert_eq!(report.findings.len(), 3);
        assert_eq!(
            report.findings[0].message,
            "Can't understand this condition/effect: frobnicate the player"
        );

        let ran = server.await.unwrap();
        assert_eq!(
            ran,
            vec!["skript reload broken.sk".to_string(), String::new()]
        );
    }

    #[tokio::test]
    async fn a_wrong_password_is_reported_as_such() {
        let (port, _server) = fake_server("secret", RELOADED, 4096).await;
        let error = reload(&target(port, "guess"), "x.sk").await.unwrap_err();
        assert_eq!(error, RconError::AuthRejected);
    }

    #[tokio::test]
    async fn a_closed_port_and_a_silent_server_fail_cleanly() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let error = run_command(&target(port, "x"), "list").await.unwrap_err();
        assert!(matches!(error, RconError::Unreachable(_)), "{error}");

        // Accepts, then never answers.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let _hold = tokio::spawn(async move {
            let accepted = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(accepted);
        });
        let quick = Target {
            timeout: Duration::from_millis(200),
            ..target(port, "x")
        };
        assert_eq!(run_command(&quick, "list").await, Err(RconError::Timeout));
    }
}
