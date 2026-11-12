//! End-to-end script execution: real dog4 runtime, real module provider, real
//! database. Needs `TEST_DATABASE_URL`.

use super::*;
use std::sync::Once;

const USER: UserId = UserId(80001);

struct Harness {
    pool: PgPool,
    chat: ChatId,
}

impl Harness {
    fn write(&self, path: &str, body: &str) {
        let mut cn = self.pool.get().unwrap();
        // `files::write` does not create parents, so make them first.
        if let Some((dir, _)) = path.rsplit_once('/') {
            files::mkdir(&mut cn, self.chat, USER, dir, true).unwrap();
        }
        files::write(&mut cn, self.chat, USER, path, body.as_bytes()).unwrap();
    }

    /// Marks a file executable, which modules and `main.dog` both require.
    fn make_executable(&self, path: &str) {
        let mut cn = self.pool.get().unwrap();
        let clause: files::perm::Clause = "u+x".parse().unwrap();
        files::set_permission(&mut cn, self.chat, USER, path, clause).unwrap();
    }

    fn run(&self, source: &str) -> Result<String, String> {
        self.run_as(source, "hello world", "", "")
    }

    /// Runs `source` with a chosen message and reply, for scripts that read the
    /// `message`, `reply` and `reply_from` globals.
    fn run_as(
        &self,
        source: &str,
        message: &str,
        reply: &str,
        reply_from: &str,
    ) -> Result<String, String> {
        run_script(
            self.pool.clone(),
            self.chat,
            USER,
            source,
            &Config::default(),
            &ScriptContext {
                message: message.to_string(),
                sender: "Tester".to_string(),
                reply: reply.to_string(),
                reply_from: reply_from.to_string(),
            },
            Hooks::default(),
        )
    }

    /// Runs `source` as a message would, returning what it queued with `later`.
    fn run_queueing(&self, source: &str) -> (Result<String, String>, Vec<String>) {
        let later = runtime::Later::default();
        let out = run_script(
            self.pool.clone(),
            self.chat,
            USER,
            source,
            &Config::default(),
            &ScriptContext {
                message: String::new(),
                sender: "Tester".to_string(),
                reply: String::new(),
                reply_from: String::new(),
            },
            Hooks {
                later: Some(later.clone()),
                courier: None,
            },
        );
        (out, later.take())
    }
}

fn setup() -> Option<Harness> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    // Each test builds its own pool, so keep them small: the default of 10
    // times the number of parallel tests exhausts Postgres's connection limit.
    let pool: PgPool = diesel::r2d2::Pool::builder()
        .max_size(2)
        .build(diesel::r2d2::ConnectionManager::new(url.clone()))
        .expect("pool");

    crate::run_migrations_once(&mut pool.get().unwrap());

    static NEXT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
    static BASE: Once = Once::new();
    BASE.call_once(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        NEXT.store(
            -(nanos % 1_000_000_000_000) - 700_000_000_000,
            std::sync::atomic::Ordering::SeqCst,
        );
    });
    let chat = ChatId(NEXT.fetch_sub(1, std::sync::atomic::Ordering::SeqCst));

    let mut cn = pool.get().unwrap();
    model::Chat::upsert(&mut cn, chat, Some("script test"), true).unwrap();
    model::User::upsert(&mut cn, USER, "Tester", None, None).unwrap();
    model::UserInChat::upsert(&mut cn, USER, chat).unwrap();
    drop(cn);

    Some(Harness { pool, chat })
}

macro_rules! script_test {
    ($h:ident) => {
        let Some($h) = setup() else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
    };
}

#[test]
fn a_plain_script_produces_its_output() {
    script_test!(h);
    assert_eq!(h.run("pln hello").unwrap(), "hello\n");
}

#[test]
fn context_arrives_as_globals() {
    script_test!(h);
    // The dog3 `main(...)` convention is gone; context is injected directly.
    assert_eq!(h.run("put $message").unwrap(), "hello world");
    assert_eq!(h.run("put $sender").unwrap(), "Tester");
}

