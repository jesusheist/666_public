//! Switches, group 2 (see ../known.rs).

#[allow(unused_imports)]
use crate::{tunable, tunable_list};

tunable!(pub HOST_RDNA: bool = false, "VICTORY_HOST_RDNA", "Translate shaders as for an AMD RDNA card (the translate command only)", live = Restart);
tunable!(pub AMBIENT_ONCE: bool = false, "VICTORY_AMBIENT_ONCE", "The ambient light estimate is one bounce of the light the title's own lighting put there, not of the stand-in's too", live = Restart);
tunable!(pub BOOK: bool = true, "VICTORY_BOOK", "Keep the fault book (diagnostics builds); off keeps none", live = Restart);
tunable!(pub SLOT_CHECK: bool = false, "VICTORY_SLOT_CHECK", "A thread walks the view heap and says which slots hold bad views (diagnostics builds)", live = Restart);
tunable!(pub CAPTURE_WORLD: bool = false, "VICTORY_CAPTURE_WORLD", "Frame capture also saves frames around the first world frame and every 16th before it (diagnostics builds)", live = Restart);
tunable!(pub DUMP_TARGETS: bool = false, "VICTORY_DUMP_TARGETS", "With frame capture on, also save the title textures the listed frames render to or write (diagnostics builds)", live = Restart);
tunable!(pub BARRIER_TEST: enum = "off", ["off", "groups", "indirect", "both"], "VICTORY_BARRIER_TEST", "Timing experiment that leaves out barrier waits: groups, indirect or both (diagnostics builds)", live = Restart);
tunable!(pub COPY_MODE: enum = "guarded", ["guarded", "plain", "none"], "VICTORY_COPY_MODE", "How title memory copies run: guarded, plain (no mapped-page guard) or none; for timing only (diagnostics builds)", live = Restart);
tunable!(pub MEMORY_MAP: bool = true, "VICTORY_MEMORY_MAP", "Walk the address space for the memory map line (diagnostics builds)", live = Restart);
tunable!(pub FAST_CORES: i64 = 2, 0..=3, "VICTORY_FAST_CORES", "Which threads are kept on the fast cores: 0 none, 1 the threads a frame waits for, 2 their helpers too, 3 all working threads", live = Restart);
tunable!(pub SUBMIT_HELPER_PRIORITY: bool = false, "VICTORY_SUBMIT_HELPER_PRIORITY", "Run the threads the submitting thread waits for at its raised priority", live = Restart);
tunable!(pub HEAP_SYNC_ASIDE: bool = true, "VICTORY_HEAP_SYNC_ASIDE", "Do the heap sync beside the rest of the preparing; off does it in front of it", live = Restart);
tunable!(pub HEAP_SYNC_EARLY: i64 = 4, 0..=1000, "VICTORY_HEAP_SYNC_EARLY", "Milliseconds the heap sync thread waits for a submission before making a round of its own (0 none)", live = Restart);
tunable!(pub LENT_HEAP: bool = true, "VICTORY_LENT_HEAP", "Lend the biggest heap to dispatches that need one; off refuses such dispatches", live = Restart);
tunable!(pub VIEW_INLINE: bool = true, "VICTORY_VIEW_INLINE", "Keep views inline in their records; off makes every view an entry", live = Restart);
tunable!(pub DYNAMIC_SLOTS: i64 = 1048576, 0..=4294967295, "VICTORY_DYNAMIC_SLOTS", "The most dynamic descriptor slots a big host heap may give a title heap", live = Restart);
tunable!(pub KEEP_SHOWN: i64 = 1, 0..=2, "VICTORY_KEEP_SHOWN", "A slot written with another texture keeps showing the old one until the new upload runs: 0 never, 1 same kind, 2 any of the same dimension", live = Restart);
tunable!(pub KEEP_VIEWS: bool = true, "VICTORY_KEEP_VIEWS", "Keep views alive after the title nulls them; off makes them null at once", live = Restart);
tunable!(pub LIST_VIEWS: bool = true, "VICTORY_LIST_VIEWS", "A list makes a view its tables hold once; off turns that off", live = Restart);
tunable!(pub QUEUE_MARKS: bool = true, "VICTORY_QUEUE_MARKS", "Use the queue's marks to skip lock takes; off takes the lock every time", live = Restart);
tunable!(pub QUICK_MIRROR: bool = true, "VICTORY_QUICK_MIRROR", "Use the quick heap mirror path; off goes back to the earlier way", live = Restart);
tunable!(pub RAW_VIEWS: bool = true, "VICTORY_RAW_VIEWS", "Make raw twins of buffer views; off makes none", live = Restart);
tunable!(pub SLOT_HOLDS: bool = true, "VICTORY_SLOT_HOLDS", "Slots hold what they view; off leaves holding to the list only", live = Restart);
tunable!(pub STALE_TEXTURES: enum = "start", ["start", "0", "overlap"], "VICTORY_STALE_TEXTURES", "When a written view makes older views of other memory stale: start (default rule), 0 never, overlap wherever their spans overlap", live = Restart);
tunable!(pub TEXTURE_AWAY_MS: i64 = 3000, 0..=600000, "VICTORY_TEXTURE_AWAY_MS", "Milliseconds a texture must stay unmapped before its slot is dropped (0 at once)", live = Restart);
tunable!(pub VIEW_BYTES: bool = true, "VICTORY_VIEW_BYTES", "Read buffer views off their copy's elements; off leaves them null", live = Restart);
tunable!(pub VIEW_COPY: bool = true, "VICTORY_VIEW_COPY", "Make view copies in a staging heap; off makes them in place", live = Restart);
tunable!(pub VIEW_EXTENT: bool = true, "VICTORY_VIEW_EXTENT", "Take a view's extent from the copy it shows; off looks it up the earlier way", live = Restart);
tunable!(pub AMBIENT_SPECULAR_ALPHA: bool = false, "VICTORY_AMBIENT_SPECULAR_ALPHA", "Add the alpha layer to the ambient specular pass", live = Restart);
tunable!(pub BAKE_EXACT: bool = true, "VICTORY_BAKE_EXACT", "A decoration bake finds textures as title memory has them at that moment; off takes them as the pass leaves them", live = Restart);
tunable!(pub BAKE_SOURCES: bool = true, "VICTORY_BAKE_SOURCES", "Bring a decoration bake's sources up to date before the bake; off does not", live = Restart);
tunable!(pub BAKE_TEXTURES_MS: i64 = 1500, 0..=600000, "VICTORY_BAKE_TEXTURES_MS", "Milliseconds the texture upload pass may take just before a decoration bake draw (0 nothing special)", live = Restart);
tunable!(pub BIND_CACHE: bool = true, "VICTORY_BIND_CACHE", "Cache repeated bindings of a draw; off binds every time", live = Restart);
tunable!(pub CARD_ACCOUNTING: bool = false, "VICTORY_CARD_ACCOUNTING", "Count what the textures picked by index would need on the card, with a line when it first has no room", live = Restart);
tunable!(pub EARLY_TEXTURES_MS: i64 = 12, 0..=600000, "VICTORY_EARLY_TEXTURES_MS", "Milliseconds of every frame the index-picked texture upload pass takes before a shader asks for it (0 never)", live = Restart);
tunable!(pub FULL_CARD_PASS: bool = true, "VICTORY_FULL_CARD_PASS", "Run the full card pass that gives back slots needing a new host copy; off goes back to the earlier pass", live = Restart);
tunable!(pub HEAP_RECHECK: bool = true, "VICTORY_HEAP_RECHECK", "Check heap textures again after their first upload; off uploads once", live = Restart);
tunable!(pub HEAP_SPREAD: bool = true, "VICTORY_HEAP_SPREAD", "Spread the heap pass over frames, each slot at every sixteenth; off does all of them each pass", live = Restart);
tunable!(pub HEAP_TEXTURES: bool = true, "VICTORY_HEAP_TEXTURES", "Upload textures picked by index from the heap; off turns it off", live = Restart);
tunable!(pub QUEUE_BATCH: bool = true, "VICTORY_QUEUE_BATCH", "Ask the host heap for due texture slots in one batch; off asks each one told", live = Restart);
tunable!(pub READY_ONCE: bool = true, "VICTORY_READY_ONCE", "Leave a ready entry as it is after use; off drops every entry", live = Restart);
tunable!(pub RECHECK_SOON: bool = true, "VICTORY_RECHECK_SOON", "Check again in the next frame; off waits four frames", live = Restart);

tunable_list!(pub(super) fn register:
    HOST_RDNA, AMBIENT_ONCE, BOOK, SLOT_CHECK, CAPTURE_WORLD, DUMP_TARGETS, BARRIER_TEST, COPY_MODE, MEMORY_MAP,
    FAST_CORES, SUBMIT_HELPER_PRIORITY, HEAP_SYNC_ASIDE, HEAP_SYNC_EARLY, LENT_HEAP, VIEW_INLINE, DYNAMIC_SLOTS,
    KEEP_SHOWN, KEEP_VIEWS, LIST_VIEWS, QUEUE_MARKS, QUICK_MIRROR, RAW_VIEWS, SLOT_HOLDS, STALE_TEXTURES,
    TEXTURE_AWAY_MS, VIEW_BYTES, VIEW_COPY, VIEW_EXTENT, AMBIENT_SPECULAR_ALPHA, BAKE_EXACT, BAKE_SOURCES,
    BAKE_TEXTURES_MS, BIND_CACHE, CARD_ACCOUNTING, EARLY_TEXTURES_MS, FULL_CARD_PASS, HEAP_RECHECK, HEAP_SPREAD,
    HEAP_TEXTURES, QUEUE_BATCH, READY_ONCE, RECHECK_SOON
);
