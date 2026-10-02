//! Content identity for uploaded textures.
//!
//! # Why the renderer hashes texels
//!
//! Two materials that name the same image are the ordinary case, not the exception: an
//! environment library shares one bark texture across every tree, and a glTF file that packs
//! its occlusion, roughness and metallic into one image hands the same image to the ORM slot
//! of every material that uses it. Before this existed, each of those became its own GPU
//! texture, its own upload, and its own descriptor — the same bytes resident several times.
//!
//! Identity has to be the *content*, not the path or the array index. Two `.glb` files
//! exported from the same source contain the same image bytes under two different indices,
//! and an asset produced procedurally has no path at all. Content is the only thing the two
//! have in common, and it is the only thing that answers "is this the texture I already
//! have" correctly for both.
//!
//! # The hash, and why a hash is enough
//!
//! Two independent 64-bit lanes over the same bytes, a *word* at a time: each step rotates
//! the lane, folds in the next eight bytes and multiplies by a dense odd constant (the
//! FxHash step), the second lane consuming every word with its bits reversed. A tail shorter
//! than a word goes through byte-at-a-time FNV-1a, the length is folded in, and each lane
//! ends with MurmurHash3's finalizer so every input bit reaches every output bit. It is a few
//! lines, has no dependency, and is entirely deterministic — the same image hashes the same
//! on every run and every machine, which is what will let a processed asset be cached by
//! content later.
//!
//! It used to be FNV-1a a *byte* at a time, and that was 46 ms of a 400 ms launch: a 2048x2048
//! texture is sixteen million bytes, and each byte waited on the previous byte's multiply. A
//! word at a time is the same dependency chain an eighth as long. Nothing stores these values
//! between runs, so changing the function changed nothing but the time
//! (docs/PERFORMANCE_AUDIT.md).
//!
//! A hash is enough because the *key* is not the hash alone: it carries the dimensions and
//! the colour space too, so a collision would have to be two different images of exactly the
//! same size in exactly the same colour space whose texels collide in a 128-bit space. At
//! that width the probability is not a risk anyone budgets for. Storing the texels to compare
//! them byte-for-byte on a hit is the alternative, and it means keeping every texture's CPU
//! copy resident forever to defend against an event that will not happen.
//!
//! Cryptographic strength is not the property being bought here — an adversary is not
//! supplying textures — so a fast non-cryptographic hash is the right tool.

/// FNV-1a's 64-bit prime, used for both lanes' byte-at-a-time tail.
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

/// The multiplier for a whole word: dense and odd, so a multiply is a bijection that moves
/// every bit of the word into the bits above it. FxHash's constant.
const WORD_MULTIPLIER_A: u64 = 0x517c_c1b7_2722_0a95;

/// The second lane's multiplier: the 64-bit golden ratio, unrelated to the first.
const WORD_MULTIPLIER_B: u64 = 0x9e37_79b9_7f4a_7c15;

/// FNV-1a's 64-bit offset basis: the first lane's starting state.
const FNV_OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;

/// A second, unrelated starting state for the other lane.
///
/// Any constant that is not the first one gives an independent lane; this is the
/// widely-used 64-bit variant of the offset basis with its halves exchanged, chosen because
/// it is unmistakably not a value that could be confused for the first.
const FNV_OFFSET_B: u64 = 0x8422_2325_cbf2_9ce4;

/// A 128-bit content hash of a texel buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(u128);

impl ContentHash {
    /// Hashes a byte slice.
    ///
    /// Deterministic: the same bytes always produce the same value, on every run and on
    /// every machine. Nothing here depends on pointer values, iteration order or the
    /// standard library's randomized hasher, all three of which would break the caching this
    /// exists for.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut low = FNV_OFFSET_A;
        let mut high = FNV_OFFSET_B;
        let mut words = bytes.chunks_exact(8);
        for word in &mut words {
            let mut array = [0_u8; 8];
            array.copy_from_slice(word);
            let word = u64::from_le_bytes(array);
            // The rotation carries the multiply's well-mixed high bits back down to the
            // low ones before the next word lands on them.
            low = (low.rotate_left(5) ^ word).wrapping_mul(WORD_MULTIPLIER_A);
            // The second lane consumes the same word with its bits reversed, so that two
            // buffers whose difference cancels in one lane cannot cancel in both.
            high = (high.rotate_left(5) ^ word.reverse_bits()).wrapping_mul(WORD_MULTIPLIER_B);
        }
        for byte in words.remainder() {
            low ^= u64::from(*byte);
            low = low.wrapping_mul(FNV_PRIME);
            high ^= u64::from(byte.reverse_bits());
            high = high.wrapping_mul(FNV_PRIME);
        }
        // The length, folded in at the end: a buffer of trailing zeros and the same buffer
        // with more of them are different images.
        let length = bytes.len() as u64;
        low ^= length;
        low = low.wrapping_mul(FNV_PRIME);
        high ^= length.rotate_left(32);
        high = high.wrapping_mul(FNV_PRIME);
        Self((u128::from(finalize(high)) << 64) | u128::from(finalize(low)))
    }

    /// The hash as a number, for logging and for tests that want to see two differ.
    #[must_use]
    pub fn value(self) -> u128 {
        self.0
    }
}