#[test]
fn a_chat_local_module_is_importable() {
    script_test!(h);
    h.write(
        "lib/greet.dog",
        "pub fn hi (name: str) { put \"hi $name\" }",
    );
    h.make_executable("lib/greet.dog");

    assert_eq!(
        h.run("use hi from mod lib/greet; hi Bob").unwrap(),
        "hi Bob"
    );
}

#[test]
fn a_module_without_execute_permission_is_refused() {
    script_test!(h);
    // Written but never chmod +x: importing is running code, so it must fail.
    h.write("lib/secret.dog", "pub fn hi () { put nope }");

    let err = h.run("use mod lib/secret").unwrap_err();
    assert!(
        err.contains("permission denied") || err.contains("secret"),
        "unexpected error: {err}"
    );
}

#[test]
fn relative_imports_resolve_against_the_importer() {
    script_test!(h);
    h.write("lib/a.dog", "pub fn a () { put from_a }");
    h.make_executable("lib/a.dog");
    h.write("lib/b.dog", "use a from mod ./a; pub fn b () { a }");
    h.make_executable("lib/b.dog");

    assert_eq!(h.run("use b from mod lib/b; b").unwrap(), "from_a");
}

#[test]
fn the_system_library_is_available() {
    script_test!(h);
    // syslib/ is compiled into the binary, so no filesystem setup is needed.
    let out = h
        .run("use truncate from mod sys/text; truncate \"abcdefghij\" 5")
        .unwrap();
    assert_eq!(out, "abcd\u{2026}");
}

#[test]
fn the_compat_shim_covers_dog3_json_helpers() {
    script_test!(h);
    let out = h
        .run("use jkeys, jstr_parse from mod sys/compat; put { jkeys { jstr_parse '{\"a\":1,\"b\":2}' } }")
        .unwrap();
    assert!(out.contains('a') && out.contains('b'), "got {out:?}");
}

#[test]
fn a_missing_module_is_reported_clearly() {
    script_test!(h);
    let err = h.run("use mod lib/nope").unwrap_err();
    assert!(err.contains("nope"), "unexpected error: {err}");
}

#[test]
fn a_circular_import_is_an_error_not_a_hang() {
    script_test!(h);
    h.write("a.dog", "use mod ./b; pub fn a () { put a }");
    h.make_executable("a.dog");
    h.write("b.dog", "use mod ./a; pub fn b () { put b }");
    h.make_executable("b.dog");

    let err = h.run("use mod a").unwrap_err();
    assert!(err.contains("circular"), "unexpected error: {err}");
}

#[test]
fn the_bot_module_reads_and_writes_the_filesystem() {
    script_test!(h);
    let out = h
        .run("use read, write from mod bot; write notes.txt \"from a script\"; put { read notes.txt }")
        .unwrap();
    assert_eq!(out, "from a script");

    // And the write is really in the filesystem.
    let mut cn = h.pool.get().unwrap();
    let (_, bytes) = files::read(&mut cn, h.chat, USER, "notes.txt").unwrap();
    assert_eq!(bytes, b"from a script");
}

#[test]
fn ls_returns_an_array() {
    script_test!(h);
    h.write("one.txt", "1");
    h.write("two.txt", "2");

    // dog3 returned a newline-joined string; dog4 returns a real array.
    let out = h
        .run("use ls from mod bot; put { len { ls \"\" } }")
        .unwrap();
    assert_eq!(out, "2");
}

#[test]
fn a_filesystem_error_is_catchable() {
    script_test!(h);
    // A bare `try` discards its value, so it has to be used as an expression.
    let out = h
        .run("use read from mod bot; put (try { read nope.txt } catch e { put caught })")
        .unwrap();
    assert_eq!(out, "caught");
}

#[test]
fn an_endless_loop_hits_the_time_limit() {
    script_test!(h);
    let config = Config {
        timeout: 0.5,
        net: false,
    };
    let err = run_script(
        h.pool.clone(),
        h.chat,
        USER,
        "while true {}",
        &config,
        &ScriptContext {
            message: String::new(),
            sender: String::new(),
            reply: String::new(),
            reply_from: String::new(),
        },
        Hooks::default(),
    )
    .unwrap_err();
    assert!(
        err.contains("time") || err.contains("fuel"),
        "unexpected error: {err}"
    );
}

