//! `ssh <chat>@host`: a console that talks to a chat's `main.dog` from a
//! terminal, so a script can be tried without Telegram and without an audience.
//!
//! Every line is run as a message, exactly as the bot would run it — same
//! runtime, same files, same permissions, same `config/bot.json` — but the
//! reply, and anything the script `send`s, is printed instead of posted. Files
//! are the chat's real ones, so a script that writes, writes.
//!
//! Three ways in:
//!
//! * `ssh chat@host` gets an interactive prompt (with a pty).
//! * `ssh -T chat@host < lines.txt` runs each line of stdin as a message.
//! * `ssh chat@host 'some message'` runs that one message and exits with
//!   status 0 if the script succeeded, 1 if it failed and 2 if there was
//!   nothing to run, so it can be scripted.

use crate::messages::{self, Courier, Run, ScriptContext};
use crate::prelude::*;
use russh::server::{Handle, Msg};
use russh::{Channel, ChannelId, ChannelMsg};
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use teloxide::prelude::*;

/// Who is at the console, and which chat they are talking to.
#[derive(Clone)]
pub struct Console {
    pub pool: PgPool,
    pub user_id: UserId,
    pub chat_id: ChatId,
    pub out: Out,
}

/// Where the console writes. Shared with the script's blocking thread, so it
/// goes through the session's `Handle` rather than the channel itself.
#[derive(Clone)]
pub struct Out {
    pub handle: Handle,
    pub id: ChannelId,
    /// A terminal wants `\r\n`; a pipe wants the bytes as they are.
    pub pty: bool,
}

impl Out {
    fn encode(&self, text: &str) -> String {
        if self.pty {
            text.replace('\n', "\r\n")
        } else {
            text.to_owned()
        }
    }

    pub async fn print(&self, text: &str) {
        // A client that has gone away is not worth failing over.
        let _ = self.handle.data(self.id, self.encode(text)).await;
    }

    /// Errors go to stderr when output is piped, so `2>/dev/null` and exit
    /// codes behave the way they would for any other command.
    pub async fn eprint(&self, text: &str) {
        if self.pty {
            self.print(text).await;
        } else {
            let _ = self.handle.extended_data(self.id, 1, self.encode(text)).await;
        }
    }

    pub async fn finish(&self, status: u32) {
        let _ = self.handle.exit_status_request(self.id, status).await;
        let _ = self.handle.eof(self.id).await;
        let _ = self.handle.close(self.id).await;
    }
}

/// What became of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// The script ran and failed, or a console command was misused.
    Failed,
    /// There was nothing to run: no `main.dog`, or one this user can't run.
    Missing,
    /// `:quit`.
    Quit,
}

impl Status {
    fn exit_code(self) -> u32 {
        match self {
            Status::Ok | Status::Quit => 0,
            Status::Failed => 1,
            Status::Missing => 2,
        }
    }
}

const HELP: &str = "\
Each line you type is sent to main.dog as a message from you, and the reply is
printed here. Nothing reaches Telegram. Files are the chat's real files, and
`history` shows the real chat's messages.

  :help          this text
  :eval <code>   run one line of dog4 here instead of main.dog
  :quit          leave (Ctrl-D works too)

