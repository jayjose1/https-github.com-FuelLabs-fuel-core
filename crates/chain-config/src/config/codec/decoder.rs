use std::fmt::Debug;

use itertools::Itertools;

use crate::{
    config::{
        contract_balance::ContractBalance,
        contract_state::ContractStateConfig,
    },
    CoinConfig,
    ContractConfig,
    Group,
    MessageConfig,
    StateConfig,
};

use super::GroupResult;

pub enum IntoIter<T> {
    InMemory {
        groups: std::vec::IntoIter<GroupResult<T>>,
    },
    #[cfg(feature = "parquet")]
    Parquet {
        decoder: super::parquet::Decoder<std::fs::File, T>,
    },
}

#[cfg(feature = "parquet")]
impl<T> Iterator for IntoIter<T>
where
    super::parquet::Decoder<std::fs::File, T>: Iterator<Item = GroupResult<T>>,
{
    type Item = super::GroupResult<T>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IntoIter::InMemory { groups } => groups.next(),
            IntoIter::Parquet { decoder } => decoder.next(),
        }
    }
}
#[cfg(not(feature = "parquet"))]
impl<T> Iterator for IntoIter<T> {
    type Item = GroupResult<T>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            IntoIter::InMemory { groups } => groups.next(),
        }
    }
}

#[derive(Clone, Debug)]
enum DataSource {
    #[cfg(feature = "parquet")]
    Parquet { snapshot_dir: std::path::PathBuf },
    InMemory {
        state: StateConfig,
        group_size: usize,
    },
}

#[derive(Clone, Debug)]
pub struct Decoder {
    data_source: DataSource,
}

impl Decoder {
    #[cfg(feature = "std")]
    pub fn json(
        snapshot_dir: impl AsRef<std::path::Path>,
        group_size: usize,
    ) -> anyhow::Result<Self> {
        let path = snapshot_dir.as_ref().join("chain_state.json");

        let mut file = std::fs::File::open(path)?;

        let state = serde_json::from_reader(&mut file)?;

        Ok(Self::in_memory(state, group_size))
    }

    pub fn in_memory(state: StateConfig, group_size: usize) -> Self {
        Self {
            data_source: DataSource::InMemory { state, group_size },
        }
    }

    #[cfg(feature = "parquet")]
    pub fn parquet(snapshot_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            data_source: DataSource::Parquet {
                snapshot_dir: snapshot_dir.into(),
            },
        }
    }

    #[cfg(feature = "std")]
    pub fn detect_encoding(
        snapshot_dir: impl AsRef<std::path::Path>,
        default_group_size: usize,
    ) -> anyhow::Result<Self> {
        let snapshot_dir = snapshot_dir.as_ref();

        if snapshot_dir.join("state.json").exists() {
            return Self::json(snapshot_dir, default_group_size)
        }

        #[cfg(feature = "parquet")]
        return Ok(Self::parquet(snapshot_dir.to_owned()));

        #[cfg(not(feature = "parquet"))]
        anyhow::bail!("Could not detect encoding used in snapshot {snapshot_dir:?}");
    }

    pub fn coins(&self) -> anyhow::Result<IntoIter<CoinConfig>> {
        self.create_iterator(
            |state| &state.coins,
            #[cfg(feature = "parquet")]
            "coins",
        )
    }

    pub fn messages(&self) -> anyhow::Result<IntoIter<MessageConfig>> {
        self.create_iterator(
            |state| &state.messages,
            #[cfg(feature = "parquet")]
            "messages",
        )
    }

    pub fn contracts(&self) -> anyhow::Result<IntoIter<ContractConfig>> {
        self.create_iterator(
            |state| &state.contracts,
            #[cfg(feature = "parquet")]
            "contracts",
        )
    }

    pub fn contract_state(&self) -> anyhow::Result<IntoIter<ContractStateConfig>> {
        self.create_iterator(
            |state| &state.contract_state,
            #[cfg(feature = "parquet")]
            "contract_state",
        )
    }

    pub fn contract_balance(&self) -> anyhow::Result<IntoIter<ContractBalance>> {
        self.create_iterator(
            |state| &state.contract_balance,
            #[cfg(feature = "parquet")]
            "contract_balance",
        )
    }

    fn create_iterator<T: Clone>(
        &self,
        extractor: impl FnOnce(&StateConfig) -> &Vec<T>,
        #[cfg(feature = "parquet")] parquet_filename: &'static str,
    ) -> anyhow::Result<IntoIter<T>> {
        match &self.data_source {
            DataSource::InMemory { state, group_size } => {
                let groups = extractor(state).clone();
                Ok(Self::in_memory_iter(groups, *group_size))
            }
            #[cfg(feature = "parquet")]
            DataSource::Parquet { snapshot_dir } => {
                let path = snapshot_dir.join(format!("{parquet_filename}.parquet"));
                let file = std::fs::File::open(path)?;
                Ok(IntoIter::Parquet {
                    decoder: super::parquet::Decoder::new(file)?,
                })
            }
        }
    }

    fn in_memory_iter<T>(items: Vec<T>, group_size: usize) -> IntoIter<T> {
        let groups = items
            .into_iter()
            .chunks(group_size)
            .into_iter()
            .map(Itertools::collect_vec)
            .enumerate()
            .map(|(index, vec_chunk)| {
                Ok(Group {
                    data: vec_chunk,
                    index,
                })
            })
            .collect_vec()
            .into_iter();

        IntoIter::InMemory { groups }
    }
}
