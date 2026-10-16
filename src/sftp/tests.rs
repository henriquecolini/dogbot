//! End-to-end tests: a real SSH + SFTP client against the real server,
//! talking to a real database. Needs `TEST_DATABASE_URL`, like the
//! filesystem tests.

use crate::prelude::*;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const USER: UserId = UserId(90001);
const OUTSIDER: UserId = UserId(90002);

struct AcceptAnyServer;

impl russh::client::Handler for AcceptAnyServer {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Seeds a chat, two users and an SSH key. Returns the pool, the chat's alias
/// and the client key, leaving it to the caller to start a server.
async fn seeded_pool() -> Option<(PgPool, String, russh::keys::PrivateKey)> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;

    // Each test builds its own pool, so keep them small: the default of 10
    // times the number of parallel tests exhausts Postgres's connection limit.
    let pool: PgPool = diesel::r2d2::Pool::builder()
        .max_size(2)
        .build(diesel::r2d2::ConnectionManager::new(url))
        .expect("pool");

    let key =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let fingerprint = key
        .public_key()
        .fingerprint(russh::keys::HashAlg::Sha256)
        .to_string();

    // A unique chat per run, so repeated runs don't collide.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let chat_id = ChatId(-(nanos % 1_000_000_000_000) - 500_000_000_000);
    let alias = format!("sftptest{}", chat_id.0.unsigned_abs());

    {
        let mut cn = pool.get().unwrap();
        crate::run_migrations_once(&mut cn);
        model::Chat::upsert(&mut cn, chat_id, Some("sftp test"), true).unwrap();
        model::Chat::set_alias(&mut cn, chat_id, Some(&alias)).unwrap();
        model::User::upsert(&mut cn, USER, "tester", None, None).unwrap();
        model::User::upsert(&mut cn, OUTSIDER, "outsider", None, None).unwrap();
        model::UserInChat::upsert(&mut cn, USER, chat_id).unwrap();
        model::SshKey::insert(
            &mut cn,
            USER,
            &fingerprint,
            &key.public_key().to_openssh().unwrap(),
            None,
        )
        .unwrap();
    }

    Some((pool, alias, key))
}

/// `seeded_pool`, plus a server on a free port with the production timings.
async fn harness() -> Option<(u16, String, russh::keys::PrivateKey)> {
    let (pool, alias, key) = seeded_pool().await?;

    // Bind port 0 to let the OS pick a free one and hand the live listener
    // straight to the server. Dropping it first and re-binding by number would
    // let a parallel test steal the port in between.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        if let Err(err) = super::run_on_listener(pool, listener).await {
            eprintln!("sftp server error: {err}");
        }
    });

    Some((port, alias, key))
}

async fn connect(
    port: u16,
    username: &str,
    key: &russh::keys::PrivateKey,
) -> anyhow::Result<SftpSession> {
    let config = Arc::new(russh::client::Config::default());
    let mut session = russh::client::connect(config, ("127.0.0.1", port), AcceptAnyServer).await?;

    let auth = session
        .authenticate_publickey(
            username,
            russh::keys::PrivateKeyWithHashAlg::new(
                Arc::new(key.clone()),
                Some(russh::keys::HashAlg::Sha512),
            ),
        )
        .await?;
    if !auth.success() {
        anyhow::bail!("authentication rejected");
    }

    let channel = session.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    Ok(SftpSession::new(channel.into_stream()).await?)
}

macro_rules! sftp_test {
    ($port:ident, $alias:ident, $key:ident) => {
        let Some(($port, $alias, $key)) = harness().await else {
            eprintln!("skipping: TEST_DATABASE_URL not set");
            return;
        };
    };
}

#[tokio::test]
async fn write_read_list_and_delete() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut file = sftp
        .open_with_flags(
            "/main.dog",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ,
        )
        .await
        .unwrap();
    file.write_all(b"pln hello").await.unwrap();
    file.shutdown().await.unwrap();

    let mut got = String::new();
    let mut file = sftp.open("/main.dog").await.unwrap();
    file.read_to_string(&mut got).await.unwrap();
    assert_eq!(got, "pln hello");

    let names: Vec<String> = sftp
        .read_dir("/")
        .await
        .unwrap()
        .map(|e| e.file_name())
        .collect();
    assert!(names.contains(&"main.dog".to_string()), "got {names:?}");

    sftp.remove_file("/main.dog").await.unwrap();
    assert!(sftp.metadata("/main.dog").await.is_err());
}

