//! Hands the redrawn icon movie to the title.
//!
//! The UI runtime asks one title function, through a pointer, for the address
//! of every movie it loads. [`install`] puts [`movie_address`] in that
//! pointer. The streaming store hands a movie over as an address in memory;
//! when that movie is the icon movie, a redrawn copy is kept. A movie that
//! later loads the icon movie by name is answered with the copy's address;
//! one that asks while the copy is still being drawn gets the original.
//! Nothing else about a request changes.

use std::cell::OnceCell;
use std::ffi::{CString, c_char, c_void};
use std::mem::offset_of;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use victory_formats::gfx;
use victory_loader::{LoadedImage, SlotError};

use super::{HookOffsets, Refusal, active_key, font::Fonts, key_caps_with};

/// The size of the buffer a string's text is read into: text of 511 bytes
/// and its NUL at most.
const TEXT_BUFFER_BYTES: usize = 512;
/// The largest movie copied: the icon movie is 28 KB (Series 1.0.2.1), and
/// a larger one costs a copy and a parse on the loading thread for nothing.
const LARGEST_MOVIE_BYTES: usize = 256 << 10;
/// The most distinct icon movies redrawn in one run; the title has one.
const MOST_DRAWN: usize = 8;
/// The most places in the store remembered at once; the oldest go first.
const MOST_STORED: usize = 64;
/// A UI runtime string is a pointer to a block, with flags in these bits
/// (seen in the strings the title passes, Series 1.0.2.1).
const STRING_FLAG_MASK: usize = 3;
/// Where the text starts in a string's block, after a size and a count
/// (seen there too).
const STRING_TEXT_OFFSET_BYTES: usize = 12;

/// A UI runtime string: a pointer to a block, with flags in its low bits.
#[repr(transparent)]
#[derive(Clone, Copy, Debug)]
struct UiString(usize);

/// The start of what the UI runtime passes to describe the movie it wants:
/// the kind of request and the movie's name. The parent's path follows, and
/// is not read.
#[repr(C)]
struct Location {
    kind: u32,
    name: UiString,
}

const _: () = assert!(offset_of!(Location, kind) == 0 && offset_of!(Location, name) == 8);

/// The requests the hook acts on, by the kind the record names (seen in the
/// title's requests while its first screen loads, Series 1.0.2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Request {
    /// Kind 0: the streaming store has just read the movie into memory.
    HandedOver,
    /// Kind 3: a movie loads another movie by name.
    LoadedByAMovie,
}

impl TryFrom<u32> for Request {
    type Error = u32;

    fn try_from(kind: u32) -> Result<Self, u32> {
        match kind {
            0 => Ok(Self::HandedOver),
            3 => Ok(Self::LoadedByAMovie),
            other => Err(other),
        }
    }
}

type BuildAddress = unsafe extern "system" fn(*mut c_void, *mut UiString, *const Location);
type AssignString = unsafe extern "system" fn(*mut UiString, *const c_char);

/// The title's own functions, set once before the pointer is replaced.
struct Title {
    build_address: BuildAddress,
    assign_string: AssignString,
}

static TITLE: OnceLock<Title> = OnceLock::new();

/// Whether a movie has been answered with a copy yet, so that the first
/// answer is logged.
static SERVED: AtomicBool = AtomicBool::new(false);

/// Warnings logged once each, however often their cause comes back.
#[derive(Clone, Copy, Debug)]
enum Warning {
    Unreadable,
    NothingDrawn,
    TooManyMovies,
}

static WARNED: AtomicU8 = AtomicU8::new(0);

fn warn_once(warning: Warning) {
    let bit = 1 << warning as u8;
    if WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        tracing::warn!(
            ?warning,
            "prompts: the icon movie keeps its gamepad pictures"
        );
    }
}

/// A movie the store handed over and our redrawn copy of it.
struct Drawn {
    original: Box<[u8]>,
    redrawn: &'static [u8],
}

/// Where the store keeps a movie we have a redrawn copy of.
struct Stored {
    tag: String,
    address: usize,
    len: usize,
    redrawn: &'static [u8],
}

/// The redrawn movies, one per distinct original; icon movies that could
/// not be redrawn; and where the store keeps each original. A redrawn movie
/// is never freed: the title may read it for as long as it runs.
struct Kept {
    drawn: Vec<Drawn>,
    refused: Vec<Box<[u8]>>,
    stored: Vec<Stored>,
}

/// Held only to look up or change an entry: never across a call into the
/// title, a redraw or a log line.
static KEPT: Mutex<Kept> = Mutex::new(Kept {
    drawn: Vec::new(),
    refused: Vec::new(),
    stored: Vec::new(),
});