#[test]
fn net_is_withheld_when_the_config_says_so() {
    script_test!(h);
    let config = Config {
        timeout: 5.0,
        net: false,
    };
    // A withheld capability removes the builtin, so this fails to compile.
    let err = run_script(
        h.pool.clone(),
        h.chat,
        USER,
        "get https://example.com",
        &config,
        &ScriptContext {
            message: String::new(),
            sender: String::new(),
            reply: String::new(),
            reply_from: String::new(),
        },
        Hooks::default(),
    )
    .unwrap_err();
    assert!(err.contains("get"), "unexpected error: {err}");
}

/// Cross-chat imports are deliberate: a module in another chat is only
/// importable once its owner grants read and execute to others.
#[test]
fn a_cross_chat_import_needs_the_others_execute_bit() {
    script_test!(h);

    // A second chat owned by a different user, which `h`'s user is NOT in, so
    // the others_* bits are what govern access.
    const OWNER: UserId = UserId(80002);
    let other = ChatId(h.chat.0 - 1_000_000);
    let alias = format!("shared{}", other.0.unsigned_abs());
    {
        let mut cn = h.pool.get().unwrap();
        model::Chat::upsert(&mut cn, other, Some("shared"), true).unwrap();
        model::Chat::set_alias(&mut cn, other, Some(&alias)).unwrap();
        model::User::upsert(&mut cn, OWNER, "Owner", None, None).unwrap();
        model::UserInChat::upsert(&mut cn, OWNER, other).unwrap();

        files::mkdir(&mut cn, other, OWNER, "lib", true).unwrap();
        files::write(
            &mut cn,
            other,
            OWNER,
            "lib/dice.dog",
            b"pub fn roll () { put 4 }",
        )
        .unwrap();
        let x: files::perm::Clause = "u+x".parse().unwrap();
        files::set_permission(&mut cn, other, OWNER, "lib/dice.dog", x).unwrap();

        // Sanity: our user really is an outsider to that chat.
        assert!(!model::UserInChat::exists(&mut cn, USER, other).unwrap());
    }

    // New files default to rw-r-----, so others cannot execute them.
    let source = format!("use roll from mod {alias}:lib/dice; roll");
    let err = h.run(&source).unwrap_err();
    assert!(
        err.contains("permission denied") || err.contains("dice"),
        "unexpected error: {err}"
    );

    // Granting read and execute to others opens it up.
    {
        let mut cn = h.pool.get().unwrap();
        for clause in ["o+r", "o+x"] {
            let clause: files::perm::Clause = clause.parse().unwrap();
            files::set_permission(&mut cn, other, OWNER, "lib/dice.dog", clause).unwrap();
        }
    }

    assert_eq!(h.run(&source).unwrap(), "4");
}

/// The chat's scripts depend on this. A module can't see the variables of the
/// code that imported it, so a command module reads the message context
/// straight from the globals; that only works because the host installs them
/// alongside the builtins.
#[test]
fn the_message_globals_are_visible_inside_a_module() {
    script_test!(h);
    h.write(
        "lib/who.dog",
        r#"pub fn who () { put "$sender/$reply_from/$reply/$message" }"#,
    );
    h.make_executable("lib/who.dog");
    let out = h
        .run_as("use who from mod lib/who; who", "oi bot", "texto", "Ana")
        .unwrap();
    assert_eq!(out, "Tester/Ana/texto/oi bot");
}

#[test]
fn later_queues_a_script_once() {
    script_test!(h);
    h.write("jobs/tidy.dog", "pln tidied");
    h.make_executable("jobs/tidy.dog");
    let (out, queued) = h.run_queueing(
        "use later from mod bot; later jobs/tidy.dog; later jobs/tidy.dog; put queued",
    );
    assert_eq!(out.unwrap(), "queued");
    assert_eq!(queued, vec!["jobs/tidy.dog".to_string()]);
}

