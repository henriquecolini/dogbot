use diesel::*;

use crate::schema::settings::dsl as settings;

/// Process-level state that must survive restarts, kept in the database so no
/// extra volume has to be mounted.
pub struct Setting;

impl Setting {
    pub fn get(cn: &mut PgConnection, key: &str) -> QueryResult<Option<Vec<u8>>> {
        settings::settings
            .select(settings::value)
            .filter(settings::key.eq(key))
            .first(cn)
            .optional()
    }

    pub fn set(cn: &mut PgConnection, key: &str, value: &[u8]) -> QueryResult<()> {
        insert_into(settings::settings)
            .values((settings::key.eq(key), settings::value.eq(value)))
            .on_conflict(settings::key)
            .do_update()
            .set(settings::value.eq(value))
            .execute(cn)?;
        Ok(())
    }
}
