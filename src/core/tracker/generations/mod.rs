use anyhow::{Context, Result};
use sqlx::SqlitePool;

mod schema;

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    let mut transaction = pool.begin().await?;
    for statement in schema::STATEMENTS {
        sqlx::query(statement)
            .execute(&mut *transaction)
            .await
            .context("Cannot initialize retained deployment metadata")?;
    }
    transaction
        .commit()
        .await
        .context("Cannot commit retained deployment metadata upgrade")
}

#[cfg(test)]
mod tests;