Messages a script sends while it runs show up as [#id], and edits and deletes
of them as [#id edited] and [#id deleted].
";

impl Console {
    /// Runs one line: a console command if it starts with `:`, otherwise a
    /// message for `main.dog`.
    pub async fn submit(&self, line: &str) -> Status {
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(command) = line.strip_prefix(':') {
            let (name, rest) = command.split_once(' ').unwrap_or((command, ""));
            return match name {
                "help" | "h" | "?" => {
                    self.out.print(HELP).await;
                    Status::Ok
                }
                "quit" | "q" | "exit" => Status::Quit,
                "eval" | "e" if !rest.trim().is_empty() => self.eval(rest).await,
                "eval" | "e" => {
                    self.out.eprint("usage: :eval <dog4 code>\n").await;
                    Status::Failed
                }
                _ => {
                    self.out
                        .eprint(&format!("unknown console command :{name}; :help lists them\n"))
                        .await;
                    Status::Failed
                }
            };
        }
        self.message(line).await
    }

    /// Runs `main.dog` for `text`, as if it had been sent in the chat.
    async fn message(&self, text: &str) -> Status {
        let loaded = self.with_connection(messages::load_main);
        let (source, config) = match loaded {
            Ok(Ok(found)) => found,
            Ok(Err(err)) => {
                let why = messages::explain_missing(&err)
                    .map(|html| strip_html(&html))
                    .unwrap_or_else(|| "the bot could not read main.dog".to_string());
                self.out.eprint(&format!("{why}\n")).await;
                return Status::Missing;
            }
            Err(()) => return Status::Failed,
        };
        self.run(source, config, text.to_string(), true).await
    }

    /// Runs `code` with the chat's modules and settings, without `main.dog`.
    async fn eval(&self, code: &str) -> Status {
        let Ok(config) = self.with_connection(messages::load_config)
        else {
            return Status::Failed;
        };
        self.run(code.to_string(), config, String::new(), false).await
    }

    async fn run(&self, source: String, config: messages::Config, message: String, defer: bool) -> Status {
        let sender = {
            let user = self.with_connection(|cn, _, user| model::User::get(cn, user).ok());
            match user {
                Ok(Some(user)) => match user.last_name {
                    Some(last) => format!("{} {last}", user.first_name),
                    None => user.first_name,
                },
                _ => String::new(),
            }
        };
        let ctx = ScriptContext {
            message,
            sender,
            ..Default::default()
        };
        let courier: Arc<dyn Courier> = Arc::new(ConsoleCourier::new(
            self.out.clone(),
            tokio::runtime::Handle::current(),
        ));

        let (pool, chat, user) = (self.pool.clone(), self.chat_id, self.user_id);
        let result = {
            let (config, ctx, courier) = (config.clone(), ctx.clone(), courier.clone());
            tokio::task::spawn_blocking(move || {
                messages::run_message(pool, chat, user, &source, &config, &ctx, courier)
            })
            .await
        };

        match result {
            Ok(Run { output, later, .. }) => {
                if defer {
                    // Deferred scripts print here too, if the console is still
                    // open when they get to it.
                    messages::run_later(courier, self.pool.clone(), chat, user, later, config, ctx);
                }
                match output {
                    Ok(reply) => {
                        if !reply.is_empty() {
                            self.out.print(&with_newline(reply)).await;
                        }
                        Status::Ok
                    }
                    Err(err) => {
                        self.out.eprint(&with_newline(err)).await;
                        Status::Failed
                    }
                }
            }
            Err(err) => {
                log::error!("console script task failed: {err}");
                self.out.eprint("the bot failed to run the script\n").await;
                Status::Failed
            }
        }
    }

    /// Runs `f` with a database connection, reporting a pool failure to the
    /// console.
    fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut PgConnection, ChatId, UserId) -> T,
    ) -> Result<T, ()> {
        match self.pool.get() {
            Ok(mut cn) => Ok(f(&mut cn, self.chat_id, self.user_id)),
            Err(err) => {
                log::error!("console: no database connection: {err}");
                let out = self.out.clone();
                tokio::spawn(async move { out.eprint("the bot's database is unavailable\n").await });
                Err(())
            }
        }
    }

    /// The welcome printed when an interactive console opens.
    pub fn banner(&self) -> String {
        let (chat, who) = self
            .with_connection(|cn, chat_id, user_id| {
                let chat = model::Chat::find(cn, ChatAddress::Id(chat_id))
                    .ok()
                    .flatten()
                    .map(|c| c.alias.clone().unwrap_or_else(|| c.to_string()))
                    .unwrap_or_else(|| chat_id.to_string());
                let who = model::User::get(cn, user_id)
                    .map(|u| u.first_name)
                    .unwrap_or_default();
                (chat, who)
            })
            .unwrap_or_default();
        format!(
            "dogbot console for {chat}. Hi {who}!\n\
             Type a message and main.dog answers it here; nothing is posted to Telegram.\n\
             :help for more, Ctrl-D to leave.\n\n"
        )
    }

    /// The prompt shown before each line.
    pub fn prompt(&self) -> String {
        let chat = self
            .with_connection(|cn, chat_id, _| {
                model::Chat::find(cn, ChatAddress::Id(chat_id))
                    .ok()
                    .flatten()
                    .and_then(|c| c.alias)
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| "dogbot".to_string());
        format!("{chat}> ")
    }
}

fn with_newline(mut text: String) -> String {
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// The text of the bot's HTML explanations, for a terminal.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// Prints what a script sends instead of posting it.
struct ConsoleCourier {
    out: Out,
    rt: tokio::runtime::Handle,
    next: AtomicI32,
    /// What this console has "sent", so `edit` and `delete` refuse anything
    /// else, as they would refuse a message that isn't the bot's.
    sent: Mutex<HashSet<i32>>,
}

impl ConsoleCourier {
    fn new(out: Out, rt: tokio::runtime::Handle) -> Self {
        ConsoleCourier {
            out,
            rt,
            next: AtomicI32::new(1),
            sent: Mutex::new(HashSet::new()),
        }
    }

    fn show(&self, label: String, text: Option<Formatted>) {
        let body = match text {
            None => String::new(),
            Some(Formatted::Raw(t)) | Some(Formatted::Code(t)) => format!(" {t}"),
            Some(Formatted::Html(t)) => format!(" (html) {t}"),
            Some(Formatted::Markdown(t)) => format!(" (markdown) {t}"),
        };
        let line = with_newline(format!("[{label}]{body}"));
        let out = self.out.clone();
        self.rt.block_on(async move { out.print(&line).await });
    }

    fn known(&self, id: i32) -> Result<(), String> {
        if self.sent.lock().unwrap().contains(&id) {
            Ok(())
        } else {
            Err(format!(
                "message {id} was not sent from this console; the console never touches Telegram"
            ))
        }
    }
}

impl Courier for ConsoleCourier {
    fn send(&self, text: Formatted, _how: &messages::courier::Outgoing, _within: Option<Duration>) -> Result<i32, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.sent.lock().unwrap().insert(id);
        self.show(format!("#{id}"), Some(text));
        Ok(id)
    }

    fn edit(&self, id: i32, text: Formatted, _within: Option<Duration>) -> Result<(), String> {
        self.known(id)?;
        self.show(format!("#{id} edited"), Some(text));
        Ok(())
    }

    fn delete(&self, id: i32, _within: Option<Duration>) -> Result<(), String> {
        self.known(id)?;
        self.sent.lock().unwrap().remove(&id);
        self.show(format!("#{id} deleted"), None);
        Ok(())
    }
}

/// What the console does with a request to run a command (`ssh host cmd`).
/// Tools that expect a Unix shell at the other end get told what to use
/// instead of having their protocol chatter run as a message.
pub fn foreign_command(command: &str) -> Option<&'static str> {
    let program = command.split_whitespace().next().unwrap_or("");
    match program {
        "scp" => Some(
            "this server has no shell, so scp's old protocol can't work here.\n\
             Use `scp -s` (or any scp from OpenSSH 9 or later, which uses SFTP), `sftp`, or sshfs.\n",
        ),
        "rsync" => Some(
            "rsync needs to run a copy of itself on the server, and this server has no shell.\n\
             Mount the chat with sshfs and rsync into the mount instead (/mount shows how).\n",
        ),
        "git-upload-pack" | "git-receive-pack" | "git-upload-archive" => Some(
            "git can't push to or pull from this server. Keep the repository locally and copy\n\
             files into an sshfs mount of the chat.\n",
        ),
        _ => None,
    }
}

