mod receipts;
pub(crate) use receipts::Receipts;

use crate::PrunerError;
use reth_db::database::Database;
use reth_interfaces::RethResult;
use reth_primitives::{BlockNumber, PruneCheckpoint, PruneMode, PruneSegment, TxNumber};
use reth_provider::{
    BlockReader, DatabaseProviderRW, PruneCheckpointReader, PruneCheckpointWriter,
};
use std::ops::RangeInclusive;
use tracing::error;

/// A segment represents a pruning of some portion of the data.
///
/// Segments are called from [Pruner](crate::Pruner) with the following lifecycle:
/// 1. Call [Segment::prune] with `delete_limit` of [PruneInput].
/// 2. If [Segment::prune] returned a [Some] in `checkpoint` of [PruneOutput], call
///    [Segment::save_checkpoint].
/// 3. Subtract `pruned` of [PruneOutput] from `delete_limit` of next [PruneInput].
pub(crate) trait Segment {
    /// Segment of the data that's pruned.
    const SEGMENT: PruneSegment;

    /// Prune data for [Self::SEGMENT] using the provided input.
    fn prune<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        input: PruneInput,
    ) -> Result<PruneOutput, PrunerError>;

    /// Save checkpoint for [Self::SEGMENT] to the database.
    fn save_checkpoint<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        checkpoint: PruneCheckpoint,
    ) -> RethResult<()> {
        provider.save_prune_checkpoint(Self::SEGMENT, checkpoint)
    }
}

/// Segment pruning input, see [Segment::prune].
#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneInput {
    /// Target block up to which the pruning needs to be done, inclusive.
    pub(crate) to_block: BlockNumber,
    /// Maximum entries to delete from the database.
    pub(crate) delete_limit: usize,
}

impl PruneInput {
    /// Get next inclusive tx number range to prune according to the checkpoint and `to_block` block
    /// number.
    ///
    /// To get the range start:
    /// 1. If checkpoint exists, get next block body and return its first tx number.
    /// 2. If checkpoint doesn't exist, return 0.
    ///
    /// To get the range end: get last tx number for `to_block`.
    pub(crate) fn get_next_tx_num_range_from_checkpoint<DB: Database>(
        &self,
        provider: &DatabaseProviderRW<'_, DB>,
        segment: PruneSegment,
    ) -> RethResult<Option<RangeInclusive<TxNumber>>> {
        let from_tx_number = provider
            .get_prune_checkpoint(segment)?
            // Checkpoint exists, prune from the next transaction after the highest pruned one
            .and_then(|checkpoint| match checkpoint.tx_number {
                Some(tx_number) => Some(tx_number + 1),
                _ => {
                    error!(target: "pruner", %segment, ?checkpoint, "Expected transaction number in prune checkpoint, found None");
                    None
                },
            })
            // No checkpoint exists, prune from genesis
            .unwrap_or(0);

        let to_tx_number = match provider.block_body_indices(self.to_block)? {
            Some(body) => body,
            None => return Ok(None),
        }
        .last_tx_num();

        let range = from_tx_number..=to_tx_number;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
    }
}

/// Segment pruning output, see [Segment::prune].
#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneOutput {
    /// `true` if pruning has been completed up to the target block, and `false` if there's more
    /// data to prune in further runs.
    pub(crate) done: bool,
    /// Number of entries pruned, i.e. deleted from the database.
    pub(crate) pruned: usize,
    /// Pruning checkpoint to save to database, if any.
    pub(crate) checkpoint: Option<PruneOutputCheckpoint>,
}

impl PruneOutput {
    /// Returns a [PruneOutput] with `done = true`, `pruned = 0` and `checkpoint = None`.
    /// Use when no pruning is needed.
    pub(crate) fn done() -> Self {
        Self { done: true, pruned: 0, checkpoint: None }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PruneOutputCheckpoint {
    /// Highest pruned block number. If it's [None], the pruning for block `0` is not finished yet.
    pub(crate) block_number: Option<BlockNumber>,
    /// Highest pruned transaction number, if applicable.
    pub(crate) tx_number: Option<TxNumber>,
}

impl PruneOutputCheckpoint {
    /// Converts [PruneOutputCheckpoint] to [PruneCheckpoint] with the provided [PruneMode]
    pub(crate) fn as_prune_checkpoint(&self, prune_mode: PruneMode) -> PruneCheckpoint {
        PruneCheckpoint { block_number: self.block_number, tx_number: self.tx_number, prune_mode }
    }
}
