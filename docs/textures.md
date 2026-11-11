# Textures

How an image becomes a GPU texture: what it costs, how it is filtered, how it is sampled, and
how to find out when it goes wrong.

The code is `crates/engine-renderer/src/mipmap.rs` (chain generation and filtering),
`texture_id.rs` (content-addressed deduplication), `resource.rs` (`GpuTexture::with_mips`),
`upload.rs` (`queue_texture_mips`), `renderer.rs` (`create_texture`, `create_model`) and
`crates/engine-assets/src/bc.rs` (the BC1 and BC5 encoders). The sampler is
`mesh_pipeline::static_sampler`. Where a number appears here it is the number in the code, and
where the two disagree the code is right and this document is stale.

## The path an image takes

```text
Model.textures ──► kind, from the material slots that name it   (Color | Data | Normal)
               ──► TextureKey: content hash + shape + kind + srgb
                     └─ already uploaded? reuse the handle, no upload, no descriptor
               ──► mipmap::generate                 level 1..n, filtered per kind, on the CPU
               ──► compression_for                  BC1 / BC5 / None, applied per level
               ──► GpuTexture::with_mips            one resource, MipLevels = n
               ──► queue_texture_mips               one staged copy per subresource
               ──► SRV over the whole chain         MostDetailedMip 0, MipLevels n
```

## Kind decides the filter

A mip filter that does not know what a texture *means* is wrong twice over:

* **Colour** (base colour, emissive) is authored in sRGB, whose numbers are not proportional
  to light. Averaging them directly and reading the result through an sRGB view darkens every
  level — the texture gets muddier with distance. `average_color` decodes to linear, averages,
  re-encodes.
* **Normals** are directions packed into [0,1]. The average of two unit vectors is not a unit
  vector, and the shorter it is the flatter the surface. `average_normal` unpacks, averages,
  renormalizes.
* **Data** (ORM, masks, the UI atlas) is plain numbers. `average_data` is a box filter, which
  is correct.

The asset carries only a colour space per texture, which is not enough — a normal map is
linear, and so is an ORM map. So `Renderer::create_model` works the role out from the material
slots that name the texture *before* it uploads anything,
starting from the colour space and promoting to `Normal` any texture a material's
`normal_texture` claims. A texture named by two slots of different kinds resolves to the first
role that claims it and logs a warning rather than picking silently.

## Colour space

The texels are the same bytes either way; it is the *read* that linearizes. So the colour
space lives in the view format for uncompressed textures — `R8G8B8A8_UNORM_SRGB` against
`R8G8B8A8_UNORM` — and in the format itself for compressed ones, because BC1 has an sRGB
variant and BC5, which only ever holds a direction, does not need one.

Colour-space correctness is a pair, not one decision: an sRGB *view* without linear-space mip
averaging still darkens with distance. Both halves are in place, and the uniform-image test
plus the "colour averages to sRGB 188 where data averages to 128" test are what keep them
together.

## Non-power-of-two

Every level halves and rounds down, stopping at 1x1 — D3D12's own rule, and the one its
`MipLevels` count is defined against. 100x60 gives 50x30, 25x15, 12x7, 6x3, 3x1, 1x1. Where a
dimension is odd the 2x2 block runs off the edge and the sample clamps back onto the last row
or column, which shifts that level by half a texel. That is the standard trade against a
windowed filter that changes width per level.

## Why the CPU, at upload

Not a GPU compute pass. Uploading content is not a per-frame cost; the filter differs per
kind, so a compute path would need three shaders; and a UAV on every sampled texture is a
resource flag on all of them to save milliseconds that happen once.

Compression happens *after* filtering, level by level. Filtering compressed data would mean
decompressing it first, and compressing level 0 and reducing *that* would compound the
format's error down the chain.

## Compression

`mipmap::compression_for` chooses by kind and size:

| Kind                        | Result | Why |
|-----------------------------|--------|-----|
| Colour, opaque, >= 64 px    | BC1    | Half a byte a texel |
| Colour with alpha           | None   | BC1's alpha is one bit per *block*; it frays every cutout edge |
| Normal, >= 64 px            | BC5    | X and Y at one byte a texel, Z reconstructed in the shader |
| ORM                         | None   | Three independent channels, one pair of shared endpoints |
| Anything under 64 px on an edge | None | The saving is kilobytes; on a 16x16 image one bad 4x4 block is a sixteenth of the picture, and the 1x1 fallbacks must stay exact |