fn kept() -> MutexGuard<'static, Kept> {
    KEPT.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Why the hook was not placed.
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    #[error("the pointer does not lead to code of the image")]
    NotATitlePointer,
    #[error("the hook is already placed")]
    AlreadyPlaced,
    #[error(transparent)]
    Slot(#[from] SlotError),
}

/// Replaces the title's movie address pointer with [`movie_address`]. The
/// pointer is read and checked, and the title's functions kept, before it
/// is written.
///
/// # Safety
///
/// `image` is the mapped and bound image of the build `offsets` belong to,
/// and no title thread runs yet.
pub unsafe fn install(image: &LoadedImage, offsets: HookOffsets) -> Result<(), Refused> {
    let assign_string = image.code_address(offsets.string_assign.0)?;
    let slot = offsets.movie_address_builder.0;
    // SAFETY: no title thread runs, per the caller.
    let build_address = unsafe { image.read_pointer(slot) }?;
    let builder_offset = build_address
        .checked_sub(image.base())
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or(Refused::NotATitlePointer)?;
    if image.code_address(builder_offset).is_err() {
        return Err(Refused::NotATitlePointer);
    }
    // SAFETY: both addresses are code of the build the offsets belong to,
    // checked to lie in its code sections, and the types are those
    // functions' arguments.
    let title = unsafe {
        Title {
            build_address: std::mem::transmute::<usize, BuildAddress>(build_address),
            assign_string: std::mem::transmute::<usize, AssignString>(assign_string),
        }
    };
    TITLE.set(title).map_err(|_| Refused::AlreadyPlaced)?;
    let hook: BuildAddress = movie_address;
    // SAFETY: no title thread runs, per the caller.
    unsafe { image.write_pointer(slot, hook as usize) }?;
    Ok(())
}

/// A movie in memory, as the title's address text names it:
/// `memory:$<address, hex>,<length>,<number>:<tag>`.
#[derive(Debug, PartialEq, Eq)]
struct InMemory<'a> {
    address: usize,
    len: usize,
    tag: &'a str,
}

impl<'a> InMemory<'a> {
    /// `None` when the text names no movie, or one shorter than a movie's
    /// header or larger than this module copies.
    fn parse(text: &'a str) -> Option<Self> {
        let (fields, tag) = text.strip_prefix("memory:$")?.split_once(':')?;
        let mut fields = fields.split(',');
        let address = usize::from_str_radix(fields.next()?, 16).ok()?;
        let len: usize = fields.next()?.parse().ok()?;
        (address != 0 && (gfx::HEADER_BYTES..=LARGEST_MOVIE_BYTES).contains(&len)).then_some(Self {
            address,
            len,
            tag,
        })
    }
}

/// The address text for one of our movies, tagged with the name the movie
/// was asked for by. With the store's own tag in the text instead, the
/// title's screen stayed black (Series 1.0.2.1, measured).
fn address_text(movie: &[u8], name: &str) -> Option<CString> {
    CString::new(format!(
        "memory:${:016X},{},0:victory:{name}",
        movie.as_ptr().addr(),
        movie.len()
    ))
    .ok()
}

/// The text of a UI runtime string, read into a buffer of our own.
struct UiText {
    bytes: [u8; TEXT_BUFFER_BYTES],
    len: usize,
}

impl UiText {
    fn as_str(&self) -> &str {
        self.bytes
            .get(..self.len)
            .and_then(|text| std::str::from_utf8(text).ok())
            .unwrap_or_default()
    }

    /// The text of `string`. `None` when it is empty, does not fit the
    /// buffer with its NUL, or is not UTF-8.
    ///
    /// # Safety
    ///
    /// `string` is a live UI runtime string that no other thread changes.
    unsafe fn read(string: UiString) -> Option<Self> {
        let block = string.0 & !STRING_FLAG_MASK;
        if block == 0 {
            return None;
        }
        let from = block.checked_add(STRING_TEXT_OFFSET_BYTES)? as *const u8;
        let mut text = Self {
            bytes: [0; TEXT_BUFFER_BYTES],
            len: 0,
        };
        for at in 0..TEXT_BUFFER_BYTES {
            // SAFETY: the text ends in a NUL, and the read stops at it.
            let byte = unsafe { from.add(at).read() };
            if byte == 0 {
                text.len = at;
                let valid = std::str::from_utf8(text.bytes.get(..at)?).is_ok();
                return (valid && at > 0).then_some(text);
            }
            *text.bytes.get_mut(at)? = byte;
        }
        None
    }
}