/// MurmurHash3's 64-bit finalizer: every input bit flips each output bit with probability
/// close to one half, so a lane's last steps, which only ever carry differences upward, end
/// with them spread across all 64 bits.
fn finalize(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

/// Everything that makes two uploaded textures interchangeable.
///
/// The hash is not the whole key, and the two other fields are not decoration.
///
/// Dimensions, because two images can hold the same bytes and mean different pictures — a
/// 4x1 and a 1x4 strip of the same four texels.
///
/// And the *kind*, which decides two things at once: the colour space of the view, so the
/// same bytes read as colour and as data are two GPU textures; and the mip filter, so the
/// same bytes filtered as a normal map and as data are two different chains under one
/// full-size level. The second half is the one that is easy to miss — a key that carried only
/// the colour space would happily hand a normal map the mips of an ORM map, and the symptom
/// would be lighting that goes subtly wrong with distance and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureKey {
    /// The texel content.
    pub hash: ContentHash,
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// What the image means, which fixes both its view format and its mip filter.
    pub kind: crate::mipmap::TextureKind,
    /// Whether the upload built a mip chain. A texture uploaded with mips and one without
    /// are different resources even for identical texels, so they must not share a slot.
    pub mipped: bool,
}

impl TextureKey {
    /// Builds a key for an image about to be uploaded.
    #[must_use]
    pub fn new(
        texels: &[u8],
        width: u32,
        height: u32,
        kind: crate::mipmap::TextureKind,
        mipped: bool,
    ) -> Self {
        Self {
            hash: ContentHash::of(texels),
            width,
            height,
            kind,
            mipped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_bytes_hash_the_same_and_different_bytes_do_not() {
        let image = vec![7_u8; 4096];
        assert_eq!(ContentHash::of(&image), ContentHash::of(&image.clone()));

        let mut altered = image.clone();
        altered[2048] = 8;
        assert_ne!(
            ContentHash::of(&image),
            ContentHash::of(&altered),
            "one texel of one channel must change the hash"
        );
    }

    #[test]
    fn every_single_bit_flip_of_a_texture_changes_both_lanes() {
        // A buffer longer than a word, with a tail, and a flip of each bit of a word-sized
        // stretch in the middle and of the tail: the word path and the byte path both have to
        // notice a one-bit difference, and in both halves of the hash.
        let image: Vec<u8> = (0..4099_u32).map(|index| (index * 31 + 7) as u8).collect();
        let original = ContentHash::of(&image).value();
        for at in (2040..2056).chain(4096..4099) {
            for bit in 0..8 {
                let mut altered = image.clone();
                altered[at] ^= 1 << bit;
                let changed = ContentHash::of(&altered).value();
                assert_ne!(
                    changed as u64, original as u64,
                    "low lane, byte {at} bit {bit}"
                );
                assert_ne!(
                    changed >> 64,
                    original >> 64,
                    "high lane, byte {at} bit {bit}"
                );
            }
        }
    }

    #[test]
    fn swapped_words_do_not_collide() {
        // The word path's version of the transposition check below.
        let forward: Vec<u8> = (0..16_u8).collect();
        let mut swapped = forward.clone();
        swapped.rotate_left(8);
        assert_ne!(ContentHash::of(&forward), ContentHash::of(&swapped));
    }

    #[test]
    fn length_is_part_of_the_hash() {
        // Two buffers where one is a prefix of the other. Without folding the length in,
        // trailing zeros are nearly free in FNV and these two land very close together.
        let short = vec![0_u8; 16];
        let long = vec![0_u8; 32];
        assert_ne!(ContentHash::of(&short), ContentHash::of(&long));
    }

    #[test]
    fn the_hash_does_not_depend_on_where_the_bytes_live() {
        // The property the cache rests on: the same image decoded from two different files,
        // into two different allocations, is one texture.
        let first = vec![1_u8, 2, 3, 4, 5, 6, 7, 8];
        let second = first.clone();
        assert_eq!(ContentHash::of(&first), ContentHash::of(&second));
        assert_eq!(ContentHash::of(&first[..4]), ContentHash::of(&second[..4]));
    }

    #[test]
    fn transposed_bytes_do_not_collide() {
        // Single-lane FNV separates these fine; the second lane exists so that a difference
        // which cancels in one cannot cancel in both, and this is the cheap check that the
        // second lane is actually consuming something different.
        let forward = [0x0F_u8, 0xF0, 0x33, 0xCC];
        let swapped = [0xF0_u8, 0x0F, 0xCC, 0x33];
        let a = ContentHash::of(&forward);
        let b = ContentHash::of(&swapped);
        assert_ne!(a, b);
        assert_ne!(
            a.value() >> 64,
            b.value() >> 64,
            "the high lane must separate them too"
        );
    }

    #[test]
    fn the_key_separates_images_the_hash_alone_would_not() {
        use crate::mipmap::TextureKind;

        let texels = vec![255_u8; 16];
        let wide = TextureKey::new(&texels, 4, 1, TextureKind::Data, false);
        let tall = TextureKey::new(&texels, 1, 4, TextureKind::Data, false);
        assert_ne!(
            wide, tall,
            "the same bytes at different shapes are two images"
        );

        let colour = TextureKey::new(&texels, 2, 2, TextureKind::Color, false);
        let data = TextureKey::new(&texels, 2, 2, TextureKind::Data, false);
        assert_ne!(
            colour, data,
            "an sRGB view and a linear one are two textures"
        );

        // The case a colour-space-only key gets wrong: both are linear, and their chains are
        // filtered by different rules.
        let normal = TextureKey::new(&texels, 2, 2, TextureKind::Normal, true);
        let mipped_data = TextureKey::new(&texels, 2, 2, TextureKind::Data, true);
        assert_ne!(
            normal, mipped_data,
            "a normal map's chain is renormalized and a data map's is not"
        );

        let flat = TextureKey::new(&texels, 2, 2, TextureKind::Data, false);
        assert_ne!(mipped_data, flat, "a mip chain is part of the resource");
    }
}