/// Checked when queued, so a mistake surfaces in the reply instead of only in
/// the server log once the background run fails.
#[test]
fn later_refuses_a_script_that_cannot_run() {
    script_test!(h);
    h.write("jobs/plain.dog", "pln nope");
    let (out, queued) = h.run_queueing(
        "use later from mod bot; put (try { later jobs/missing.dog } catch e { put missing; });
         put (try { later jobs/plain.dog } catch e { put \" denied\" })",
    );
    assert_eq!(out.unwrap(), "missing denied");
    assert!(queued.is_empty());
}

#[test]
fn a_deferred_run_cannot_queue_another() {
    script_test!(h);
    h.write("jobs/tidy.dog", "pln tidied");
    h.make_executable("jobs/tidy.dog");
    let out = h
        .run("use later from mod bot; put (try { later jobs/tidy.dog } catch e { put $e.message })")
        .unwrap();
    assert!(out.contains("deferred"), "unexpected: {out}");
}

// --- send, edit and sleep ---------------------------------------------------
//
// None of these touch the database or Telegram: the courier is a fake and the
// pool never connects, so they run without `TEST_DATABASE_URL`.

use courier::Courier;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
struct FakeCourier {
    /// `(id, mode, text)` for every send, and `edit` records under the same id.
    log: Mutex<Vec<String>>,
}

fn describe(text: &Formatted) -> String {
    match text {
        Formatted::Raw(t) => format!("plain:{t}"),
        Formatted::Code(t) => format!("code:{t}"),
        Formatted::Html(t) => format!("html:{t}"),
        Formatted::Markdown(t) => format!("markdown:{t}"),
    }
}

impl Courier for FakeCourier {
    fn send(&self, text: Formatted, _: Option<Duration>) -> Result<i32, String> {
        let mut log = self.log.lock().unwrap();
        log.push(format!("send {}", describe(&text)));
        Ok(100 + log.len() as i32)
    }
    fn edit(&self, id: i32, text: Formatted, _: Option<Duration>) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("edit {id} {}", describe(&text)));
        Ok(())
    }
    fn delete(&self, id: i32, _: Option<Duration>) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("delete {id}"));
        Ok(())
    }
}

fn offline_pool() -> PgPool {
    diesel::r2d2::Pool::builder()
        .max_size(1)
        .min_idle(Some(0))
        .connection_timeout(Duration::from_millis(1))
        .build_unchecked(diesel::r2d2::ConnectionManager::new("postgres://invalid/invalid"))
}

fn run_offline(
    source: &str,
    config: &Config,
    courier: Option<Arc<dyn Courier>>,
) -> Result<String, String> {
    run_script(
        offline_pool(),
        ChatId(-1),
        UserId(1),
        source,
        config,
        &ScriptContext {
            message: String::new(),
            sender: String::new(),
            reply: String::new(),
            reply_from: String::new(),
        },
        Hooks {
            later: None,
            courier,
        },
    )
}

fn run_sending(source: &str) -> (Result<String, String>, Vec<String>) {
    let courier = Arc::new(FakeCourier::default());
    let out = run_offline(source, &Config::default(), Some(courier.clone()));
    let log = courier.log.lock().unwrap().clone();
    (out, log)
}

#[test]
fn send_and_send_id_deliver_in_each_format_and_only_send_id_returns_the_id() {
    let (out, log) = run_sending(
        r#"use send, send_id from mod bot;
           send "one";
           a = { send_id "two" markdown };
           send "<b>three</b>" html;
           send "four" code;
           put $a"#,
    );
    // Only `send_id` returns the id; `send` leaves nothing behind in the reply.
    assert_eq!(out.unwrap(), "102");
    assert_eq!(
        log,
        vec![
            "send plain:one",
            "send markdown:two",
            "send html:<b>three</b>",
            "send code:four",
        ]
    );
}

#[test]
fn edit_rewrites_a_message_sent_earlier() {
    let (out, log) = run_sending(
        r#"use send_id, edit from mod bot;
           id = { send_id "working…" };
           edit $id "done" html;
           edit $id "really done";
           put ok"#,
    );
    assert_eq!(out.unwrap(), "ok");
    assert_eq!(
        log,
        vec![
            "send plain:working…",
            "edit 101 html:done",
            "edit 101 plain:really done",
        ]
    );
}