/// A copy of the movie at `memory`; `None` when its header does not name
/// an uncompressed movie of that length.
///
/// # Safety
///
/// `memory` names bytes the title holds readable for this call.
unsafe fn copy_of(memory: &InMemory) -> Option<Box<[u8]>> {
    let from = memory.address as *const u8;
    // SAFETY: the first bytes of the ones the title names; `InMemory`
    // names at least a header's worth.
    let header = unsafe { from.cast::<[u8; gfx::HEADER_BYTES]>().read_unaligned() };
    if !gfx::names_movie(&header, memory.len) {
        return None;
    }
    let mut copy = Vec::with_capacity(memory.len);
    // SAFETY: the title names these bytes, and the copy has room for them;
    // they are copied, since the store reuses its buffer.
    unsafe {
        from.copy_to_nonoverlapping(copy.as_mut_ptr(), memory.len);
        copy.set_len(memory.len);
    }
    Some(copy.into_boxed_slice())
}

/// The redrawn copy of `original`, drawn once for each distinct original.
/// `None` when it is not the icon movie or cannot be drawn; an icon movie
/// that cannot be drawn is remembered, and not tried again.
fn redrawn(original: Box<[u8]>) -> Option<&'static [u8]> {
    let drawn_before = |kept: &Kept| {
        kept.drawn
            .iter()
            .find(|drawn| drawn.original == original)
            .map(|drawn| drawn.redrawn)
    };
    {
        let kept = kept();
        if let Some(redrawn) = drawn_before(&kept) {
            return Some(redrawn);
        }
        if kept.refused.contains(&original) {
            return None;
        }
        if kept.drawn.len() >= MOST_DRAWN {
            drop(kept);
            warn_once(Warning::TooManyMovies);
            return None;
        }
    }
    let fonts = OnceCell::new();
    let glyph = |character| fonts.get_or_init(Fonts::open).glyph(character);
    let warning = match key_caps_with(&original, &glyph, &active_key) {
        Ok(drawn) => {
            let drawn = drawn.into_boxed_slice();
            let mut kept = kept();
            if let Some(redrawn) = drawn_before(&kept) {
                return Some(redrawn);
            }
            let redrawn: &'static [u8] = Box::leak(drawn);
            kept.drawn.push(Drawn { original, redrawn });
            drop(kept);
            tracing::info!(
                bytes = redrawn.len(),
                "prompts: key caps drawn into the icon movie"
            );
            return Some(redrawn);
        }
        Err(Refusal::NotIconMovie) => return None,
        Err(Refusal::Unreadable) => Warning::Unreadable,
        Err(Refusal::NothingDrawn) => Warning::NothingDrawn,
    };
    let mut kept = kept();
    if kept.refused.len() < MOST_DRAWN && !kept.refused.contains(&original) {
        kept.refused.push(original);
    }
    drop(kept);
    warn_once(warning);
    None
}

/// Keeps a redrawn copy of the movie at `memory` when it is the icon movie,
/// and forgets what the store kept under its tag before.
///
/// # Safety
///
/// `memory` names bytes the title holds readable for this call.
unsafe fn keep(memory: &InMemory) {
    // SAFETY: forwarded from the caller.
    let redrawn = unsafe { copy_of(memory) }.and_then(redrawn);
    let mut kept = kept();
    kept.stored.retain(|stored| stored.tag != memory.tag);
    let Some(redrawn) = redrawn else {
        return;
    };
    if kept.stored.len() >= MOST_STORED {
        kept.stored.remove(0);
    }
    kept.stored.push(Stored {
        tag: memory.tag.to_owned(),
        address: memory.address,
        len: memory.len,
        redrawn,
    });
}

/// The redrawn copy of the movie the store keeps at `memory`.
fn served(memory: &InMemory) -> Option<&'static [u8]> {
    kept()
        .stored
        .iter()
        .find(|stored| {
            stored.tag == memory.tag && stored.address == memory.address && stored.len == memory.len
        })
        .map(|stored| stored.redrawn)
}

/// Answers a movie that loads the icon movie by name with our copy.
fn serve(title: &Title, out: *mut UiString, name: UiString, memory: &InMemory) {
    let Some(movie) = served(memory) else {
        return;
    };
    // SAFETY: the name in the caller's record, owned by the caller.
    let Some(text) =
        unsafe { UiText::read(name) }.and_then(|name| address_text(movie, name.as_str()))
    else {
        return;
    };
    // SAFETY: the title's own string assignment on the answer string, the
    // way the title's function has just assigned it; the text is copied.
    unsafe { (title.assign_string)(out, text.as_ptr()) };
    if !SERVED.swap(true, Ordering::Relaxed) {
        tracing::info!("prompts: the title loads the icon movie with key caps");
    }
}

