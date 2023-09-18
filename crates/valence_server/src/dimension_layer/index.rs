pub use bevy_ecs::prelude::*;
use rustc_hash::FxHashMap;
use valence_protocol::{BlockPos, ChunkPos};

use super::block::{Block, BlockRef};
use super::chunk::{Chunk, LoadedChunk};

/// The mapping of chunk positions to [`LoadedChunk`]s in a dimension layer.
///
/// **NOTE**: By design, directly modifying the chunk index does not send
/// packets to synchronize state with clients.
#[derive(Component, Debug)]
pub struct ChunkIndex {
    map: FxHashMap<ChunkPos, LoadedChunk>,
    height: i32,
}

impl ChunkIndex {
    pub(crate) fn new(height: i32) -> Self {
        Self {
            map: Default::default(),
            height,
        }
    }

    pub fn get(&self, pos: impl Into<ChunkPos>) -> Option<&LoadedChunk> {
        self.map.get(&pos.into())
    }

    pub fn get_mut(&mut self, pos: impl Into<ChunkPos>) -> Option<&mut LoadedChunk> {
        self.map.get_mut(&pos.into())
    }

    pub fn insert(&mut self, pos: impl Into<ChunkPos>, chunk: Chunk) -> Option<Chunk> {
        match self.entry(pos.into()) {
            Entry::Occupied(mut o) => Some(o.insert(chunk)),
            Entry::Vacant(mut v) => {
                v.insert(chunk);
                None
            }
        }
    }

    pub fn remove(&mut self, pos: impl Into<ChunkPos>) -> Option<Chunk> {
        match self.entry(pos.into()) {
            Entry::Occupied(o) => Some(o.remove()),
            Entry::Vacant(_) => None,
        }
    }

    pub fn entry(&mut self, pos: impl Into<ChunkPos>) -> Entry {
        match self.map.entry(pos.into()) {
            std::collections::hash_map::Entry::Occupied(o) => {
                Entry::Occupied(OccupiedEntry { entry: o })
            }
            std::collections::hash_map::Entry::Vacant(v) => Entry::Vacant(VacantEntry {
                entry: v,
                height: self.height,
            }),
        }
    }

    pub fn block(&self, pos: impl Into<BlockPos>) -> Option<BlockRef> {
        todo!()
    }

    pub fn set_block(
        &mut self,
        pos: impl Into<BlockPos>,
        block: impl Into<Block>,
    ) -> Option<Block> {
        todo!()
    }

    // TODO: iter, iter_mut, clear
}

#[derive(Debug)]
pub enum Entry<'a> {
    Occupied(OccupiedEntry<'a>),
    Vacant(VacantEntry<'a>),
}

impl<'a> Entry<'a> {
    pub fn or_default(self) -> &'a mut LoadedChunk {
        match self {
            Entry::Occupied(oe) => oe.into_mut(),
            Entry::Vacant(ve) => ve.insert(Chunk::new()),
        }
    }
}

#[derive(Debug)]
pub struct OccupiedEntry<'a> {
    entry: std::collections::hash_map::OccupiedEntry<'a, ChunkPos, LoadedChunk>,
}

impl<'a> OccupiedEntry<'a> {
    pub fn get(&self) -> &LoadedChunk {
        self.entry.get()
    }

    pub fn get_mut(&mut self) -> &mut LoadedChunk {
        self.entry.get_mut()
    }

    pub fn insert(&mut self, chunk: Chunk) -> Chunk {
        self.entry.get_mut().replace(chunk)
    }

    pub fn into_mut(self) -> &'a mut LoadedChunk {
        self.entry.into_mut()
    }

    pub fn key(&self) -> &ChunkPos {
        self.entry.key()
    }

    pub fn remove(self) -> Chunk {
        self.remove_entry().1
    }

    pub fn remove_entry(self) -> (ChunkPos, Chunk) {
        let (pos, chunk) = self.entry.remove_entry();

        (pos, chunk.into_chunk())
    }
}

#[derive(Debug)]
pub struct VacantEntry<'a> {
    entry: std::collections::hash_map::VacantEntry<'a, ChunkPos, LoadedChunk>,
    height: i32,
}

impl<'a> VacantEntry<'a> {
    pub fn insert(self, chunk: Chunk) -> &'a mut LoadedChunk {
        let mut loaded = LoadedChunk::new(self.height);
        loaded.replace(chunk);

        self.entry.insert(loaded)
    }

    pub fn into_key(self) -> ChunkPos {
        *self.entry.key()
    }

    pub fn key(&self) -> &ChunkPos {
        self.entry.key()
    }
}