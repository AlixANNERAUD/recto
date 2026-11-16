//! What each printer charges, set on the Printers administration page. Submission reads the
//! prices of the job's printer and the ledger keeps the cost it computes, so a new price only
//! applies to later jobs.
//!
//! Printers are named as CUPS names them. A printer without a row is free, and setting every
//! price to 0 removes the row, so only priced printers take room.

use std::collections::HashMap;

use chrono::Utc;
use recto_entity::printer_prices;
use sea_orm::{ActiveValue::Set, ConnectionTrait, EntityTrait, sea_query::OnConflict};

use crate::error::Result;
pub use crate::ledger::Prices;

/// The prices of a printer; all 0 when it has none.
pub async fn get(db: &impl ConnectionTrait, printer: &str) -> Result<Prices> {
    Ok(printer_prices::Entity::find_by_id(printer.to_owned())
        .one(db)
        .await?
        .map(prices_of)
        .unwrap_or_default())
}

/// Sets the prices of a printer; all 0 removes them. One statement either way, so two saves of
/// the same printer cannot clash.
pub async fn set(db: &impl ConnectionTrait, printer: &str, prices: Prices) -> Result<()> {
    if prices == Prices::default() {
        printer_prices::Entity::delete_by_id(printer.to_owned())
            .exec(db)
            .await?;
        return Ok(());
    }
    let row = printer_prices::ActiveModel {
        printer: Set(printer.to_owned()),
        mono_page: Set(prices.mono_page),
        color_page: Set(prices.color_page),
        sheet: Set(prices.sheet),
        updated_at: Set(Utc::now().fixed_offset()),
    };
    printer_prices::Entity::insert(row)
        .on_conflict(
            OnConflict::column(printer_prices::Column::Printer)
                .update_columns([
                    printer_prices::Column::MonoPage,
                    printer_prices::Column::ColorPage,
                    printer_prices::Column::Sheet,
                    printer_prices::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// The priced printers by name.
pub async fn all(db: &impl ConnectionTrait) -> Result<HashMap<String, Prices>> {
    Ok(printer_prices::Entity::find()
        .all(db)
        .await?
        .into_iter()
        .map(|row| (row.printer.clone(), prices_of(row)))
        .collect())
}

fn prices_of(row: printer_prices::Model) -> Prices {
    Prices {
        mono_page: row.mono_page,
        color_page: row.color_page,
        sheet: row.sheet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prices_round_trip_and_zero_removes_them() {
        let database = crate::testing::database().await;
        assert_eq!(get(&database, "office").await.unwrap(), Prices::default());
        assert!(all(&database).await.unwrap().is_empty());

        let priced = Prices {
            mono_page: 5,
            color_page: 20,
            sheet: 1,
        };
        set(&database, "office", priced).await.unwrap();
        set(
            &database,
            "Bureau / 2e étage",
            Prices {
                sheet: 2,
                ..Prices::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(get(&database, "office").await.unwrap(), priced);
        // Replaced, not added.
        let cheaper = Prices {
            color_page: 15,
            ..priced
        };
        set(&database, "office", cheaper).await.unwrap();
        assert_eq!(get(&database, "office").await.unwrap(), cheaper);
        let every = all(&database).await.unwrap();
        assert_eq!(every.len(), 2);
        assert_eq!(every["office"], cheaper);
        assert_eq!(every["Bureau / 2e étage"].sheet, 2);

        set(&database, "office", Prices::default()).await.unwrap();
        assert_eq!(get(&database, "office").await.unwrap(), Prices::default());
        assert_eq!(all(&database).await.unwrap().len(), 1);
        // Removing what is not there is fine.
        set(&database, "office", Prices::default()).await.unwrap();
    }
}