/// Runs `ssh chat@host 'message'`: one line, then the exit status.
pub async fn exec(console: Console, command: String) {
    if let Some(advice) = foreign_command(&command) {
        console.out.eprint(&format!("dogbot: {advice}")).await;
        console.out.finish(127).await;
        return;
    }
    let status = console.submit(&command).await;
    console.out.finish(status.exit_code()).await;
}

/// Runs a shell session: an interactive prompt on a pty, or one message per
/// line of stdin otherwise.
pub async fn shell(console: Console, mut channel: Channel<Msg>) {
    let pty = console.out.pty;
    let mut editor = LineEditor::new(pty);
    let mut worst = Status::Ok;
    let prompt = console.prompt();

    if pty {
        console.out.print(&console.banner()).await;
        console.out.print(&prompt).await;
    }

    'session: loop {
        let actions = match channel.wait().await {
            Some(ChannelMsg::Data { data }) => editor.feed(&data),
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                let mut rest = editor.finish();
                rest.push(Action::Quit);
                rest
            }
            Some(_) => continue,
        };
        for action in actions {
            match action {
                Action::Echo(text) => console.out.print(&text).await,
                Action::Submit(line) => {
                    let status = if line.trim().is_empty() {
                        Status::Ok
                    } else {
                        console.submit(&line).await
                    };
                    if status == Status::Quit {
                        break 'session;
                    }
                    if status.exit_code() > worst.exit_code() {
                        worst = status;
                    }
                    if pty {
                        console.out.print(&prompt).await;
                    }
                }
                Action::Redraw(line) => {
                    console.out.print(&format!("\r\x1b[K{prompt}{line}")).await;
                }
                Action::Quit => break 'session,
            }
        }
    }

    if pty {
        console.out.print("\n").await;
    }
    // Piped, the status says whether every message went through, so a file of
    // test messages can fail a build.
    console.out.finish(if pty { 0 } else { worst.exit_code() }).await;
}