## Sampling

One static sampler at `s0` for every material map: `D3D12_FILTER_ANISOTROPIC`, wrapping,
`MaxAnisotropy = 8`, `MaxLOD = FLT_MAX`.

Anisotropic, because trilinear picks a level for the *widest* axis of the pixel footprint, and
on a receding floor that axis is many times the other one — the texture blurs to nothing
halfway to the horizon. Eight rather than sixteen: the visible difference is confined to ground
planes at extreme angles, and this engine is meant to stay playable on weak hardware.
Anisotropy is a rule for *choosing between levels*, so it is worth nothing without a chain —
which is why those two changes are one change. Wrapping rather than clamping because a
character's UVs run across a shared texture and a prism's cap lands exactly on a boundary.

The other two samplers are unchanged and deliberately mip-free: the shadow comparison sampler
at `s1` (`MIP_POINT`, one atlas level) and the contact-shadow depth sampler at `s2`
(`MIN_MAG_MIP_POINT` — the average of two distances is a surface that is not there). The UI
atlas has no chain by design: the CPU already knows the draw size and picks the icon size, and
a chain of an atlas would blend glyphs into icons (`docs/architecture.md`, the interface
section).

## Uploading a chain

`GpuTexture::with_mips` creates one resource with `MipLevels = level_count(w, h)`.
`queue_texture_mips` asserts the supplied level count matches, because a chain with a level
missing leaves that subresource uninitialized — sampled as whatever the heap held before, which
is a bug that appears only at distance. Each level is staged and copied separately, at its own
footprint; a 2D texture with one array slice numbers its subresources by mip level, so
subresource *i* is level *i*.

The SRV names the whole chain (`MostDetailedMip: 0, MipLevels: n`). A view naming one level
would leave the sampler nothing to fall to and the mips would be memory nothing reads.

State tracking stays per-resource, not per-subresource: every level of a texture makes the same
transition at the same time, so there is nothing a per-subresource tracker would express here.
(`docs/phase2.md` records the older state of that, when nothing had mips.)

## Cost

Measured on the environment library —
`cargo test -p engine-renderer --test descriptor_budget -- --nocapture`:

```text
21 materials, 19 texture references resolving to 11 distinct images, 95 descriptors
texture memory: 170 KB resident against 1413 KB for one uncompressed copy per reference (88% saved)
```

A chain adds a third to a texture's own bytes (1 + 1/4 + 1/16 + ... = 4/3). On this content
the net saving is deduplication: the library's textures are small enough that
`compression_for` declines them. The 88% is against the naive baseline — one uncompressed
RGBA8 copy per *reference* — not against the pre-chain build.

Frame cost, RX 6700 XT, GPU p50, one-process benchmark matrix (`--benchmark`):

```text
resolution   before   after
800x450      0.592    0.603
1920x1080    0.721    0.700
```

Nothing measurable. Generation and compression are load-time work, and anisotropic filtering on
this content is inside the noise. Load time is the cost that moved: the whole library uploads
in ~365 ms on this machine, dominated by the per-material constant-buffer upload's own GPU wait
rather than by the filtering.

## Finding out when it goes wrong

* `cargo test -p engine-renderer --test texture_identity` — chain lengths against D3D12's count
  for powers of two, non-powers, strips and single texels; a uniform image surviving its whole
  chain in every filter; colour averaging to sRGB 188 where data averages to 128; filtered
  normals staying unit length; deduplication producing one resource for two identical buffers;
  a normal map and a data map of the same bytes staying apart.
* `cargo test -p engine-renderer --test descriptor_budget -- --nocapture` — resident against
  naive texture memory, distinct images against references, descriptors spent.
* `RUST_LOG=debug` — `create_texture` logs every compressed texture's size, kind, target format
  and before/after KB, and warns on a texture whose material slots disagree about its kind.
* Symptoms worth mapping back: shimmer on a distant wall is a missing chain; a texture that
  gets *muddier* with distance is colour averaged in sRGB space; lighting that flickers as the
  camera moves is a normal map filtered as data; a texture that turns to garbage only far away
  is an uninitialized subresource.
