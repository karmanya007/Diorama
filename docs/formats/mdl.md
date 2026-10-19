# MDL — puppet meshes

Source: the three puppet files of corpus scene 2468489223, read with a probe
script and checked against a RenderDoc capture (the skinned shark matches WE's
pose at the captured `g_Time`). 3D models (`MDLV0023`, vertex format `0x0f`,
corpus scene 3509243656) use a different vertex layout and are not covered yet.

An image object's model JSON names the mesh: `{"material": …, "puppet": "models/x_puppet.mdl"}`.
The object picks animations with `animationlayers: [{animation: <id>, rate, blend, visible, additive}]`.
Files are zero-padded to 1 MiB.

All values little-endian.

```text
"MDLV0013\0"
u32   vertex format flags        0x01800009 for puppets (position, bone indices, weights, uv)
u32   ?  (1)
u32   ?  (1)
char  material path \0
u32   0
u32   vertex bytes               52 per vertex
      vertex: f32 position[3]    image-centred pixels, y up (z = 0)
              u32 bones[4]
              f32 weights[4]
              f32 uv[2]          0..1 over the image (not the padded texture), v down
u32   index bytes                u16 triangle list

"MDLS0001\0"
u32   file offset of the next chunk
u32   bone count
      bone: char name \0
            u32 flags            (1; 0 seen once)
            i32 parent           -1 = root
            u32 matrix bytes     64
            f32 matrix[16]       column-major, relative to the parent (bind pose)
            char data \0         (empty in the corpus)

"MDLA0001\0"
u32   file offset of the end
u32   animation count
      animation: u32 id          referenced by animationlayers[].animation
                 u32 ?  (0)
                 char name \0
                 char mode \0    loop | mirror | single
                 f32 fps
                 u32 frames      length; each track holds frames + 1 samples
                 u32 ?  (0)
                 u32 track count one per bone, in bone order
                 track: u32 ?  (0 or 1)
                        u32 bytes   36 per sample
                        sample: f32 position[3], f32 angles[3] (radians, z = in-plane,
                                counter-clockwise), f32 scale[3]; relative to the parent
```

Frame 0 of each track equals the bone's bind matrix. A vertex is skinned with
`Σ wᵢ · posedᵢ · bindᵢ⁻¹ · v` (world transforms through the parent chain).
Mode `mirror` plays forwards then backwards; samples are interpolated linearly.
