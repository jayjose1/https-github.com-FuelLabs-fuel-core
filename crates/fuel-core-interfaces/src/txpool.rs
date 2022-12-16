use derive_more::{
    Deref,
    DerefMut,
};
use fuel_core_storage::{
    tables::{
        Coins,
        Messages,
    },
    Error as StorageError,
    StorageAsRef,
    StorageInspect,
};
use fuel_core_types::{
    blockchain::primitives::BlockHeight,
    entities::{
        coin::Coin,
        message::Message,
    },
    fuel_tx::{
        Transaction,
        TxId,
        UtxoId,
    },
    fuel_types::{
        Bytes32,
        ContractId,
        MessageId,
        Word,
    },
    services::txpool::{
        ArcPoolTx,
        InsertionResult,
        TxInfo,
    },
};
use fuel_vm::storage::ContractsRawCode;
use std::{
    fmt::Debug,
    sync::Arc,
};
use thiserror::Error;
use tokio::sync::{
    mpsc,
    oneshot,
};

pub trait TxPoolDb:
    StorageInspect<Coins, Error = StorageError>
    + StorageInspect<ContractsRawCode, Error = StorageError>
    + StorageInspect<Messages, Error = StorageError>
    + Send
    + Sync
{
    fn utxo(&self, utxo_id: &UtxoId) -> Result<Option<Coin>, StorageError> {
        self.storage::<Coins>()
            .get(utxo_id)
            .map(|t| t.map(|t| t.as_ref().clone()))
    }

    fn contract_exist(&self, contract_id: &ContractId) -> Result<bool, StorageError> {
        self.storage::<ContractsRawCode>().contains_key(contract_id)
    }

    fn message(&self, message_id: &MessageId) -> Result<Option<Message>, StorageError> {
        self.storage::<Messages>()
            .get(message_id)
            .map(|t| t.map(|t| t.as_ref().clone()))
    }

    fn current_block_height(&self) -> Result<BlockHeight, StorageError>;
}

/// RPC client for doing calls to the TxPool through an MPSC channel.
#[derive(Clone, Deref, DerefMut)]
pub struct Sender(mpsc::Sender<TxPoolMpsc>);

impl Sender {
    pub fn new(sender: mpsc::Sender<TxPoolMpsc>) -> Self {
        Self(sender)
    }

    pub async fn insert(
        &self,
        txs: Vec<Arc<Transaction>>,
    ) -> anyhow::Result<Vec<anyhow::Result<InsertionResult>>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::Insert { txs, response }).await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn find(&self, ids: Vec<TxId>) -> anyhow::Result<Vec<Option<TxInfo>>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::Find { ids, response }).await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn find_one(&self, id: TxId) -> anyhow::Result<Option<TxInfo>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::FindOne { id, response }).await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn find_dependent(&self, ids: Vec<TxId>) -> anyhow::Result<Vec<ArcPoolTx>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::FindDependent { ids, response })
            .await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn filter_by_negative(&self, ids: Vec<TxId>) -> anyhow::Result<Vec<TxId>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::FilterByNegative { ids, response })
            .await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn includable(&self) -> anyhow::Result<Vec<ArcPoolTx>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::Includable { response }).await?;
        receiver.await.map_err(Into::into)
    }

    pub async fn remove(&self, ids: Vec<TxId>) -> anyhow::Result<Vec<ArcPoolTx>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::Remove { ids, response }).await?;
        receiver.await.map_err(Into::into)
    }

    pub fn channel(buffer: usize) -> (Sender, mpsc::Receiver<TxPoolMpsc>) {
        let (sender, reciever) = mpsc::channel(buffer);
        (Sender(sender), reciever)
    }
}

#[async_trait::async_trait]
impl super::poa_coordinator::TransactionPool for Sender {
    async fn pending_number(&self) -> anyhow::Result<usize> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::PendingNumber { response }).await?;
        receiver.await.map_err(Into::into)
    }

    async fn total_consumable_gas(&self) -> anyhow::Result<u64> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::ConsumableGas { response }).await?;
        receiver.await.map_err(Into::into)
    }

    async fn remove_txs(&mut self, ids: Vec<TxId>) -> anyhow::Result<Vec<ArcPoolTx>> {
        let (response, receiver) = oneshot::channel();
        self.send(TxPoolMpsc::Remove { ids, response }).await?;
        receiver.await.map_err(Into::into)
    }
}

