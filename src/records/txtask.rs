//! A transaction owned by a task of its own, which runs the jobs sent to it in order.

use std::sync::Arc;

use deadpool_postgres::Transaction;
use tokio::sync::{mpsc, oneshot};

use super::api::FileEffects;
use crate::core::Core;
use crate::db::describe_pool_error;
use crate::error::ApiError;

type TxJob =
    Box<dyn for<'a> FnOnce(&'a Transaction<'a>, &'a mut FileEffects) -> BoxFuture<'a, ()> + Send>;
pub(crate) type BoxFuture<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

enum TxMessage {
    Job(TxJob),
    Commit(oneshot::Sender<Result<(), ApiError>>),
}

/// A transaction owned by its own task, so that writes can run from code that can't hold
/// a borrow of the connection (request hooks of batches, JavaScript migrations).
#[derive(Clone)]
pub(crate) struct TxHandle(mpsc::Sender<TxMessage>);

fn transaction_gone() -> ApiError {
    ApiError::internal(anyhow::anyhow!("the transaction ended early"))
}

impl TxHandle {
    /// Starts the task owning a new transaction. Without a commit, the transaction rolls
    /// back when the handles are dropped (or the task is aborted).
    pub(crate) fn start(
        core: &Arc<Core>,
    ) -> (
        TxHandle,
        Arc<tokio::sync::Mutex<FileEffects>>,
        tokio::task::JoinHandle<()>,
    ) {
        let effects = Arc::new(tokio::sync::Mutex::new(FileEffects::default()));
        let (sender, receiver) = mpsc::channel(1);
        let worker = tokio::spawn(run_transaction(core.clone(), receiver, effects.clone()));
        (TxHandle(sender), effects, worker)
    }

    /// Runs `f` in the transaction and returns its result.
    pub(crate) async fn run<T, F>(&self, f: F) -> Result<T, ApiError>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(
                &'a Transaction<'a>,
                &'a mut FileEffects,
            ) -> BoxFuture<'a, Result<T, ApiError>>
            + Send
            + 'static,
    {
        let (reply, result) = oneshot::channel();
        let job: TxJob = Box::new(move |tx, effects| {
            Box::pin(async move {
                let _ = reply.send(f(tx, effects).await);
            })
        });
        self.0
            .send(TxMessage::Job(job))
            .await
            .map_err(|_| transaction_gone())?;
        result.await.map_err(|_| transaction_gone())?
    }

    pub(crate) async fn commit(&self) -> Result<(), ApiError> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(TxMessage::Commit(reply))
            .await
            .map_err(|_| transaction_gone())?;
        result.await.map_err(|_| transaction_gone())?
    }
}

/// Owns the connection and transaction and runs the jobs sent to it, in order.
/// Ends with a commit, or rolls back when the handles are dropped without one.
async fn run_transaction(
    core: Arc<Core>,
    mut jobs: mpsc::Receiver<TxMessage>,
    effects: Arc<tokio::sync::Mutex<FileEffects>>,
) {
    let mut client = match core.pool.get().await {
        Ok(client) => client,
        Err(e) => return fail_all(jobs, ApiError::internal(describe_pool_error(e))).await,
    };
    let tx = match client.transaction().await {
        Ok(tx) => tx,
        Err(e) => return fail_all(jobs, ApiError::internal(e.into())).await,
    };
    while let Some(message) = jobs.recv().await {
        match message {
            TxMessage::Job(job) => {
                let mut effects = effects.lock().await;
                job(&tx, &mut effects).await;
            }
            TxMessage::Commit(reply) => {
                let _ = reply.send(tx.commit().await.map_err(|e| ApiError::internal(e.into())));
                return;
            }
        }
    }
}

/// Answers every message with `error` (the transaction couldn't start).
async fn fail_all(mut jobs: mpsc::Receiver<TxMessage>, error: ApiError) {
    tracing::warn!("transaction: {}", error.message);
    while let Some(message) = jobs.recv().await {
        // a job's reply sender is dropped with it, which reports the failure
        if let TxMessage::Commit(reply) = message {
            let _ = reply.send(Err(ApiError::internal(anyhow::anyhow!(
                "the transaction couldn't start"
            ))));
        }
    }
}