#[tokio::test]
async fn directories_and_nested_paths() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    sftp.create_dir("/lib").await.unwrap();
    assert!(sftp.metadata("/lib").await.unwrap().is_dir());

    let mut file = sftp
        .open_with_flags(
            "/lib/util.dog",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .unwrap();
    file.write_all(b"pub fn x () {}").await.unwrap();
    file.shutdown().await.unwrap();

    let names: Vec<String> = sftp
        .read_dir("/lib")
        .await
        .unwrap()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(names, vec!["util.dog".to_string()]);

    // rmdir must refuse a non-empty directory.
    assert!(sftp.remove_dir("/lib").await.is_err());
    sftp.remove_file("/lib/util.dog").await.unwrap();
    sftp.remove_dir("/lib").await.unwrap();
}

#[tokio::test]
async fn rename_supports_the_atomic_save_pattern() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    for (path, body) in [("/notes.txt", "old"), ("/.notes.txt.tmp", "new")] {
        let mut f = sftp
            .open_with_flags(
                path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .unwrap();
        f.write_all(body.as_bytes()).await.unwrap();
        f.shutdown().await.unwrap();
    }

    // This is exactly what an editor does on save.
    sftp.rename("/.notes.txt.tmp", "/notes.txt").await.unwrap();

    let mut got = String::new();
    sftp.open("/notes.txt")
        .await
        .unwrap()
        .read_to_string(&mut got)
        .await
        .unwrap();
    assert_eq!(got, "new");
    assert!(sftp.metadata("/.notes.txt.tmp").await.is_err());
}

#[tokio::test]
async fn empty_files_survive_a_round_trip() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut f = sftp
        .open_with_flags(
            "/empty.dog",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .unwrap();
    f.shutdown().await.unwrap();

    let meta = sftp.metadata("/empty.dog").await.unwrap();
    assert!(!meta.is_dir(), "an empty file must not become a directory");
    assert_eq!(meta.len(), 0);
}

#[tokio::test]
async fn partial_writes_land_at_the_right_offsets() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut f = sftp
        .open_with_flags(
            "/offsets.txt",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE | OpenFlags::READ,
        )
        .await
        .unwrap();
    f.write_all(b"aaaaaaaa").await.unwrap();
    f.seek(std::io::SeekFrom::Start(3)).await.unwrap();
    f.write_all(b"BB").await.unwrap();
    f.shutdown().await.unwrap();

    let mut got = String::new();
    sftp.open("/offsets.txt")
        .await
        .unwrap()
        .read_to_string(&mut got)
        .await
        .unwrap();
    assert_eq!(got, "aaaBBaaa");
}

#[tokio::test]
async fn a_non_member_cannot_mount_the_chat() {
    sftp_test!(port, alias, key);

    // A valid key, but registered to a user who is not in this chat.
    let other =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let mut cn = diesel::Connection::establish(&url).unwrap();
    model::SshKey::insert(
        &mut cn,
        OUTSIDER,
        &other
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string(),
        &other.public_key().to_openssh().unwrap(),
        None,
    )
    .unwrap();

    assert!(
        connect(port, &alias, &other).await.is_err(),
        "a non-member was allowed to mount the chat"
    );
    // The real member still gets in.
    assert!(connect(port, &alias, &key).await.is_ok());
}

#[tokio::test]
async fn an_unregistered_key_is_rejected() {
    sftp_test!(port, alias, _key);
    let stranger =
        russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519).unwrap();
    assert!(connect(port, &alias, &stranger).await.is_err());
}

#[tokio::test]
async fn paths_cannot_escape_the_chat_root() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    // `..` is normalised away rather than climbing out of the root.
    sftp.create_dir("/d").await.unwrap();
    let mut f = sftp
        .open_with_flags(
            "/d/../escaped.txt",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .unwrap();
    f.write_all(b"x").await.unwrap();
    f.shutdown().await.unwrap();

    let names: Vec<String> = sftp
        .read_dir("/")
        .await
        .unwrap()
        .map(|e| e.file_name())
        .collect();
    assert!(names.contains(&"escaped.txt".to_string()));

    // A host-qualified path would reach another chat; it must be refused.
    assert!(sftp.metadata("/other:main.dog").await.is_err());
}