#[test]
fn delete_removes_a_message_by_id() {
    let (out, log) = run_sending(
        r#"use send_id, delete from mod bot;
           id = { send_id "oops" };
           delete $id;
           put gone"#,
    );
    assert_eq!(out.unwrap(), "gone");
    assert_eq!(log, vec!["send plain:oops", "delete 101"]);
}

#[test]
fn delete_needs_a_real_id_and_reports_refusals() {
    let (out, log) = run_sending(
        r#"use delete from mod bot;
           put (try { delete hello } catch e { put "[$e.message]" });
           put (try { delete 1.5 } catch e { put "[$e.message]" })"#,
    );
    let out = out.unwrap();
    assert!(out.contains("expects a num"), "got {out:?}");
    assert!(out.contains("needs a message id"), "got {out:?}");
    assert!(log.is_empty());

    let out = run_offline(
        r#"use delete from mod bot; put (try { delete 5 } catch e { put $e.message })"#,
        &Config::default(),
        Some(Arc::new(Refusing)),
    )
    .unwrap();
    assert!(out.contains("could not delete") && out.contains("not yours"), "got {out:?}");
}

/// A courier that refuses to delete, as the real one does for other people's messages.
struct Refusing;
impl Courier for Refusing {
    fn send(&self, _: Formatted, _: Option<Duration>) -> Result<i32, String> {
        Ok(1)
    }
    fn edit(&self, _: i32, _: Formatted, _: Option<Duration>) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _: i32, _: Option<Duration>) -> Result<(), String> {
        Err("not yours".into())
    }
}

#[test]
fn send_refuses_what_telegram_would() {
    let (out, log) = run_sending(
        r#"use send from mod bot;
           put (try { send "" } catch e { put "[$e.message]" });
           put (try { send hi pdf } catch e { put "[$e.message]" })"#,
    );
    let out = out.unwrap();
    assert!(out.contains("empty"), "got {out:?}");
    assert!(out.contains("unknown format"), "got {out:?}");
    assert!(log.is_empty());
}

#[test]
fn send_is_unavailable_without_a_chat_to_talk_to() {
    let err = run_offline("use send from mod bot; send hi", &Config::default(), None).unwrap_err();
    assert!(err.contains("cannot send"), "got {err:?}");
}

#[test]
fn a_run_has_a_message_budget() {
    let (out, log) = run_sending(
        r#"use send from mod bot;
           for i in (0..100) { send "m$i" }"#,
    );
    let err = out.unwrap_err();
    assert!(err.contains("at most"), "got {err:?}");
    assert_eq!(log.len(), runtime::MAX_MESSAGES as usize);
}

#[test]
fn a_courier_failure_is_catchable() {
    struct Down;
    impl Courier for Down {
        fn send(&self, _: Formatted, _: Option<Duration>) -> Result<i32, String> {
            Err("Too Many Requests: retry after 3".into())
        }
        fn edit(&self, _: i32, _: Formatted, _: Option<Duration>) -> Result<(), String> {
            Err("nope".into())
        }
        fn delete(&self, _: i32, _: Option<Duration>) -> Result<(), String> {
            Err("not yours".into())
        }
    }
    let out = run_offline(
        r#"use send from mod bot; put (try { send hi } catch e { put $e.message })"#,
        &Config::default(),
        Some(Arc::new(Down)),
    )
    .unwrap();
    assert!(out.contains("could not send") && out.contains("retry after 3"), "got {out:?}");
}