/// The title's movie address function, with the icon movie's address
/// replaced by our copy's.
///
/// The UI runtime calls it from any thread that loads a movie. It takes the
/// [`KEPT`] lock to look up or change an entry, and redraws a movie the
/// store hands over outside it.
extern "system" fn movie_address(this: *mut c_void, out: *mut UiString, location: *const Location) {
    let Some(title) = TITLE.get() else {
        return;
    };
    // SAFETY: the title's own function with the title's own arguments.
    unsafe { (title.build_address)(this, out, location) };
    if out.is_null() || location.is_null() {
        return;
    }
    // SAFETY: the caller's record and its answer string, which the caller
    // owns for this call.
    let (Location { kind, name }, answer) =
        unsafe { (location.read_unaligned(), out.read_unaligned()) };
    let Ok(request) = Request::try_from(kind) else {
        return;
    };
    // SAFETY: the answer the title has just assigned, owned by the caller.
    let Some(answer) = (unsafe { UiText::read(answer) }) else {
        return;
    };
    let Some(memory) = InMemory::parse(answer.as_str()) else {
        return;
    };
    match request {
        // SAFETY: the store has just read the movie into the bytes it names.
        Request::HandedOver => unsafe { keep(&memory) },
        Request::LoadedByAMovie => serve(title, out, name, &memory),
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::Mutex;

    use super::super::fixtures::{icon_movie, known_icons};
    use super::{
        InMemory, LARGEST_MOVIE_BYTES, Location, Request, TEXT_BUFFER_BYTES, TITLE, Title,
        UiString, UiText, address_text, movie_address,
    };

    /// A UI runtime string block: twelve bytes of header, then the text and
    /// its NUL, in words so that it is aligned like the title's.
    fn block(text: &[u8]) -> Vec<u64> {
        let mut bytes = vec![0u8; 12];
        bytes.extend(text);
        bytes.push(0);
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        let (words, _) = bytes.as_chunks::<8>();
        words.iter().map(|word| u64::from_le_bytes(*word)).collect()
    }

    fn string(block: &[u64]) -> UiString {
        UiString(block.as_ptr().addr())
    }

    #[test]
    fn an_address_in_memory_gives_its_address_length_and_tag() {
        assert_eq!(
            InMemory::parse("memory:$00000000DEADBEEF,300,0:tag-one"),
            Some(InMemory {
                address: 0xDEAD_BEEF,
                len: 300,
                tag: "tag-one",
            })
        );
    }

    #[test]
    fn other_addresses_are_not_in_memory() {
        for text in [
            "disk:/our/movie",
            "memory:$0000000000000000,10,0:tag",
            "memory:$00000000DEADBEEF,0,0:tag",
            "memory:$00000000DEADBEEF,7,0:tag",
            "memory:$00000000DEADBEEF,ten,0:tag",
            "memory:$00000000DEADBEEF,10,0",
            "memory:$XYZ,10,0:tag",
        ] {
            assert_eq!(InMemory::parse(text), None, "{text}");
        }
        let too_large = format!("memory:$00000000DEADBEEF,{},0:tag", LARGEST_MOVIE_BYTES + 1);
        assert_eq!(InMemory::parse(&too_large), None);
        assert!(InMemory::parse("memory:$00000000DEADBEEF,8,0:tag").is_some());
    }

    #[test]
    fn our_address_text_parses_back_to_our_movie() {
        let movie = [1u8; 9];

        let text = address_text(&movie, "our.movie").unwrap();

        assert_eq!(
            InMemory::parse(text.to_str().unwrap()),
            Some(InMemory {
                address: movie.as_ptr().addr(),
                len: 9,
                tag: "victory:our.movie",
            })
        );
    }

    #[test]
    fn a_string_gives_its_text_whatever_its_flag_bits() {
        let block = block(b"memory:$1,2,0:tag");

        // SAFETY: the block is ours and ends in a NUL.
        let read = unsafe { UiText::read(UiString(block.as_ptr().addr() | 2)) };

        assert_eq!(read.as_ref().map(UiText::as_str), Some("memory:$1,2,0:tag"));
    }

    #[test]
    fn a_null_an_empty_an_overlong_or_a_wrapping_string_gives_nothing() {
        let empty = block(b"");
        let longest = block(&[b'a'; TEXT_BUFFER_BYTES - 1]);
        let overlong = block(&[b'a'; TEXT_BUFFER_BYTES]);

        // SAFETY: a null string and one whose text would start past the end
        // of the address space are not read; the blocks are ours, end in a
        // NUL and are read no further than it.
        let read = unsafe {
            [
                UiText::read(UiString(1)).is_some(),
                UiText::read(UiString(usize::MAX)).is_some(),
                UiText::read(string(&empty)).is_some(),
                UiText::read(string(&longest)).is_some(),
                UiText::read(string(&overlong)).is_some(),
            ]
        };

        assert_eq!(read, [false, false, false, true, false]);
    }

    #[test]
    fn only_two_kinds_of_request_are_acted_on() {
        assert_eq!(Request::try_from(0), Ok(Request::HandedOver));
        assert_eq!(Request::try_from(3), Ok(Request::LoadedByAMovie));
        for kind in [1, 2, 4, u32::MAX] {
            assert_eq!(Request::try_from(kind), Err(kind));
        }
    }

    /// A stand-in for the title's function: the answer is the string its
    /// first argument names.
    unsafe extern "system" fn build_address(
        this: *mut c_void,
        out: *mut UiString,
        _: *const Location,
    ) {
        // SAFETY: the test passes a live answer slot.
        unsafe { out.write(UiString(this.addr())) };
    }

    static ASSIGNED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// A stand-in for the title's string assignment: it records the text.
    unsafe extern "system" fn assign_string(_: *mut UiString, text: *const c_char) {
        // SAFETY: the hook passes NUL-terminated text.
        let text = unsafe { CStr::from_ptr(text) }.to_str().unwrap().to_owned();
        ASSIGNED.lock().unwrap().push(text);
    }

    /// Asks the hook as the UI runtime would: `kind` of request, the name
    /// `name` names, and the store's answer `answer` names. Returns the
    /// texts the hook assigned.
    fn ask(kind: u32, name: &[u64], answer: &[u64]) -> Vec<String> {
        let location = Location {
            kind,
            name: string(name),
        };
        let mut out = UiString(0);
        movie_address(
            answer.as_ptr().cast_mut().cast(),
            &raw mut out,
            &raw const location,
        );
        assert_eq!(out.0, answer.as_ptr().addr());
        std::mem::take(&mut *ASSIGNED.lock().unwrap())
    }

    fn in_memory(movie: &[u8], tag: &str) -> Vec<u64> {
        block(
            format!(
                "memory:${:016X},{},0:{tag}",
                movie.as_ptr().addr(),
                movie.len()
            )
            .as_bytes(),
        )
    }

    #[test]
    fn a_movie_loaded_by_name_is_answered_with_the_redrawn_copy_of_what_the_store_holds() {
        assert!(
            TITLE
                .set(Title {
                    build_address,
                    assign_string,
                })
                .is_ok()
        );
        super::super::pad::use_classic_keys();
        let name = block(b"our.movie");
        let icons = icon_movie(&known_icons());
        let again = icons.clone();
        let other = [&b"GFX\x0b"[..], &13u32.to_le_bytes(), &[0; 5]].concat();
        let (one, two) = (in_memory(&icons, "tag-one"), in_memory(&again, "tag-two"));

        // Nothing is kept yet: the title's answer stands.
        assert_eq!(ask(3, &name, &one), [""; 0]);

        assert_eq!(ask(0, &name, &one), [""; 0]);
        let served = ask(3, &name, &one);
        assert_eq!(served.len(), 1);
        let ours = InMemory::parse(&served[0]).unwrap();
        assert_eq!(ours.tag, "victory:our.movie");
        assert_ne!(ours.address, icons.as_ptr().addr());
        // Only a movie loaded by a movie is answered with the copy.
        assert_eq!(ask(2, &name, &one), [""; 0]);

        // The same movie under another tag shares the copy.
        assert_eq!(ask(0, &name, &two), [""; 0]);
        assert_eq!(ask(3, &name, &two), served);

        // The tag at another address is not the movie the copy was made of.
        let moved = in_memory(&again, "tag-one");
        assert_eq!(ask(3, &name, &moved), [""; 0]);

        // Another movie handed over under the tag ends the copy's use there.
        let replaced = in_memory(&other, "tag-one");
        assert_eq!(ask(0, &name, &replaced), [""; 0]);
        assert_eq!(ask(3, &name, &one), [""; 0]);
        assert_eq!(ask(3, &name, &two), served);
    }
}