/// Manual interop harness: serves the seeded `manualtest` chat on port 52222
/// until killed, so real clients (`sftp`, `sshfs`, VSCode) can be pointed at it.
///
/// Not part of the automatic suite. Run with:
///   cargo test -- --ignored --nocapture manual_server
#[tokio::test]
#[ignore]
async fn manual_server() {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL not set");
        return;
    };
    let pool: PgPool = diesel::r2d2::Pool::builder()
        .build(diesel::r2d2::ConnectionManager::new(url))
        .expect("pool");
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .is_test(false)
        .try_init();
    // Unlike `harness`, this may run against a database nothing else touched.
    crate::run_migrations(&mut pool.get().expect("connection"));

    // Seed a chat to mount, so this is usable without a live Telegram bot:
    //   MANUAL_PUBKEY="$(cat ~/.ssh/id_ed25519.pub)" cargo test -- --ignored manual_server
    //   sshfs -p 52222 manual@127.0.0.1:/ /mnt/point
    if let Ok(pubkey) = std::env::var("MANUAL_PUBKEY") {
        let mut cn = pool.get().expect("connection");
        let key = russh::keys::PublicKey::from_openssh(pubkey.trim()).expect("MANUAL_PUBKEY");
        let chat_id = ChatId(-999_000_000_001);
        model::Chat::upsert(&mut cn, chat_id, Some("manual"), true).unwrap();
        model::Chat::set_alias(&mut cn, chat_id, Some("manual")).unwrap();
        model::User::upsert(&mut cn, USER, "tester", None, None).unwrap();
        model::UserInChat::upsert(&mut cn, USER, chat_id).unwrap();
        let fingerprint = key.fingerprint(russh::keys::HashAlg::Sha256).to_string();
        if model::SshKey::find_by_fingerprint(&mut cn, &fingerprint)
            .unwrap()
            .is_none()
        {
            model::SshKey::insert(&mut cn, USER, &fingerprint, pubkey.trim(), None).unwrap();
        }
        eprintln!("seeded chat `manual` for {fingerprint}");
    }

    eprintln!("serving on 127.0.0.1:52222 (ctrl-c to stop)");
    super::run(pool, "127.0.0.1:52222".to_string())
        .await
        .unwrap();
}

/// The GIO atomic-save sequence, which is what gedit, GNOME Text Editor and
/// anything else built on `g_file_replace` does: write a `.goutputstream-*`
/// temp file, rename it over the target *while still holding it open*, and only
/// then close it.
///
/// This is the ordering that broke. Handles used to be keyed by path, so the
/// flush at close re-resolved the temp path — recreating the temp file with the
/// content and leaving the target holding the empty row the rename had moved
/// into place. Both operations reported success, so the editor showed a clean
/// save over a file it had just emptied.
#[tokio::test]
async fn rename_while_open_then_close_lands_on_the_target() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut target = sftp
        .open_with_flags(
            "/main.dog",
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .unwrap();
    target.write_all(b"original").await.unwrap();
    target.shutdown().await.unwrap();

    let mut temp = sftp
        .open_with_flags(
            "/.goutputstream-ABC123",
            OpenFlags::CREATE | OpenFlags::EXCLUDE | OpenFlags::WRITE,
        )
        .await
        .unwrap();
    temp.write_all(b"replacement").await.unwrap();
    // Not `shutdown`: the rename has to happen before the handle closes.
    temp.flush().await.unwrap();

    sftp.rename("/.goutputstream-ABC123", "/main.dog")
        .await
        .unwrap();
    temp.shutdown().await.unwrap();
    drop(temp);

    let mut got = String::new();
    let mut file = sftp.open("/main.dog").await.unwrap();
    file.read_to_string(&mut got).await.unwrap();
    assert_eq!(got, "replacement", "the flush must follow the renamed file");

    let names: Vec<String> = sftp
        .read_dir("/")
        .await
        .unwrap()
        .map(|e| e.file_name())
        .collect();
    assert!(
        !names.iter().any(|n| n.starts_with(".goutputstream")),
        "the temp file must not be recreated by the flush, got {names:?}"
    );
}