/// What the line editor wants done.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Write this to the terminal.
    Echo(String),
    /// A complete line.
    Submit(String),
    /// Replace the current line on screen with this one (history).
    Redraw(String),
    Quit,
}

/// Just enough of a terminal line discipline for a chat prompt: typing,
/// backspace, Ctrl-C, Ctrl-U, Ctrl-D and up/down through earlier lines. Without
/// a pty the client has already done all of that, so lines pass straight
/// through.
pub struct LineEditor {
    pty: bool,
    line: String,
    /// The start of a UTF-8 character split across two packets.
    partial: Vec<u8>,
    escape: Escape,
    /// `\r\n` from a terminal is one Enter, not two.
    after_cr: bool,
    history: Vec<String>,
    /// Position while browsing `history`; `None` when editing a fresh line.
    browsing: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    None,
    /// Saw ESC.
    Start,
    /// Inside `ESC [` or `ESC O`, collecting until the final byte.
    Sequence,
}

impl LineEditor {
    pub fn new(pty: bool) -> Self {
        LineEditor {
            pty,
            line: String::new(),
            partial: Vec::new(),
            escape: Escape::None,
            after_cr: false,
            history: Vec::new(),
            browsing: None,
        }
    }

    /// The line still being typed when input ends, if any.
    pub fn finish(&mut self) -> Vec<Action> {
        let line = std::mem::take(&mut self.line);
        // On a terminal an unfinished line was never sent; through a pipe, a
        // file without a final newline still means its last line.
        if !self.pty && !line.is_empty() {
            vec![Action::Submit(line)]
        } else {
            Vec::new()
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut buf = std::mem::take(&mut self.partial);
        buf.extend_from_slice(bytes);

        let mut i = 0;
        while i < buf.len() {
            let b = buf[i];

            if !self.pty {
                match b {
                    b'\n' => actions.push(Action::Submit(
                        std::mem::take(&mut self.line).trim_end_matches('\r').to_string(),
                    )),
                    _ => match decode_char(&buf[i..]) {
                        Decoded::Char(c, len) => {
                            self.line.push(c);
                            i += len;
                            continue;
                        }
                        Decoded::Incomplete => {
                            self.partial = buf[i..].to_vec();
                            break;
                        }
                        Decoded::Invalid => self.line.push(char::REPLACEMENT_CHARACTER),
                    },
                }
                i += 1;
                continue;
            }

            match self.escape {
                Escape::Start => {
                    self.escape = if b == b'[' || b == b'O' {
                        Escape::Sequence
                    } else {
                        Escape::None
                    };
                    i += 1;
                    continue;
                }
                Escape::Sequence => {
                    if (0x40..=0x7e).contains(&b) {
                        self.escape = Escape::None;
                        match b {
                            b'A' => actions.extend(self.browse(-1)),
                            b'B' => actions.extend(self.browse(1)),
                            _ => {}
                        }
                    }
                    i += 1;
                    continue;
                }
                Escape::None => {}
            }

            let after_cr = std::mem::replace(&mut self.after_cr, false);
            match b {
                0x1b => self.escape = Escape::Start,
                b'\r' | b'\n' => {
                    if b == b'\n' && after_cr {
                        i += 1;
                        continue;
                    }
                    self.after_cr = b == b'\r';
                    let line = std::mem::take(&mut self.line);
                    self.browsing = None;
                    if !line.trim().is_empty() && self.history.last() != Some(&line) {
                        self.history.push(line.clone());
                    }
                    actions.push(Action::Echo("\n".to_string()));
                    actions.push(Action::Submit(line));
                }
                0x7f | 0x08 => {
                    if self.line.pop().is_some() {
                        actions.push(Action::Echo("\x08 \x08".to_string()));
                    }
                }
                0x03 => {
                    self.line.clear();
                    self.browsing = None;
                    actions.push(Action::Echo("^C\n".to_string()));
                    actions.push(Action::Submit(String::new()));
                }
                0x04 => {
                    if self.line.is_empty() {
                        actions.push(Action::Quit);
                        return actions;
                    }
                }
                0x15 => {
                    self.line.clear();
                    actions.push(Action::Redraw(String::new()));
                }
                b if b < 0x20 => {}
                _ => match decode_char(&buf[i..]) {
                    Decoded::Char(c, len) => {
                        self.line.push(c);
                        actions.push(Action::Echo(c.to_string()));
                        i += len;
                        continue;
                    }
                    Decoded::Incomplete => {
                        self.partial = buf[i..].to_vec();
                        break;
                    }
                    Decoded::Invalid => {}
                },
            }
            i += 1;
        }
        actions
    }

    /// Moves through earlier lines: `-1` is back (up), `1` forward (down).
    fn browse(&mut self, step: isize) -> Option<Action> {
        if self.history.is_empty() {
            return None;
        }
        let len = self.history.len();
        let next = match (self.browsing, step) {
            (None, -1) => Some(len - 1),
            (None, _) => return None,
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), _) if i + 1 < len => Some(i + 1),
            (Some(_), _) => None,
        };
        self.browsing = next;
        self.line = next.map(|i| self.history[i].clone()).unwrap_or_default();
        Some(Action::Redraw(self.line.clone()))
    }
}

