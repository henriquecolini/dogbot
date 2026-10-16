//! Database-backed tests for the filesystem layer.
//!
//! These need a real Postgres: set `TEST_DATABASE_URL`. Without it they skip,
//! so `cargo test` stays usable on a machine with no database.

use super::*;
use crate::model::{Chat, File, User, UserInChat};
use std::sync::Once;
use std::sync::atomic::{AtomicI64, Ordering};

/// Each test gets its own chat id so they can share one database. The base is
/// derived from the clock so repeated runs against a persistent test database
/// don't collide with chats left behind by earlier ones.
static NEXT_CHAT: AtomicI64 = AtomicI64::new(0);

fn next_chat_id() -> ChatId {
    static BASE: Once = Once::new();
    BASE.call_once(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        NEXT_CHAT.store(-(nanos % 1_000_000_000_000), Ordering::SeqCst);
    });
    ChatId(NEXT_CHAT.fetch_sub(1, Ordering::SeqCst))
}

const OWNER: UserId = UserId(1001);
const MEMBER: UserId = UserId(1002);
const OUTSIDER: UserId = UserId(1003);

/// Connects, migrates, and seeds a fresh chat with three users. Returns `None`
/// when no test database is configured.
fn setup() -> Option<(PgConnection, ChatId)> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    let mut cn = PgConnection::establish(&url).expect("connect to TEST_DATABASE_URL");

    crate::run_migrations_once(&mut cn);

    let chat_id = next_chat_id();
    Chat::upsert(&mut cn, chat_id, Some("test"), true).unwrap();

    for (id, name) in [(OWNER, "owner"), (MEMBER, "member"), (OUTSIDER, "outsider")] {
        User::upsert(
            &mut cn,
            id,
            name,
            None,
            Some(&format!("{name}{}", chat_id.0)),
        )
        .unwrap();
    }
    UserInChat::upsert(&mut cn, OWNER, chat_id).unwrap();
    UserInChat::upsert(&mut cn, MEMBER, chat_id).unwrap();

    Some((cn, chat_id))
}

macro_rules! db_test {
    ($cn:ident, $chat:ident) => {
        let Some((mut $cn, $chat)) = setup() else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
        let $cn = &mut $cn;
    };
}

#[test]
fn empty_file_is_a_file_not_a_directory() {
    db_test!(cn, chat);
    // Editors create empty files constantly. `is_dir` is generated from
    // `content IS NULL`, so a zero-byte write must still produce a file.
    let file = write(cn, chat, OWNER, "empty.txt", b"").unwrap();
    assert!(file.is_file(), "zero-byte write produced a directory");
    assert!(!file.is_dir());

    let (name, content) = read(cn, chat, OWNER, "empty.txt").unwrap();
    assert_eq!(name, "empty.txt");
    assert!(content.is_empty());
    assert_eq!(File::size(cn, file.id).unwrap(), 0);
}

#[test]
fn stat_reports_files_dirs_and_missing_paths() {
    db_test!(cn, chat);
    write(cn, chat, OWNER, "a.txt", b"hello").unwrap();
    mkdir(cn, chat, OWNER, "d", false).unwrap();

    assert!(stat(cn, chat, OWNER, "a.txt").unwrap().is_file());
    assert!(stat(cn, chat, OWNER, "d").unwrap().is_dir());
    assert!(matches!(
        stat(cn, chat, OWNER, "nope.txt"),
        Err(FileError::FileNotFound(_))
    ));
}

#[test]
fn size_is_reported_without_reading_content() {
    db_test!(cn, chat);
    let f = write(cn, chat, OWNER, "a.txt", b"hello").unwrap();
    assert_eq!(File::size(cn, f.id).unwrap(), 5);

    mkdir(cn, chat, OWNER, "d", false).unwrap();
    write(cn, chat, OWNER, "d/b.txt", b"xyz").unwrap();
    let root = File::get_root_dir(cn, chat).unwrap();
    let listing = File::list_children_with_size(cn, root.id).unwrap();

    let a = listing.iter().find(|(f, _)| f.name == "a.txt").unwrap();
    let d = listing.iter().find(|(f, _)| f.name == "d").unwrap();
    assert_eq!(a.1, 5);
    assert_eq!(d.1, 0, "directories have no content");
}

#[test]
fn rename_within_a_directory() {
    db_test!(cn, chat);
    write(cn, chat, OWNER, "old.dog", b"pln hi").unwrap();

    rename(cn, chat, OWNER, "old.dog", "new.dog").unwrap();

    assert_eq!(read(cn, chat, OWNER, "new.dog").unwrap().1, b"pln hi");
    assert!(matches!(
        read(cn, chat, OWNER, "old.dog"),
        Err(FileError::FileNotFound(_))
    ));
}

#[test]
fn rename_across_directories() {
    db_test!(cn, chat);
    mkdir(cn, chat, OWNER, "lib", false).unwrap();
    write(cn, chat, OWNER, "util.dog", b"pub fn x () {}").unwrap();

    rename(cn, chat, OWNER, "util.dog", "lib/util.dog").unwrap();

    assert_eq!(
        read(cn, chat, OWNER, "lib/util.dog").unwrap().1,
        b"pub fn x () {}"
    );
    assert!(matches!(
        read(cn, chat, OWNER, "util.dog"),
        Err(FileError::FileNotFound(_))
    ));
}