/// An open handle keeps working after its file is renamed somewhere else
/// entirely, the same way a Unix file descriptor does.
#[tokio::test]
async fn an_open_handle_follows_a_move_to_another_directory() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    sftp.create_dir("/lib").await.unwrap();
    let mut file = sftp
        .open_with_flags("/util.dog", OpenFlags::CREATE | OpenFlags::WRITE)
        .await
        .unwrap();
    file.write_all(b"fn helper() { }").await.unwrap();
    file.flush().await.unwrap();

    sftp.rename("/util.dog", "/lib/util.dog").await.unwrap();
    file.shutdown().await.unwrap();
    drop(file);

    let mut got = String::new();
    let mut moved = sftp.open("/lib/util.dog").await.unwrap();
    moved.read_to_string(&mut got).await.unwrap();
    assert_eq!(got, "fn helper() { }");
    assert!(sftp.metadata("/util.dog").await.is_err());
}

/// A write-only handle cannot be used to read the file back, and a read-only
/// handle cannot write. Permission is settled at open, so these have to be
/// enforced per handle rather than per path.
#[tokio::test]
async fn handles_honour_their_open_flags() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut file = sftp
        .open_with_flags("/notes.txt", OpenFlags::CREATE | OpenFlags::WRITE)
        .await
        .unwrap();
    file.write_all(b"secret").await.unwrap();
    file.shutdown().await.unwrap();

    let mut write_only = sftp
        .open_with_flags("/notes.txt", OpenFlags::WRITE)
        .await
        .unwrap();
    let mut sink = String::new();
    assert!(write_only.read_to_string(&mut sink).await.is_err());

    let mut read_only = sftp
        .open_with_flags("/notes.txt", OpenFlags::READ)
        .await
        .unwrap();
    // The client buffers writes, so the rejection surfaces on the flush.
    let rejected = read_only.write_all(b"nope").await.is_err() || read_only.flush().await.is_err();
    assert!(rejected, "a read-only handle must refuse a write");

    let mut unchanged = String::new();
    let mut check = sftp.open("/notes.txt").await.unwrap();
    check.read_to_string(&mut unchanged).await.unwrap();
    assert_eq!(unchanged, "secret");
}

/// A write is visible as soon as it is made, not when the handle closes.
///
/// Deferring content to close left a window where the file existed but read as
/// empty. Clients stat inside that window — GIO stats the target immediately
/// after renaming its temp file over it — and FUSE caches the zero it gets, so
/// the file looked truncated well after the save had finished.
#[tokio::test]
async fn a_write_is_visible_before_the_handle_closes() {
    sftp_test!(port, alias, key);
    let sftp = connect(port, &alias, &key).await.unwrap();

    let mut file = sftp
        .open_with_flags("/draft.dog", OpenFlags::CREATE | OpenFlags::WRITE)
        .await
        .unwrap();
    file.write_all(b"pln still open").await.unwrap();
    file.flush().await.unwrap();

    let meta = sftp.metadata("/draft.dog").await.unwrap();
    assert_eq!(
        meta.size,
        Some(14),
        "size must reflect the write already made"
    );

    let mut got = String::new();
    let mut reader = sftp.open("/draft.dog").await.unwrap();
    reader.read_to_string(&mut got).await.unwrap();
    assert_eq!(got, "pln still open");
}

/// An idle session must stay usable.
///
/// `inactivity_timeout` alone bounded a connection regardless of whether the
/// client was alive, so a mount left alone was dropped on the hour and sshfs
/// was left wedged. Keepalives reset that timer, so a client that answers them
/// keeps its session. The timings here are scaled down to seconds; in
/// production they are 30s against an hour.
#[tokio::test]
async fn an_idle_session_survives_past_the_inactivity_timeout() {
    let Some((pool, alias, key)) = seeded_pool().await else {
        eprintln!("skipping: TEST_DATABASE_URL not set");
        return;
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = super::run_with_timings(
            pool,
            listener,
            std::time::Duration::from_millis(300),
            std::time::Duration::from_secs(1),
        )
        .await;
    });

    let sftp = connect(port, &alias, &key).await.unwrap();
    sftp.create_dir("/before").await.unwrap();

    // Several times the inactivity timeout, with no SFTP traffic at all.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;

    sftp.create_dir("/after")
        .await
        .expect("the session must still be alive after sitting idle");
}
