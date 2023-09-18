use std::borrow::Cow;
use std::mem;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex; // Using nonstandard mutex to avoid poisoning API.
use valence_nbt::{compound, Compound};
use valence_protocol::encode::{PacketWriter, WritePacket};
use valence_protocol::packets::play::chunk_data_s2c::ChunkDataBlockEntity;
use valence_protocol::packets::play::ChunkDataS2c;
use valence_protocol::{BlockState, ChunkPos, Encode};
use valence_registry::biome::BiomeId;
use valence_registry::RegistryIdx;

use super::chunk::{bit_width, ChunkOps};
use super::unloaded::Chunk;
use super::{ChunkLayerInfo, SECTION_BLOCK_COUNT};

/// A chunk that is actively loaded in a [`ChunkLayer`]. This is only accessible
/// behind a reference.
///
/// Like [`Chunk`], loaded chunks implement the [`ChunkOps`] trait so you can
/// use many of the same methods.
///
/// **NOTE:** Loaded chunks are a low-level API. Mutations directly to loaded
/// chunks are intentionally not synchronized with clients. Consider using the
/// relevant methods on [`ChunkLayer`] instead.
///
/// [`ChunkLayer`]: super::ChunkLayer
#[derive(Debug)]
pub struct LoadedChunk {
    /// Chunk data for this loaded chunk.
    chunk: Chunk,
    /// A count of the clients viewing this chunk. Useful for knowing if it's
    /// necessary to record changes, since no client would be in view to receive
    /// the changes if this were zero.
    viewer_count: AtomicU32,
    /// Cached bytes of the chunk initialization packet. The cache is considered
    /// invalidated if empty. This should be cleared whenever the chunk is
    /// modified in an observable way, even if the chunk is not viewed.
    cached_init_packets: Mutex<Vec<u8>>,
}

impl LoadedChunk {
    pub(crate) fn new(height: u32) -> Self {
        Self {
            viewer_count: AtomicU32::new(0),
            chunk: Chunk::with_height(height),
            cached_init_packets: Mutex::new(vec![]),
        }
    }

    /// Sets the content of this chunk to the supplied [`UnloadedChunk`]. The
    /// given unloaded chunk is [resized] to match the height of this loaded
    /// chunk prior to insertion.
    ///
    /// The previous chunk data is returned.
    ///
    /// [resized]: UnloadedChunk::set_height
    pub fn replace(&mut self, mut chunk: Chunk) -> Chunk {
        chunk.set_height(self.height());

        self.cached_init_packets.get_mut().clear();

        mem::replace(&mut self.chunk, chunk)
    }

    pub(super) fn into_chunk(self) -> Chunk {
        self.chunk
    }

    /// Clones this chunk's data into the returned [`Chunk`].
    pub fn to_chunk(&self) -> Chunk {
        self.chunk.clone()
    }

    /// Returns the number of clients in view of this chunk.
    pub fn viewer_count(&self) -> u32 {
        self.viewer_count.load(Ordering::Relaxed)
    }

    /// Like [`Self::viewer_count`], but avoids an atomic operation.
    pub fn viewer_count_mut(&mut self) -> u32 {
        *self.viewer_count.get_mut()
    }

    /// Increments the viewer count.
    pub(crate) fn inc_viewer_count(&self) {
        self.viewer_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Decrements the viewer count.
    #[track_caller]
    pub(crate) fn dec_viewer_count(&self) {
        let old = self.viewer_count.fetch_sub(1, Ordering::Relaxed);
        debug_assert_ne!(old, 0, "viewer count underflow!");
    }

    /// Writes the packet data needed to initialize this chunk.
    pub(crate) fn write_init_packets(
        &self,
        mut writer: impl WritePacket,
        pos: ChunkPos,
        info: &ChunkLayerInfo,
    ) {
        let mut init_packets = self.cached_init_packets.lock();

        if init_packets.is_empty() {
            let heightmaps = compound! {
                // TODO: MOTION_BLOCKING and WORLD_SURFACE heightmaps.
            };

            let mut blocks_and_biomes: Vec<u8> = vec![];

            for sect in &self.chunk.sections {
                sect.count_non_air_blocks()
                    .encode(&mut blocks_and_biomes)
                    .unwrap();

                sect.block_states
                    .encode_mc_format(
                        &mut blocks_and_biomes,
                        |b| b.to_raw().into(),
                        4,
                        8,
                        bit_width(BlockState::max_raw().into()),
                    )
                    .expect("paletted container encode should always succeed");

                sect.biomes
                    .encode_mc_format(
                        &mut blocks_and_biomes,
                        |b| b.to_index() as _,
                        0,
                        3,
                        bit_width(info.biome_registry_len - 1),
                    )
                    .expect("paletted container encode should always succeed");
            }

            let block_entities: Vec<_> = self
                .chunk
                .block_entities
                .iter()
                .filter_map(|(&idx, nbt)| {
                    let x = idx % 16;
                    let z = idx / 16 % 16;
                    let y = idx / 16 / 16;

                    let kind = self.chunk.sections[y as usize / 16]
                        .block_states
                        .get(idx as usize % SECTION_BLOCK_COUNT)
                        .block_entity_kind();

                    kind.map(|kind| ChunkDataBlockEntity {
                        packed_xz: ((x << 4) | z) as i8,
                        y: y as i16 + info.min_y as i16,
                        kind,
                        data: Cow::Borrowed(nbt),
                    })
                })
                .collect();

            PacketWriter::new(&mut init_packets, info.threshold).write_packet(&ChunkDataS2c {
                pos,
                heightmaps: Cow::Owned(heightmaps),
                blocks_and_biomes: &blocks_and_biomes,
                block_entities: Cow::Owned(block_entities),
                sky_light_mask: Cow::Borrowed(&[]),
                block_light_mask: Cow::Borrowed(&[]),
                empty_sky_light_mask: Cow::Borrowed(&[]),
                empty_block_light_mask: Cow::Borrowed(&[]),
                sky_light_arrays: Cow::Borrowed(&[]),
                block_light_arrays: Cow::Borrowed(&[]),
            })
        }

        writer.write_packet_bytes(&init_packets);
    }
}

impl ChunkOps for LoadedChunk {
    fn height(&self) -> u32 {
        self.chunk.height()
    }