#[test]
fn sleep_waits_then_continues() {
    let started = Instant::now();
    let out = run_offline(
        "use sleep from mod bot; sleep 0.3; put done",
        &Config::default(),
        None,
    )
    .unwrap();
    assert_eq!(out, "done");
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[test]
fn sleep_cannot_outlive_the_time_limit() {
    let config = Config {
        timeout: 0.4,
        net: false,
    };
    let started = Instant::now();
    let err = run_offline("use sleep from mod bot; sleep 60", &config, None).unwrap_err();
    assert!(err.contains("too long"), "got {err:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "slept {:?}", started.elapsed());
}

#[test]
fn sleep_rejects_nonsense() {
    let out = run_offline(
        r#"use sleep from mod bot;
           put (try { sleep -1 } catch e { put "[negative]" });
           put (try { sleep 1e400 } catch e { put "[huge]" })"#,
        &Config { timeout: 0.3, net: false },
        None,
    );
    // -1 is refused outright; an absurd value is clamped by the time limit.
    let text = out.unwrap_or_else(|e| e);
    assert!(text.contains("[negative]"), "got {text:?}");
}

/// The progress-bar example from the README, so the docs can't rot.
#[test]
fn the_readme_progress_example_runs() {
    let (out, log) = run_sending(
        r#"use send_id, edit, sleep from mod bot;

           id = { send_id "Working… 0%" };
           for pct in [25 50 75] {
               sleep 0.01;
               edit $id "Working… $pct%";
           }
           edit $id "Done.";"#,
    );
    assert_eq!(out.unwrap(), "");
    assert_eq!(
        log,
        vec![
            "send plain:Working… 0%",
            "edit 101 plain:Working… 25%",
            "edit 101 plain:Working… 50%",
            "edit 101 plain:Working… 75%",
            "edit 101 plain:Done.",
        ]
    );
}

#[test]
fn the_starter_script_answers_and_counts() {
    script_test!(h);
    let starter = crate::commands::init::MAIN_TEMPLATE;
    let say = |message: &str| h.run_as(starter, message, "", "").unwrap();
    assert_eq!(say("ping"), "pong");
    assert_eq!(say("Hello there"), "Hi Tester! Edit main.dog to teach me something new.");
    assert_eq!(say("count"), "I've counted to 1.");
    assert_eq!(say("count"), "I've counted to 2.");
    // Anything else gets no reply, so the starter is quiet in a group.
    assert_eq!(say("what's up"), "");
    assert_eq!(say(""), "");
}

#[test]
fn init_lets_every_member_of_a_group_run_the_starter() {
    script_test!(h);
    let mut cn = h.pool.get().unwrap();
    crate::commands::init::init(&mut cn, h.chat, USER, None).unwrap();
    assert!(
        crate::commands::init::init(&mut cn, h.chat, USER, None).is_err(),
        "a second /init must not overwrite main.dog"
    );

    // main.dog runs with the sender's permissions, so the rest of the group
    // needs read and execute too.
    let member = UserId(80002);
    model::User::upsert(&mut cn, member, "Member", None, None).unwrap();
    model::UserInChat::upsert(&mut cn, member, h.chat).unwrap();
    assert!(load_main(&mut cn, h.chat, member).is_ok());

    let outsider = UserId(80003);
    let err = load_main(&mut cn, h.chat, outsider).unwrap_err();
    assert!(matches!(err, files::FileError::NotEnoughPermissions(_)), "{err:?}");
    assert!(explain_missing(&err).unwrap().contains("chmod"));
}

#[test]
fn a_chat_without_main_dog_says_how_to_make_one() {
    script_test!(h);
    let err = load_main(&mut h.pool.get().unwrap(), h.chat, USER).unwrap_err();
    assert!(explain_missing(&err).unwrap().contains("/init"));
}

// ---------------------------------------------------------------------------
// The examples in `examples/`, so the gallery can't rot.
// ---------------------------------------------------------------------------

fn run_example(h: &Harness, source: &str, message: &str, sender: &str, reply: (&str, &str)) -> String {
    run_script(
        h.pool.clone(),
        h.chat,
        USER,
        source,
        &Config::default(),
        &ScriptContext {
            message: message.to_string(),
            sender: sender.to_string(),
            reply: reply.0.to_string(),
            reply_from: reply.1.to_string(),
        },
        Hooks::default(),
    )
    .unwrap_or_else(|err| panic!("{message:?}: {err}"))
}

#[test]
fn example_dice() {
    script_test!(h);
    let src = include_str!("../../examples/dice.dog");
    let roll = |m: &str| run_example(&h, src, m, "Ana", ("", ""));
    let out = roll("roll 2d6");
    let total: i64 = out.rsplit("= ").next().unwrap().parse().unwrap();
    assert!((2..=12).contains(&total), "{out}");
    assert!(roll("roll d20+3").contains(" + 3 = "));
    assert!(roll("roll 4d6 - 1").contains(" - 1 = "));
    assert!(roll("roll 0d6").starts_with("I roll"));
    assert_eq!(roll("hello"), "");
}

#[test]
fn example_scoreboard() {
    script_test!(h);
    let src = include_str!("../../examples/scoreboard.dog");
    let say = |who: &str, m: &str| run_example(&h, src, m, who, ("", ""));
    assert_eq!(say("Ana", "scores"), "Nobody has scored yet. Send: point");
    assert_eq!(say("Ana", "point"), "Ana has 1 point(s).");
    say("Bo", "Point");
    assert_eq!(say("Bo", "point"), "Bo has 2 point(s).");
    assert_eq!(say("Ana", "scores"), "1. Bo: 2\n2. Ana: 1");
    assert_eq!(say("Ana", "reset scores"), "Scores cleared.");
    assert_eq!(say("Ana", "scores"), "Nobody has scored yet. Send: point");
}

#[test]
fn example_quotes() {
    script_test!(h);
    let src = include_str!("../../examples/quotes.dog");
    let say = |m: &str, reply: (&str, &str)| run_example(&h, src, m, "Ana", reply);
    assert!(say("quote", ("", "")).starts_with("No quotes yet"));
    assert!(say("remember", ("", "")).starts_with("Reply to a message"));
    assert_eq!(say("remember", ("to be or not", "Bo")), "Saved quote #1.");
    assert_eq!(say("quote", ("", "")), "“to be or not” — Bo");
}

#[test]
fn example_router() {
    script_test!(h);
    h.write("lib/commands.dog", include_str!("../../examples/router/lib/commands.dog"));
    h.make_executable("lib/commands.dog");
    let src = include_str!("../../examples/router/main.dog");
    let say = |m: &str| run_example(&h, src, m, "Ana", ("", ""));
    assert_eq!(say("!help"), "Commands: !help, !echo, !flip, !pick");
    assert_eq!(say("!echo hi there"), "hi there");
    assert!(["heads", "tails"].contains(&say("!flip").as_str()));
    assert!(say("!nope").starts_with("Unknown command !nope"));
    assert_eq!(say("no bang"), "");
}

#[test]
fn example_progress() {
    script_test!(h);
    let courier = Arc::new(FakeCourier::default());
    let out = run_script(
        h.pool.clone(),
        h.chat,
        USER,
        include_str!("../../examples/progress.dog"),
        &Config::default(),
        &ScriptContext {
            message: "work".into(),
            sender: "Ana".into(),
            reply: String::new(),
            reply_from: String::new(),
        },
        Hooks {
            later: None,
            courier: Some(courier.clone()),
        },
    )
    .unwrap();
    assert!(out.starts_with("Done!"), "{out}");
    let log = courier.log.lock().unwrap();
    assert_eq!(log.first().unwrap(), "send plain:Working… 0%");
    assert!(log.contains(&"edit 101 plain:Working… 100%".to_string()), "{log:?}");
    assert_eq!(log.last().unwrap(), "delete 101");
}

#[test]
fn a_refused_init_changes_nothing() {
    script_test!(h);
    let mut cn = h.pool.get().unwrap();
    // A name that's not allowed: the files must not be written either.
    assert!(crate::commands::init::init(&mut cn, h.chat, USER, Some("bad name")).is_err());
    assert!(matches!(
        files::stat(&mut cn, h.chat, USER, "main.dog"),
        Err(files::FileError::FileNotFound(_))
    ));

    // And with main.dog already there, a second /init must not rename the chat.
    let name = format!("init{}", h.chat.0.unsigned_abs());
    crate::commands::init::init(&mut cn, h.chat, USER, Some(&name)).unwrap();
    assert!(crate::commands::init::init(&mut cn, h.chat, USER, Some("othername")).is_err());
    let chat = model::Chat::find(&mut cn, ChatAddress::Id(h.chat)).unwrap().unwrap();
    assert_eq!(chat.alias.as_deref(), Some(name.as_str()));
}