enum Decoded {
    Char(char, usize),
    Incomplete,
    Invalid,
}

/// Decodes the UTF-8 character at the start of `bytes`.
fn decode_char(bytes: &[u8]) -> Decoded {
    let len = match bytes[0] {
        b if b < 0x80 => 1,
        b if b & 0xe0 == 0xc0 => 2,
        b if b & 0xf0 == 0xe0 => 3,
        b if b & 0xf8 == 0xf0 => 4,
        _ => return Decoded::Invalid,
    };
    if bytes.len() < len {
        return Decoded::Incomplete;
    }
    match std::str::from_utf8(&bytes[..len]) {
        Ok(s) => Decoded::Char(s.chars().next().unwrap(), len),
        Err(_) => Decoded::Invalid,
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    fn submitted(actions: &[Action]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::Submit(line) => Some(line.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_pipe_gives_one_message_per_line() {
        let mut e = LineEditor::new(false);
        let mut actions = e.feed(b"roll 2d6\r\nhel");
        actions.extend(e.feed(b"lo\n"));
        actions.extend(e.feed(b"no newline at the end"));
        actions.extend(e.finish());
        assert_eq!(
            submitted(&actions),
            ["roll 2d6", "hello", "no newline at the end"]
        );
        assert!(!actions.iter().any(|a| matches!(a, Action::Echo(_))));
    }

    #[test]
    fn a_terminal_echoes_and_edits() {
        let mut e = LineEditor::new(true);
        let actions = e.feed(b"pinx\x7fg\r");
        assert_eq!(submitted(&actions), ["ping"]);
        assert!(actions.contains(&Action::Echo("\x08 \x08".into())));
        // `\r\n` is a single Enter.
        let actions = e.feed(b"a\r\nb\r");
        assert_eq!(submitted(&actions), ["a", "b"]);
    }

    #[test]
    fn utf8_split_across_packets_survives() {
        let mut e = LineEditor::new(true);
        let bytes = "olá\r".as_bytes();
        let (a, b) = bytes.split_at(3); // splits the two bytes of `á`
        let mut actions = e.feed(a);
        actions.extend(e.feed(b));
        assert_eq!(submitted(&actions), ["olá"]);
    }

    #[test]
    fn control_keys() {
        let mut e = LineEditor::new(true);
        // Ctrl-C abandons the line.
        assert_eq!(submitted(&e.feed(b"oops\x03")), [""]);
        // Ctrl-D only quits on an empty line.
        assert!(!e.feed(b"x\x04").contains(&Action::Quit));
        assert!(e.feed(b"\x15\x04").contains(&Action::Quit));
    }

    #[test]
    fn arrows_walk_the_history_and_are_never_typed() {
        let mut e = LineEditor::new(true);
        e.feed(b"first\rsecond\r");
        assert_eq!(e.feed(b"\x1b[A"), [Action::Redraw("second".into())]);
        assert_eq!(e.feed(b"\x1b[A"), [Action::Redraw("first".into())]);
        assert_eq!(e.feed(b"\x1b[B"), [Action::Redraw("second".into())]);
        // Left and right arrows are swallowed rather than inserted as text.
        e.feed(b"\x1b[D\x1b[C");
        assert_eq!(submitted(&e.feed(b"\r")), ["second"]);
    }

    #[test]
    fn tools_that_need_a_shell_are_turned_away() {
        assert!(foreign_command("scp -t /x").is_some());
        assert!(foreign_command("rsync --server -vlogDtpre.iLsfxC . /x").is_some());
        assert!(foreign_command("roll 2d6").is_none());
    }
}