    fn block_state(&self, x: u32, y: u32, z: u32) -> BlockState {
        self.chunk.block_state(x, y, z)
    }

    fn set_block_state(&mut self, x: u32, y: u32, z: u32, block: BlockState) -> BlockState {
        let old_block = self.chunk.set_block_state(x, y, z, block);

        if block != old_block {
            self.cached_init_packets.get_mut().clear();
        }

        old_block
    }

    fn fill_block_state_section(&mut self, sect_y: u32, block: BlockState) {
        self.chunk.fill_block_state_section(sect_y, block);

        // TODO: do some checks to avoid calling this sometimes.
        self.cached_init_packets.get_mut().clear();
    }

    fn block_entity(&self, x: u32, y: u32, z: u32) -> Option<&Compound> {
        self.chunk.block_entity(x, y, z)
    }

    fn block_entity_mut(&mut self, x: u32, y: u32, z: u32) -> Option<&mut Compound> {
        let res = self.chunk.block_entity_mut(x, y, z);

        if res.is_some() {
            self.cached_init_packets.get_mut().clear();
        }

        res
    }

    fn set_block_entity(
        &mut self,
        x: u32,
        y: u32,
        z: u32,
        block_entity: Option<Compound>,
    ) -> Option<Compound> {
        self.cached_init_packets.get_mut().clear();

        self.chunk.set_block_entity(x, y, z, block_entity)
    }

    fn clear_block_entities(&mut self) {
        if self.chunk.block_entities.is_empty() {
            return;
        }

        self.chunk.clear_block_entities();

        self.cached_init_packets.get_mut().clear();
    }

    fn biome(&self, x: u32, y: u32, z: u32) -> BiomeId {
        self.chunk.biome(x, y, z)
    }

    fn set_biome(&mut self, x: u32, y: u32, z: u32, biome: BiomeId) -> BiomeId {
        let old_biome = self.chunk.set_biome(x, y, z, biome);

        if biome != old_biome {
            self.cached_init_packets.get_mut().clear();
        }

        old_biome
    }

    fn fill_biome_section(&mut self, sect_y: u32, biome: BiomeId) {
        self.chunk.fill_biome_section(sect_y, biome);

        self.cached_init_packets.get_mut().clear();
    }

    fn shrink_to_fit(&mut self) {
        self.cached_init_packets.get_mut().shrink_to_fit();
        self.chunk.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use valence_protocol::{ident, CompressionThreshold};

    use super::*;

    #[test]
    fn loaded_chunk_changes_clear_packet_cache() {
        #[track_caller]
        fn check<T>(chunk: &mut LoadedChunk, change: impl FnOnce(&mut LoadedChunk) -> T) {
            let info = ChunkLayerInfo {
                dimension_type_name: ident!("whatever").into(),
                height: 512,
                min_y: -16,
                biome_registry_len: 200,
                threshold: CompressionThreshold(-1),
            };

            let mut buf = vec![];
            let mut writer = PacketWriter::new(&mut buf, CompressionThreshold(-1));

            // Rebuild cache.
            chunk.write_init_packets(&mut writer, ChunkPos::new(3, 4), &info);

            // Check that the cache is built.
            assert!(!chunk.cached_init_packets.get_mut().is_empty());

            // Making a change should clear the cache.
            change(chunk);
            assert!(chunk.cached_init_packets.get_mut().is_empty());

            // Rebuild cache again.
            chunk.write_init_packets(&mut writer, ChunkPos::new(3, 4), &info);
            assert!(!chunk.cached_init_packets.get_mut().is_empty());
        }

        let mut chunk = LoadedChunk::new(512);

        check(&mut chunk, |c| {
            c.set_block_state(0, 4, 0, BlockState::ACACIA_WOOD)
        });
        check(&mut chunk, |c| c.set_biome(1, 2, 3, BiomeId::from_index(4)));
        check(&mut chunk, |c| c.fill_biomes(BiomeId::DEFAULT));
        check(&mut chunk, |c| c.fill_block_states(BlockState::WET_SPONGE));
        check(&mut chunk, |c| {
            c.set_block_entity(3, 40, 5, Some(compound! {}))
        });
        check(&mut chunk, |c| {
            c.block_entity_mut(3, 40, 5).unwrap();
        });
        check(&mut chunk, |c| c.set_block_entity(3, 40, 5, None));

        // Old block state is the same as new block state, so the cache should still be
        // intact.
        assert_eq!(
            chunk.set_block_state(0, 0, 0, BlockState::WET_SPONGE),
            BlockState::WET_SPONGE
        );

        assert!(!chunk.cached_init_packets.get_mut().is_empty());
    }
}