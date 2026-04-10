//! MongoDB driver `Collection<T>` already use `Arc` internally to share the connection (cloning it is cheap)
//! Wrapping in `Arc` to share it between tokio thread/task without problem.

use crate::saga::{SagaState, SagaStatus};
use anyhow::{Context, Result};
use mongodb::{
    Collection, Database,
    bson::{doc, to_document},
};

const COLLECTION: &str = "order_sagas";

#[derive(Clone)]
pub struct SagaRepository {
    col: Collection<SagaState>,
}

impl SagaRepository {
    pub fn new(db: &Database) -> Self {
        Self {
            col: db.collection(COLLECTION),
        }
    }

    pub async fn save(&self, state: &SagaState) -> Result<()> {
        let filter = doc! { "_id": &state.saga_id };
        let _ = to_document(state).context("Serializzazione SagaState fallita")?;

        self.col
            .replace_one(filter, state)
            .upsert(true)
            .await
            .context("MongoDB save SagaState fallito")?;

        Ok(())
    }

    pub async fn find_by_id(&self, saga_id: &str) -> Result<Option<SagaState>> {
        self.col
            .find_one(doc! { "_id": saga_id })
            .await
            .context("MongoDB find_by_id fallito")
    }

    pub async fn find_by_order_id(&self, order_id: &str) -> Result<Option<SagaState>> {
        self.col
            .find_one(doc! { "order_id": order_id })
            .await
            .context("MongoDB find_by_order_id fallito")
    }

    pub async fn find_by_status(&self, status: &SagaStatus) -> Result<Vec<SagaState>> {
        use futures::TryStreamExt;
        use mongodb::bson::to_bson;

        let status_bson = to_bson(status).context("Serializzazione SagaStatus fallita")?;
        let mut cursor = self
            .col
            .find(doc! { "status": status_bson })
            .await
            .context("MongoDB find_by_status fallito")?;

        let mut results = Vec::new();
        while let Some(doc) = cursor.try_next().await.context("Cursor MongoDB fallito")? {
            results.push(doc);
        }
        Ok(results)
    }
}
