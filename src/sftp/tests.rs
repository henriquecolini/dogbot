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

/// Seeds a chat, two users and an SSH key, then starts a server on a free port.
/// Returns the port, the chat's alias and the client key.
async fn harness() -> Option<(u16, String, russh::keys::PrivateKey)> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;

    // Each test builds its own pool, so keep them small: the default of 10
    // times the number of parallel tests exhausts Postgres's connection limit.
    let pool: PgPool = diesel::r2d2::Pool::builder()
        .max_size(2)
        .build(diesel::r2d2::ConnectionManager::new(url))
        .expect("pool");

    let key = russh::keys::PrivateKey::random(
        &mut rand::rng(),
        russh::keys::Algorithm::Ed25519,
    )
    .unwrap();
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
        crate::run_migrations(&mut cn);
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
    let mut session =
        russh::client::connect(config, ("127.0.0.1", port), AcceptAnyServer).await?;

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
    let other = russh::keys::PrivateKey::random(
        &mut rand::rng(),
        russh::keys::Algorithm::Ed25519,
    )
    .unwrap();
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
    let stranger = russh::keys::PrivateKey::random(
        &mut rand::rng(),
        russh::keys::Algorithm::Ed25519,
    )
    .unwrap();
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
    // Unlike `harness`, this may run against a database nothing else touched.
    crate::run_migrations(&mut pool.get().expect("connection"));
    eprintln!("serving on 127.0.0.1:52222 (ctrl-c to stop)");
    super::run(pool, "127.0.0.1:52222".to_string()).await.unwrap();
}
