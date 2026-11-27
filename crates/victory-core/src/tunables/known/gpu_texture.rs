//! gpu_texture tunables (see ../known.rs): textures, staging, descriptors, views.

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub TEXTURE_HOT: bool = true, "VICTORY_TEXTURE_HOT",
    "A texture watched by the write guard is looked at every frame (off: every eighth frame)", live = Safe);
tunable!(pub TEXTURE_NO_WAIT: bool = false, "VICTORY_TEXTURE_NO_WAIT",
    "A texture refresh does not wait for a staging buffer in use (the remap may show a frame later)", live = Safe);
tunable!(pub TEXTURE_FILL: bool = true, "VICTORY_TEXTURE_FILL",
    "A staging buffer that holds nothing yet is filled with what is there (off: left empty)", live = Safe);
tunable!(pub TEXTURE_COPY: bool = false, "VICTORY_TEXTURE_COPY",
    "Texture bytes are read through a copy of title memory (off: read in place)", live = NextFrame);
tunable!(pub TEXTURE_ONCE: bool = true, "VICTORY_TEXTURE_ONCE",
    "A texture's staging buffer goes with its upload when nothing asks for a refresh (off: kept)", live = NextFrame);
tunable!(pub TEXTURE_GUARD: bool = true, "VICTORY_TEXTURE_GUARD",
    "The write guard tells a texture is unchanged without hashing (off: hash at every check)", live = Safe);
tunable!(pub TEXTURE_SAME: bool = true, "VICTORY_TEXTURE_SAME",
    "A texture uploaded again with the same bytes is not uploaded (off: not asked)", live = Safe);
tunable!(pub COMMITTED_QUERY: bool = false, "VICTORY_COMMITTED_QUERY",
    "Texture memory is checked as committed by a kernel query (off: by page or views)", live = Safe);
tunable!(pub MAPPED_BY_VIEWS: bool = true, "VICTORY_MAPPED_BY_VIEWS",
    "Streamed textures are checked as mapped by the views of physical pages (off: ask the host)", live = Restart);
tunable!(pub MAPPED_BY_PAGE: bool = false, "VICTORY_MAPPED_BY_PAGE",
    "Texture memory is checked as mapped by whole pages, without the walk", live = Restart);
tunable!(pub HEAP_SYNC_ASIDE: bool = true, "VICTORY_HEAP_SYNC_ASIDE",
    "A submission's heap sync runs on a thread beside the rest of the preparing (off: in front of it)", live = NextFrame);
tunable!(pub HEAP_SYNC_EARLY: i64 = 4, 0..=1000, "VICTORY_HEAP_SYNC_EARLY",
    "Milliseconds the heap sync thread waits for a submission before a round of its own (0: none)", live = Safe);
tunable!(pub LENT_HEAP: bool = true, "VICTORY_LENT_HEAP",
    "A dispatch gets the biggest shader heap lent (off: such a dispatch is dropped)", live = Restart);
tunable!(pub VIEW_INLINE: bool = true, "VICTORY_VIEW_INLINE",
    "Views are kept inline in their records (off: every view is a table entry)", live = Restart);
tunable!(pub SHOW_AFTER: bool = false, "VICTORY_SHOW_AFTER",
    "Texture slots written late are shown after their wait (diagnostic)", live = NextFrame);
tunable!(pub STALE_TEXTURES: enum = "start", ["start", "overlap", "0"], "VICTORY_STALE_TEXTURES",
    "Which placed textures count as stale: start = by start, overlap = by overlapping spans, 0 = never", live = NextFrame);
tunable!(pub VIEW_BYTES: bool = true, "VICTORY_VIEW_BYTES",
    "Views off their copy's elements get a copy of their own bytes (off: left null)", live = Restart);
tunable!(pub RAW_VIEWS: bool = true, "VICTORY_RAW_VIEWS",
    "Raw views get twins in the host heap (off: read the title's slots themselves)", live = Restart);
tunable!(pub DESC_EXPAND: bool = true, "VICTORY_DESC_EXPAND",
    "A capped host heap gives more slots to dynamic tables (off: the old split)", live = Restart);
tunable!(pub DYNAMIC_SLOTS: i64 = -1, -1..=4294967295, "VICTORY_DYNAMIC_SLOTS",
    "Dynamic table slots of a capped host heap (-1: worked out)", live = Restart);
tunable!(pub TEXTURE_AWAY_MS: i64 = 3000, 0..=3600000, "VICTORY_TEXTURE_AWAY_MS",
    "Milliseconds a texture slot's record is away before the slot goes null (0: at once)", live = Safe);
tunable!(pub SLOT_HOLDS: bool = true, "VICTORY_SLOT_HOLDS",
    "A constant buffer view's slot holds its buffer (off: only the list holds it)", live = NextFrame);
tunable!(pub VIEW_COPY: bool = true, "VICTORY_VIEW_COPY",
    "Views are copied through a staging heap (off: written directly)", live = Restart);
tunable!(pub VIEW_EXTENT: bool = true, "VICTORY_VIEW_EXTENT",
    "A buffer view finds its copy by the whole buffer (off: by the view's range alone)", live = Safe);
tunable!(pub KEEP_VIEWS: bool = true, "VICTORY_KEEP_VIEWS",
    "A slot's old view stays shown until the new one is made (off: null at once)", live = Restart);
tunable!(pub QUICK_MIRROR: bool = true, "VICTORY_QUICK_MIRROR",
    "A slot already made from its record is told so without the lock (off: with it)", live = NextFrame);

tunable!(pub HEAP_WORKER: bool = false, "VICTORY_HEAP_WORKER",
    "The pass that uploads textures picked by index runs on a thread of its own (needs VICTORY_LATE_WAIT)", live = Restart);

tunable_list!(pub(super) fn register: HEAP_WORKER, TEXTURE_HOT, TEXTURE_NO_WAIT, TEXTURE_FILL, TEXTURE_COPY, TEXTURE_ONCE, TEXTURE_GUARD, TEXTURE_SAME, COMMITTED_QUERY, MAPPED_BY_VIEWS, MAPPED_BY_PAGE, HEAP_SYNC_ASIDE, HEAP_SYNC_EARLY, LENT_HEAP, VIEW_INLINE, SHOW_AFTER, STALE_TEXTURES, VIEW_BYTES, RAW_VIEWS, DESC_EXPAND, DYNAMIC_SLOTS, TEXTURE_AWAY_MS, SLOT_HOLDS, VIEW_COPY, VIEW_EXTENT, KEEP_VIEWS, QUICK_MIRROR);
