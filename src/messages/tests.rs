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
        run_script(
            self.pool.clone(),
            self.chat,
            USER,
            source,
            &Config::default(),
            &ScriptContext {
                message: "hello world".to_string(),
                sender: "Tester".to_string(),
                reply: String::new(),
                reply_from: String::new(),
            },
        )
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

    static MIGRATED: Once = Once::new();
    MIGRATED.call_once(|| {
        let mut cn = pool.get().unwrap();
        crate::run_migrations(&mut cn);
    });

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
    h.write("lib/greet.dog", "pub fn hi (name: str) { put \"hi $name\" }");
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
