//! The `bot` native module: everything a script can do beyond pure computation.
//!
//! Scripts reach these with `use read, write from mod bot;`. Unlike dog3's
//! string-plus-status-code convention, failures raise a catchable dog4 error
//! and results are real values — `ls` returns an array, `history` an array of
//! maps.

use crate::prelude::*;
use dog4::{Error, Library, Map, Value};
use log::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teloxide::prelude::*;

use super::courier::Courier;

/// Scripts queued by `later`, to run once the reply has been sent.
pub type Later = Rc<RefCell<Vec<String>>>;

/// How many messages one run may send or edit. Telegram throttles a chat to
/// roughly a message a second, so this is far more than a script can usefully
/// pace anyway; it exists so a loop can't turn the bot into a flood.
pub const MAX_MESSAGES: u32 = 40;

/// How often `sleep` looks up from its nap to check the run's time limit.
const SLEEP_SLICE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct State {
    pub pool: PgPool,
    pub chat_id: ChatId,
    pub user_id: UserId,
    /// `None` in a run that is itself deferred, so a background script can't
    /// queue another one and chain forever.
    pub later: Option<Later>,
    /// Where `send` and `edit` deliver. `None` where there is no chat to talk to.
    pub courier: Option<Arc<dyn Courier>>,
    /// Sends and edits left in this run.
    pub budget: Rc<Cell<u32>>,
}

impl State {
    /// Borrows a connection, turning pool exhaustion into a script-visible error.
    fn connect(&self) -> Result<diesel::r2d2::PooledConnection<
        diesel::r2d2::ConnectionManager<diesel::PgConnection>,
    >, Error> {
        self.pool.get().map_err(|err| {
            error!("bot module: no database connection: {err}");
            Error::value_error("the database is unavailable")
        })
    }
}

/// Reads the optional `mode` argument of `send` and `edit`.
fn formatted(text: &str, mode: Option<&str>) -> Result<Formatted, Error> {
    if text.is_empty() {
        return Err(Error::value_error("cannot send an empty message"));
    }
    match mode.unwrap_or("plain") {
        "plain" | "text" => Ok(raw(text)),
        "markdown" | "md" => Ok(markdown(text)),
        "html" => Ok(html(text)),
        "code" => Ok(code(text)),
        other => Err(Error::value_error(format!(
            "unknown format {other:?}; use plain, markdown, html or code"
        ))),
    }
}

impl State {
    /// The courier, after spending one message from the run's budget.
    fn courier(&self) -> Result<&Arc<dyn Courier>, Error> {
        let courier = self
            .courier
            .as_ref()
            .ok_or_else(|| Error::value_error("this script cannot send messages"))?;
        let left = self.budget.get();
        if left == 0 {
            return Err(Error::value_error(format!(
                "a script may send or edit at most {MAX_MESSAGES} messages per run"
            )));
        }
        self.budget.set(left - 1);
        Ok(courier)
    }
}

/// Turns a filesystem error into a dog4 error. Database faults are logged and
/// reported generically; everything else is the user's own mistake.
fn fs_error(err: files::FileError) -> Error {
    match err {
        files::FileError::QueryError(err) => {
            error!("bot module: database error: {err}");
            Error::value_error("the database is unavailable")
        }
        err => Error::value_error(err.to_string()),
    }
}

