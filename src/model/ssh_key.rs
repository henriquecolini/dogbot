use diesel::*;
use teloxide::prelude::*;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::schema::ssh_keys::dsl as keys;

/// `public_key` and `created_at` are stored for display and audit rather than
/// for authentication, which matches on the fingerprint alone.
#[allow(dead_code)]
#[derive(Selectable, Queryable, Debug, Clone)]
#[diesel(table_name = crate::schema::ssh_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct SshKey {
    pub id: Uuid,
    pub user_id: i64,
    pub fingerprint: String,
    pub public_key: String,
    pub label: Option<String>,
    pub created_at: OffsetDateTime,
}

impl SshKey {
    pub fn insert(
        cn: &mut PgConnection,
        user_id: UserId,
        fingerprint: &str,
        public_key: &str,
        label: Option<&str>,
    ) -> QueryResult<SshKey> {
        insert_into(keys::ssh_keys)
            .values((
                keys::user_id.eq(user_id.0 as i64),
                keys::fingerprint.eq(fingerprint),
                keys::public_key.eq(public_key),
                keys::label.eq(label),
            ))
            .returning(SshKey::as_select())
            .get_result(cn)
    }

    /// Resolves an offered public key to the user who registered it. This is
    /// the whole of SFTP authentication.
    pub fn find_by_fingerprint(
        cn: &mut PgConnection,
        fingerprint: &str,
    ) -> QueryResult<Option<SshKey>> {
        keys::ssh_keys
            .select(SshKey::as_select())
            .filter(keys::fingerprint.eq(fingerprint))
            .first(cn)
            .optional()
    }

    pub fn list(cn: &mut PgConnection, user_id: UserId) -> QueryResult<Vec<SshKey>> {
        keys::ssh_keys
            .select(SshKey::as_select())
            .filter(keys::user_id.eq(user_id.0 as i64))
            .order(keys::created_at)
            .load(cn)
    }

    pub fn delete(cn: &mut PgConnection, user_id: UserId, id: Uuid) -> QueryResult<usize> {
        delete(
            keys::ssh_keys
                .filter(keys::id.eq(id))
                .filter(keys::user_id.eq(user_id.0 as i64)),
        )
        .execute(cn)
    }
}