#[test]
fn rename_over_an_existing_file_replaces_it() {
    db_test!(cn, chat);
    // This is how editors save atomically: write a temp file, rename over.
    write(cn, chat, OWNER, "main.dog", b"old").unwrap();
    write(cn, chat, OWNER, ".main.dog.tmp", b"new").unwrap();

    rename(cn, chat, OWNER, ".main.dog.tmp", "main.dog").unwrap();

    assert_eq!(read(cn, chat, OWNER, "main.dog").unwrap().1, b"new");
    assert!(matches!(
        read(cn, chat, OWNER, ".main.dog.tmp"),
        Err(FileError::FileNotFound(_))
    ));
}

#[test]
fn rename_over_a_directory_fails() {
    db_test!(cn, chat);
    write(cn, chat, OWNER, "a.txt", b"x").unwrap();
    mkdir(cn, chat, OWNER, "d", false).unwrap();

    assert!(matches!(
        rename(cn, chat, OWNER, "a.txt", "d"),
        Err(FileError::FileAlreadyExists(_))
    ));
}

#[test]
fn rename_refuses_to_move_a_directory_into_itself() {
    db_test!(cn, chat);
    mkdir(cn, chat, OWNER, "a/b", true).unwrap();

    assert!(matches!(
        rename(cn, chat, OWNER, "a", "a/b/a"),
        Err(FileError::InvalidArguments)
    ));
    // The tree is still intact.
    assert!(stat(cn, chat, OWNER, "a/b").unwrap().is_dir());
}

#[test]
fn rename_requires_write_on_the_destination_parent() {
    db_test!(cn, chat);
    write(cn, chat, MEMBER, "f.txt", b"x").unwrap();
    mkdir(cn, chat, OWNER, "locked", false).unwrap();
    // Default dir perms are rwxr-xr-x, so a non-owner cannot write into it.
    assert!(matches!(
        rename(cn, chat, MEMBER, "f.txt", "locked/f.txt"),
        Err(FileError::NotEnoughPermissions(_))
    ));
    assert!(stat(cn, chat, MEMBER, "f.txt").unwrap().is_file());
}

#[test]
fn rename_requires_traverse_permission_for_outsiders() {
    db_test!(cn, chat);
    write(cn, chat, OWNER, "f.txt", b"x").unwrap();
    // Files default to rw-r----- , so an outsider cannot even read them.
    assert!(matches!(
        read(cn, chat, OUTSIDER, "f.txt"),
        Err(FileError::NotEnoughPermissions(_))
    ));
}

#[test]
fn chown_transfers_ownership_and_only_the_owner_may_do_it() {
    db_test!(cn, chat);
    let f = write(cn, chat, OWNER, "f.txt", b"x").unwrap();
    assert_eq!(f.owner_id, Some(OWNER.0 as i64));

    // A non-owner cannot give the file away.
    assert!(matches!(
        set_owner(cn, chat, MEMBER, "f.txt", MEMBER),
        Err(FileError::NotEnoughPermissions(_))
    ));

    set_owner(cn, chat, OWNER, "f.txt", MEMBER).unwrap();
    assert_eq!(
        stat(cn, chat, OWNER, "f.txt").unwrap().owner_id,
        Some(MEMBER.0 as i64)
    );
}

#[test]
fn chown_rejects_a_user_who_is_not_in_the_chat() {
    db_test!(cn, chat);
    write(cn, chat, OWNER, "f.txt", b"x").unwrap();
    assert!(matches!(
        set_owner(cn, chat, OWNER, "f.txt", OUTSIDER),
        Err(FileError::InvalidArguments)
    ));
}

#[test]
fn rename_over_a_file_keeps_the_destination_permissions() {
    db_test!(cn, chat);
    // Editors save by writing a temp file and renaming it over the target. The
    // temp file has default permissions, so taking the source's would strip
    // the execute bit that main.dog needs to run.
    write(cn, chat, OWNER, "main.dog", b"old").unwrap();
    let clause: Clause = "u+x".parse().unwrap();
    set_permission(cn, chat, OWNER, "main.dog", clause).unwrap();
    assert!(stat(cn, chat, OWNER, "main.dog").unwrap().user_execute);

    write(cn, chat, OWNER, ".main.dog.tmp", b"new").unwrap();
    assert!(!stat(cn, chat, OWNER, ".main.dog.tmp").unwrap().user_execute);

    rename(cn, chat, OWNER, ".main.dog.tmp", "main.dog").unwrap();

    let saved = stat(cn, chat, OWNER, "main.dog").unwrap();
    assert!(
        saved.user_execute,
        "an editor save stripped the execute bit"
    );
    assert_eq!(read(cn, chat, OWNER, "main.dog").unwrap().1, b"new");
}

#[test]
fn a_plain_rename_keeps_its_own_permissions() {
    db_test!(cn, chat);
    // With no file at the destination there is nothing to inherit, so the
    // moved file keeps what it had.
    write(cn, chat, OWNER, "a.dog", b"x").unwrap();
    let clause: Clause = "u+x".parse().unwrap();
    set_permission(cn, chat, OWNER, "a.dog", clause).unwrap();

    rename(cn, chat, OWNER, "a.dog", "b.dog").unwrap();

    assert!(stat(cn, chat, OWNER, "b.dog").unwrap().user_execute);
}