/// RPC commands that can be sent to the TxPool through an MPSC channel.
/// Responses are returned using `response` oneshot channel.
#[derive(Debug)]
pub enum TxPoolMpsc {
    /// The number of pending transactions in the pool.
    PendingNumber { response: oneshot::Sender<usize> },
    /// The amount of gas in all includable transactions combined
    ConsumableGas { response: oneshot::Sender<u64> },
    /// Return all sorted transactions that are includable in next block.
    /// This is going to be heavy operation, use it only when needed.
    Includable {
        response: oneshot::Sender<Vec<ArcPoolTx>>,
    },
    /// import list of transaction into txpool. All needed parents need to be known
    /// and parent->child order should be enforced in Vec, we will not do that check inside
    /// txpool and will just drop child and include only parent. Additional restrain is that
    /// child gas_price needs to be lower then parent gas_price. Transaction can be received
    /// from p2p **RespondTransactions** or from userland. Because of userland we are returning
    /// error for every insert for better user experience.
    Insert {
        txs: Vec<Arc<Transaction>>,
        response: oneshot::Sender<Vec<anyhow::Result<InsertionResult>>>,
    },
    /// find all tx by their hash
    Find {
        ids: Vec<TxId>,
        response: oneshot::Sender<Vec<Option<TxInfo>>>,
    },
    /// find one tx by its hash
    FindOne {
        id: TxId,
        response: oneshot::Sender<Option<TxInfo>>,
    },
    /// find all dependent tx and return them with requested dependencies in one list sorted by Price.
    FindDependent {
        ids: Vec<TxId>,
        response: oneshot::Sender<Vec<ArcPoolTx>>,
    },
    /// remove transaction from pool needed on user demand. Low priority
    Remove {
        ids: Vec<TxId>,
        response: oneshot::Sender<Vec<ArcPoolTx>>,
    },
    /// Iterate over `hashes` and return all hashes that we don't have.
    /// Needed when we receive list of new hashed from peer with
    /// **BroadcastTransactionHashes**, so txpool needs to return
    /// tx that we don't have, and request them from that particular peer.
    FilterByNegative {
        ids: Vec<TxId>,
        response: oneshot::Sender<Vec<TxId>>,
    },
    /// stop txpool
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TxUpdate {
    tx_id: Bytes32,
    squeezed_out: Option<Error>,
}

impl TxUpdate {
    pub fn updated(tx_id: Bytes32) -> Self {
        Self {
            tx_id,
            squeezed_out: None,
        }
    }

    pub fn squeezed_out(tx_id: Bytes32, reason: Error) -> Self {
        Self {
            tx_id,
            squeezed_out: Some(reason),
        }
    }

    pub fn tx_id(&self) -> &Bytes32 {
        &self.tx_id
    }

    pub fn was_squeezed_out(&self) -> bool {
        self.squeezed_out.is_some()
    }

    pub fn into_squeezed_out_reason(self) -> Option<Error> {
        self.squeezed_out
    }
}

#[derive(Error, Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum Error {
    #[error("TxPool required that transaction contains metadata")]
    NoMetadata,
    #[error("TxPool doesn't support this type of transaction.")]
    NotSupportedTransactionType,
    #[error("Transaction is not inserted. Hash is already known")]
    NotInsertedTxKnown,
    #[error("Transaction is not inserted. Pool limit is hit, try to increase gas_price")]
    NotInsertedLimitHit,
    #[error("Transaction is not inserted. The gas price is too low.")]
    NotInsertedGasPriceTooLow,
    #[error(
        "Transaction is not inserted. More priced tx {0:#x} already spend this UTXO output: {1:#x}"
    )]
    NotInsertedCollision(TxId, UtxoId),
    #[error(
        "Transaction is not inserted. More priced tx has created contract with ContractId {0:#x}"
    )]
    NotInsertedCollisionContractId(ContractId),
    #[error(
        "Transaction is not inserted. A higher priced tx {0:#x} is already spending this messageId: {1:#x}"
    )]
    NotInsertedCollisionMessageId(TxId, MessageId),
    #[error(
        "Transaction is not inserted. Dependent UTXO output is not existing: {0:#x}"
    )]
    NotInsertedOutputNotExisting(UtxoId),
    #[error("Transaction is not inserted. UTXO input contract is not existing: {0:#x}")]
    NotInsertedInputContractNotExisting(ContractId),
    #[error("Transaction is not inserted. ContractId is already taken {0:#x}")]
    NotInsertedContractIdAlreadyTaken(ContractId),
    #[error("Transaction is not inserted. UTXO is not existing: {0:#x}")]
    NotInsertedInputUtxoIdNotExisting(UtxoId),
    #[error("Transaction is not inserted. UTXO is spent: {0:#x}")]
    NotInsertedInputUtxoIdSpent(UtxoId),
    #[error("Transaction is not inserted. Message is spent: {0:#x}")]
    NotInsertedInputMessageIdSpent(MessageId),
    #[error("Transaction is not inserted. Message id {0:#x} does not match any received message from the DA layer.")]
    NotInsertedInputMessageUnknown(MessageId),
    #[error(
        "Transaction is not inserted. UTXO requires Contract input {0:#x} that is priced lower"
    )]
    NotInsertedContractPricedLower(ContractId),
    #[error("Transaction is not inserted. Input output mismatch. Coin owner is different from expected input")]
    NotInsertedIoWrongOwner,
    #[error("Transaction is not inserted. Input output mismatch. Coin output does not match expected input")]
    NotInsertedIoWrongAmount,
    #[error("Transaction is not inserted. Input output mismatch. Coin output asset_id does not match expected inputs")]
    NotInsertedIoWrongAssetId,
    #[error("Transaction is not inserted. The computed message id doesn't match the provided message id.")]
    NotInsertedIoWrongMessageId,
    #[error(
        "Transaction is not inserted. Input output mismatch. Expected coin but output is contract"
    )]
    NotInsertedIoContractOutput,
    #[error(
        "Transaction is not inserted. Input output mismatch. Expected coin but output is message"
    )]
    NotInsertedIoMessageInput,
    #[error("Transaction is not inserted. Maximum depth of dependent transaction chain reached")]
    NotInsertedMaxDepth,
    #[error("Transaction exceeds the max gas per block limit. Tx gas: {tx_gas}, block limit {block_limit}")]
    NotInsertedMaxGasLimit { tx_gas: Word, block_limit: Word },
    // small todo for now it can pass but in future we should include better messages
    #[error("Transaction removed.")]
    Removed,
    #[error("Transaction squeezed out because {0}")]
    SqueezedOut(String),
}