pub fn build(state: State) -> Value {
    let mut lib = Library::new();

    let s = state.clone();
    lib.def("read", "path: str", move |vm, args| {
        let mut cn = s.connect()?;
        let path = args[0].to_string();
        let (_, bytes) = files::read(&mut cn, s.chat_id, s.user_id, &path).map_err(fs_error)?;
        vm.meter().check_now()?;
        Ok(Value::str(String::from_utf8_lossy(&bytes).as_ref()))
    });

    let s = state.clone();
    lib.def("write", "path: str, content: str", move |vm, args| {
        let mut cn = s.connect()?;
        let (path, content) = (args[0].to_string(), args[1].to_string());
        files::write(&mut cn, s.chat_id, s.user_id, &path, content.as_bytes())
            .map_err(fs_error)?;
        vm.meter().check_now()?;
        Ok(Value::Null)
    });

    let s = state.clone();
    lib.def("exists", "path: str", move |vm, args| {
        let mut cn = s.connect()?;
        let path = args[0].to_string();
        let found = match files::stat(&mut cn, s.chat_id, s.user_id, &path) {
            Ok(_) => true,
            Err(files::FileError::FileNotFound(_)) => false,
            Err(err) => return Err(fs_error(err)),
        };
        vm.meter().check_now()?;
        Ok(Value::Bool(found))
    });

    // dog4 arity is fixed (`t?` is a type, not an optional slot), so the
    // variants are registered as overloads.
    for arity in 1..=3 {
        let s = state.clone();
        let params = match arity {
            1 => "path: str",
            2 => "path: str, include_files: bool",
            _ => "path: str, include_files: bool, include_dirs: bool",
        };
        lib.def("ls", params, move |vm, args| {
            let mut cn = s.connect()?;
            let path = args[0].to_string();
            let include_files = args.get(1).is_none_or(|v| v.truthy());
            let include_dirs = args.get(2).is_none_or(|v| v.truthy());

            let listing =
                files::list(&mut cn, s.chat_id, s.user_id, &path, false).map_err(fs_error)?;
            vm.meter().check_now()?;

            let names = listing
                .into_iter()
                .filter(|f| (include_files && f.is_file()) || (include_dirs && f.is_dir()))
                .map(|f| Value::str(&f.name))
                .collect::<Vec<_>>();
            Ok(Value::arr(names))
        });
    }

    for arity in 0..=3 {
        let s = state.clone();
        let params = match arity {
            0 => "",
            1 => "count: num",
            2 => "count: num, offset: num",
            _ => "count: num, offset: num, with_bot: bool",
        };
        lib.def("history", params, move |vm, args| {
            let mut cn = s.connect()?;
            let count = args.first().and_then(|v| v.as_int()).unwrap_or(10);
            let offset = args.get(1).and_then(|v| v.as_int()).unwrap_or(0);
            let with_bot = args.get(2).is_some_and(|v| v.truthy());

            let messages =
                model::Message::list(&mut cn, s.chat_id, count, offset).map_err(|err| {
                    error!("bot module: history failed: {err}");
                    Error::value_error("the database is unavailable")
                })?;
            vm.meter().check_now()?;

            let rows = messages
                .into_iter()
                .filter(|(_, user)| with_bot || user.is_some())
                .map(|(message, user)| {
                    let name = match &user {
                        Some(user) => match &user.last_name {
                            Some(last) => format!("{} {}", user.first_name, last),
                            None => user.first_name.clone(),
                        },
                        None => "<bot>".to_string(),
                    };
                    let mut row = Map::default();
                    row.insert("user".into(), Value::str(&name));
                    row.insert("content".into(), Value::str(&message.content));
                    row.insert("is_bot".into(), Value::Bool(user.is_none()));
                    Value::map(row)
                })
                .collect::<Vec<_>>();
            Ok(Value::arr(rows))
        });
    }

    // Queues a script to run after this message has been answered, so slow
    // upkeep (summarising, reindexing) doesn't hold up the reply. The script is
    // checked now, so a typo fails here rather than silently in the background.
    let s = state.clone();
    lib.def("later", "path: str", move |vm, args| {
        let Some(later) = &s.later else {
            return Err(Error::value_error(
                "later is not available in a deferred run",
            ));
        };
        let path = args[0].to_string();
        let mut cn = s.connect()?;
        files::read_for_execution(&mut cn, s.chat_id, s.user_id, &path).map_err(fs_error)?;
        vm.meter().check_now()?;
        let mut queue = later.borrow_mut();
        if !queue.contains(&path) {
            queue.push(path);
        }
        Ok(Value::Null)
    });

    // Sends a message right away, while the script is still running. Overloaded
    // for the format. `send` returns nothing, because a command's value is
    // output in dog4 and a stray message id would end up in the reply; `post`
    // is the same call returning the id, for a message that will be `edit`ed.
    for (name, returns_id) in [("send", false), ("post", true)] {
        for arity in 1..=2 {
            let s = state.clone();
            let params = if arity == 1 {
                "text: str"
            } else {
                "text: str, mode: str"
            };
            lib.def(name, params, move |vm, args| {
                let mode = args.get(1).map(|m| m.to_string());
                let text = formatted(&args[0].to_string(), mode.as_deref())?;
                let id = s
                    .courier()?
                    .send(text, vm.meter().remaining())
                    .map_err(|err| Error::value_error(format!("could not send: {err}")))?;
                vm.meter().check_now()?;
                Ok(if returns_id {
                    Value::Int(id as i64)
                } else {
                    Value::Null
                })
            });
        }
    }

    // Rewrites a message `post` returned the id of: the way to show progress.
    for arity in 2..=3 {
        let s = state.clone();
        let params = if arity == 2 {
            "id: num, text: str"
        } else {
            "id: num, text: str, mode: str"
        };
        lib.def("edit", params, move |vm, args| {
            let id = args[0]
                .as_int()
                .and_then(|id| i32::try_from(id).ok())
                .ok_or_else(|| Error::value_error("edit needs a message id from post"))?;
            let mode = args.get(2).map(|m| m.to_string());
            let text = formatted(&args[1].to_string(), mode.as_deref())?;
            s.courier()?
                .edit(id, text, vm.meter().remaining())
                .map_err(|err| Error::value_error(format!("could not edit: {err}")))?;
            vm.meter().check_now()?;
            Ok(Value::Null)
        });
    }

    // Pauses the script, so messages can be paced. Counts against the run's
    // time limit like everything else, and fails with the usual timeout error
    // instead of sleeping past it.
    lib.def("sleep", "seconds: num", |vm, args| {
        let seconds = args[0].as_f64().filter(|s| s.is_finite() && *s >= 0.0);
        let Some(seconds) = seconds else {
            return Err(Error::value_error("sleep needs a number of seconds, zero or more"));
        };
        // Saturate rather than panic on an absurd request; the deadline decides.
        let wake = Instant::now()
            .checked_add(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX))
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(60 * 60 * 24));
        loop {
            vm.meter().check_now()?;
            let left = wake.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Value::Null);
            }
            let nap = left.min(SLEEP_SLICE);
            // Never sleep past the deadline; the next check reports it.
            let nap = vm.meter().remaining().map_or(nap, |r| nap.min(r));
            std::thread::sleep(nap);
        }
    });

    lib.def("version", "", |_, _| {
        Ok(Value::str(option_env!("APP_VERSION").unwrap_or("dev")))
    });

    lib.into_value()
}
